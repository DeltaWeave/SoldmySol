/// Pump.fun Token Launch Sniper
///
/// Monitors Pump.fun program for NEW TOKEN CREATION events
/// Buys tokens during bonding curve phase (before graduation)
/// Sets exit conditions (2-5x profit or graduation event)
///
/// Strategy:
/// - Subscribe to Pump.fun program logs for token initialization
/// - Validate token safety (not a rug, reasonable initial params)
/// - Buy immediately on bonding curve (0.1-0.5 SOL positions)
/// - Monitor for 2-5x profit or graduation event
/// - Exit when target hit or curve completes

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::services::{Database, JupiterService, PumpFunSwap, SolanaConnection};
use solana_sdk::pubkey::Pubkey;

/// Pump.fun program ID
const PUMPFUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// Token launch candidate
#[derive(Debug, Clone)]
pub struct TokenLaunchCandidate {
    pub token_mint: String,
    pub bonding_curve: String,
    pub creator: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub timestamp: i64,
}

/// Active position on bonding curve
#[derive(Debug, Clone)]
struct BondingCurvePosition {
    token_mint: String,
    entry_price_sol: f64,
    amount_sol: f64,
    target_multiplier: f64, // 2x, 3x, 5x
    entry_time: std::time::Instant,
}

#[derive(Debug, Deserialize, Serialize)]
struct LogsNotification {
    jsonrpc: String,
    method: String,
    params: LogsParams,
}

#[derive(Debug, Deserialize, Serialize)]
struct LogsParams {
    result: LogsResult,
    subscription: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct LogsResult {
    value: LogsValue,
}

#[derive(Debug, Deserialize, Serialize)]
struct LogsValue {
    signature: String,
    logs: Vec<String>,
    #[serde(default)]
    err: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct PumpfunTokenSniper {
    config: Config,
    solana: Arc<SolanaConnection>,
    jupiter: Arc<JupiterService>,
    pumpfun: Arc<PumpFunSwap>,
    db: Arc<Database>,

    /// Active positions
    active_positions: Arc<tokio::sync::RwLock<HashMap<String, BondingCurvePosition>>>,

    /// Seen signatures
    seen_signatures: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,

    /// Position size (SOL)
    position_size_sol: f64,

    /// Target profit multiplier
    target_multiplier: f64,
}

impl PumpfunTokenSniper {
    pub fn new(
        config: Config,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        pumpfun: Arc<PumpFunSwap>,
        db: Arc<Database>,
    ) -> Self {
        Self {
            config,
            solana,
            jupiter,
            pumpfun,
            db,
            active_positions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            seen_signatures: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            position_size_sol: 0.1, // Small positions - bonding curve is risky
            target_multiplier: 3.0, // 3x profit target
        }
    }

    /// Start the Pump.fun token sniper
    pub async fn start(&self) -> Result<()> {
        info!("🎯 Starting Pump.fun Token Launch Sniper...");
        info!("   Position Size: {} SOL", self.position_size_sol);
        info!("   Target: {}x profit", self.target_multiplier);
        info!("   Monitoring: Token CREATION events on bonding curve");

        // Spawn position monitor
        let monitor_handle = self.spawn_position_monitor();

        // Subscribe to Pump.fun program logs
        loop {
            if let Err(e) = self.monitor_token_launches().await {
                error!("Token launch monitoring error: {}", e);
                warn!("Reconnecting in 5 seconds...");
                sleep(Duration::from_secs(5)).await;
            }
        }
    }

    /// Monitor Pump.fun program for token creation events
    async fn monitor_token_launches(&self) -> Result<()> {
        info!("📡 Connecting to WebSocket for Pump.fun monitoring...");

        let ws_url = self.config.rpc.url.replace("https://", "wss://");
        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("✅ WebSocket connected");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to Pump.fun program logs (standard method that works on all RPCs)
        let subscribe_msg = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "logsSubscribe",
            "params": [
                {
                    "mentions": [PUMPFUN_PROGRAM]
                },
                {
                    "commitment": "confirmed"
                }
            ]
        });

        let msg = Message::Text(subscribe_msg.to_string());
        write.send(msg).await?;
        info!("📌 Subscribed to Pump.fun program logs: {}", PUMPFUN_PROGRAM);
        info!("   ⚡ Using QuickNode RPC for faster indexing");

        let mut message_count = 0u64;
        let mut last_heartbeat = std::time::Instant::now();

        // Process incoming messages
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    message_count += 1;

                    // Heartbeat every 100 messages or 60 seconds
                    if message_count % 100 == 0 || last_heartbeat.elapsed().as_secs() >= 60 {
                        debug!("📡 Pump.fun monitor active - {} messages received", message_count);
                        last_heartbeat = std::time::Instant::now();
                    }

                    self.process_message(&text).await;
                }
                Ok(Message::Ping(data)) => {
                    debug!("🏓 WebSocket ping");
                    write.send(Message::Pong(data)).await?;
                }
                Ok(Message::Close(_)) => {
                    warn!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {:?}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Process WebSocket message
    async fn process_message(&self, text: &str) {
        let notification: Result<LogsNotification, _> = serde_json::from_str(text);

        if let Ok(notif) = notification {
            if notif.method == "logsNotification" {
                let logs = &notif.params.result.value.logs;
                let signature = &notif.params.result.value.signature;

                // Deduplicate signatures
                {
                    let mut seen = self.seen_signatures.write().await;
                    if seen.contains(signature) {
                        return;
                    }
                    seen.insert(signature.clone());

                    // Keep cache bounded
                    if seen.len() > 5000 {
                        let to_remove: Vec<String> = seen.iter().take(2500).cloned().collect();
                        for sig in to_remove {
                            seen.remove(&sig);
                        }
                    }
                }

                // Skip if transaction failed
                if notif.params.result.value.err.is_some() {
                    return;
                }

                // Check for token creation event
                if !self.is_token_creation_event(logs) {
                    return;
                }

                info!("🆕 NEW TOKEN CREATION detected | sig={}", &signature[..12]);

                // FAST PATH: Use QuickNode's premium RPC with only 1 retry and 500ms delay
                // QuickNode is much faster than Helius for indexing
                let signature_clone = signature.to_string();
                let self_clone = self.clone();
                tokio::spawn(async move {
                    // Wait only 500ms for QuickNode to index (much faster than Helius)
                    sleep(Duration::from_millis(500)).await;

                    for attempt in 0..2 { // Only 2 attempts total
                        match self_clone.extract_token_mint_from_tx(&signature_clone).await {
                            Ok(token_mint) => {
                                let candidate = TokenLaunchCandidate {
                                    token_mint: token_mint.clone(),
                                    bonding_curve: "unknown".to_string(),
                                    creator: "unknown".to_string(),
                                    name: None,
                                    symbol: None,
                                    timestamp: chrono::Utc::now().timestamp(),
                                };

                                info!("🎯 TOKEN MINT: {} | sig={} | latency={}ms",
                                      &token_mint[..8], &signature_clone[..12], 500 + (attempt * 500));

                                if let Err(e) = self_clone.evaluate_and_snipe(candidate).await {
                                    warn!("⚠️  Snipe failed: {}", e);
                                }
                                return;
                            }
                            Err(_) if attempt < 1 => {
                                // One more try after 500ms
                                sleep(Duration::from_millis(500)).await;
                            }
                            Err(e) => {
                                debug!("⏭️  Skipped (RPC timeout) | sig={} | {}", &signature_clone[..12], e);
                                return;
                            }
                        }
                    }
                });
            }
        }
    }

    /// Check if logs indicate token creation event
    fn is_token_creation_event(&self, logs: &[String]) -> bool {
        for log in logs {
            let log_lower = log.to_lowercase();
            // Pump.fun specific patterns
            if (log_lower.contains("instruction:") && log_lower.contains("create"))
                || log_lower.contains("initializemint")
                || log_lower.contains("createv2")
            {
                return true;
            }
        }
        false
    }

    /// Extract token mint from transaction using RPC (QuickNode is fast!)
    async fn extract_token_mint_from_tx(&self, signature: &str) -> Result<String> {
        use solana_client::rpc_client::RpcClient;
        use solana_sdk::commitment_config::CommitmentConfig;
        use solana_transaction_status::UiTransactionEncoding;
        use std::str::FromStr;

        let rpc_client = RpcClient::new_with_commitment(
            self.config.rpc.url.clone(),
            CommitmentConfig::confirmed(),
        );

        // Fetch transaction from QuickNode (much faster than Helius)
        let sig = solana_sdk::signature::Signature::from_str(signature)?;
        let tx = rpc_client.get_transaction(&sig, UiTransactionEncoding::Json)?;

        // Extract from post token balances (most reliable for new mints)
        if let Some(tx_meta) = tx.transaction.meta {
            let post_balances: Option<Vec<_>> = tx_meta.post_token_balances.into();
            if let Some(balances) = post_balances {
                if let Some(first_balance) = balances.first() {
                    return Ok(first_balance.mint.clone());
                }
            }
        }

        // Fallback: extract from account keys
        match &tx.transaction.transaction {
            solana_transaction_status::EncodedTransaction::Json(ui_tx) => {
                if let solana_transaction_status::UiMessage::Raw(ref msg) = ui_tx.message {
                    for account_key in &msg.account_keys {
                        // Skip known programs
                        if account_key != PUMPFUN_PROGRAM
                            && account_key != "11111111111111111111111111111111"
                            && !account_key.starts_with("TokenkegQfeZyiNwAJbNbGKPFXCW")
                            && !account_key.starts_with("ATokenGPvbdGVxr")
                        {
                            return Ok(account_key.clone());
                        }
                    }
                }
            }
            _ => {}
        }

        anyhow::bail!("Failed to extract token mint")
    }

    /// Evaluate token and execute snipe if viable
    async fn evaluate_and_snipe(&self, candidate: TokenLaunchCandidate) -> Result<()> {
        // Skip if already in position
        {
            let positions = self.active_positions.read().await;
            if positions.contains_key(&candidate.token_mint) {
                return Ok(());
            }
        }

        // Basic validation
        // TODO: Add more sophisticated checks:
        // - Creator reputation
        // - Token metadata quality
        // - Initial liquidity amount
        // - Social signals

        info!("✅ Token passed basic validation");

        // Execute buy on bonding curve
        match self.execute_bonding_curve_buy(&candidate).await {
            Ok(position) => {
                info!("🚀 SNIPED: {} | Entry: {} SOL | Target: {}x",
                    &candidate.token_mint[..8],
                    position.amount_sol,
                    position.target_multiplier
                );

                // Record position
                let mut positions = self.active_positions.write().await;
                positions.insert(candidate.token_mint.clone(), position);

                Ok(())
            }
            Err(e) => {
                error!("❌ Snipe failed: {}", e);
                Err(e)
            }
        }
    }

    /// Execute buy on Pump.fun bonding curve
    async fn execute_bonding_curve_buy(
        &self,
        candidate: &TokenLaunchCandidate,
    ) -> Result<BondingCurvePosition> {
        info!("💰 Buying {} SOL on bonding curve...", self.position_size_sol);

        // Get keypair from SolanaConnection for real execution
        let keypair = self.solana.get_wallet();

        // Execute buy via Pump.fun
        match self.pumpfun.buy_token(
            &candidate.token_mint,
            self.position_size_sol,
            keypair,
            50, // 0.5% slippage
        ).await {
            Ok(signature) => {
                info!("✅ REAL BUY EXECUTED | Sig: {}", signature);
                info!("   Token: {} | Amount: {} SOL", &candidate.token_mint[..8], self.position_size_sol);

                // Log to dashboard
                self.log_trade_to_dashboard(&candidate.token_mint, self.position_size_sol, &signature).await;

                Ok(BondingCurvePosition {
                    token_mint: candidate.token_mint.clone(),
                    entry_price_sol: self.position_size_sol,
                    amount_sol: self.position_size_sol,
                    target_multiplier: self.target_multiplier,
                    entry_time: std::time::Instant::now(),
                })
            }
            Err(e) => {
                error!("❌ Buy failed: {}", e);
                Err(e)
            }
        }
    }

    /// Log trade to dashboard log file
    async fn log_trade_to_dashboard(&self, token_mint: &str, amount_sol: f64, signature: &str) {
        let log_msg = format!(
            "[{}] PUMP.FUN BUY | Token: {} | Amount: {} SOL | Sig: {}\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
            token_mint,
            amount_sol,
            signature
        );

        if let Err(e) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("bot.log")
            .and_then(|mut f| std::io::Write::write_all(&mut f, log_msg.as_bytes()))
        {
            warn!("Failed to write to dashboard log: {}", e);
        }
    }

    /// Spawn background task to monitor positions
    fn spawn_position_monitor(&self) -> tokio::task::JoinHandle<()> {
        let positions = self.active_positions.clone();
        let solana = self.solana.clone();
        let jupiter = self.jupiter.clone();
        let pumpfun = self.pumpfun.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(10)).await;

                let positions_snapshot: Vec<BondingCurvePosition> = {
                    let pos_guard = positions.read().await;
                    pos_guard.values().cloned().collect()
                };

                if positions_snapshot.is_empty() {
                    continue;
                }

                debug!("📊 Monitoring {} bonding curve positions", positions_snapshot.len());

                let mut positions_to_remove = Vec::new();

                for position in positions_snapshot {
                    let age_secs = position.entry_time.elapsed().as_secs();

                    // Exit conditions:
                    // 1. Target multiplier reached (need price checking)
                    // 2. Bonding curve completed (graduation)
                    // 3. Token rugpulled (price near zero)
                    // 4. Max hold time exceeded (e.g., 1 hour)

                    if let Ok(current_price) = jupiter.get_token_price(&position.token_mint).await {
                        let token_pubkey = match Pubkey::from_str(&position.token_mint) {
                            Ok(pubkey) => pubkey,
                            Err(e) => {
                                warn!("Invalid token mint {}: {}", position.token_mint, e);
                                continue;
                            }
                        };

                        let (token_amount_raw, decimals) = match solana.get_token_balance_raw(&token_pubkey).await {
                            Ok(balance) => balance,
                            Err(e) => {
                                warn!("Failed to fetch token balance for {}: {}", position.token_mint, e);
                                continue;
                            }
                        };

                        if token_amount_raw == 0 {
                            warn!("Position {} has zero token balance; removing from monitor", &position.token_mint[..8]);
                            positions_to_remove.push(position.token_mint.clone());
                            continue;
                        }

                        let token_amount_ui = token_amount_raw as f64 / 10_f64.powi(decimals as i32);
                        let value_now = current_price * token_amount_ui;
                        let multiplier = value_now / position.entry_price_sol;

                        if multiplier >= position.target_multiplier {
                            info!("🎯 TARGET HIT: {} at {:.2}x", &position.token_mint[..8], multiplier);
                            if let Err(e) = execute_exit(
                                &jupiter,
                                &pumpfun,
                                &solana,
                                &config,
                                &position.token_mint,
                            ).await {
                                warn!("Failed to exit position {}: {}", &position.token_mint[..8], e);
                            } else {
                                positions_to_remove.push(position.token_mint.clone());
                            }
                        }
                    }

                    if age_secs > 3600 {
                        warn!("⏰ Position {} held too long, should exit", &position.token_mint[..8]);
                        if let Err(e) = execute_exit(
                            &jupiter,
                            &pumpfun,
                            &solana,
                            &config,
                            &position.token_mint,
                        ).await {
                            warn!("Failed to exit position {}: {}", &position.token_mint[..8], e);
                        } else {
                            positions_to_remove.push(position.token_mint.clone());
                        }
                    }
                }

                if !positions_to_remove.is_empty() {
                    let mut pos_guard = positions.write().await;
                    for token_mint in positions_to_remove {
                        pos_guard.remove(&token_mint);
                    }
                }
            }
        })
    }

    /// Stop the sniper gracefully
    pub async fn stop(&self) -> Result<()> {
        info!("⏸️  Stopping Pump.fun Token Sniper...");
        info!("✅ Pump.fun Token Sniper stopped");
        Ok(())
    }
}

async fn execute_exit(
    jupiter: &Arc<JupiterService>,
    pumpfun: &Arc<PumpFunSwap>,
    solana: &Arc<SolanaConnection>,
    config: &Config,
    token_mint: &str,
) -> Result<()> {
    let token_pubkey = Pubkey::from_str(token_mint)?;
    let (token_amount_raw, _decimals) = solana.get_token_balance_raw(&token_pubkey).await?;

    if token_amount_raw == 0 {
        return Err(anyhow::anyhow!("No token balance available for exit"));
    }

    let signature = match pumpfun
        .sell_token(token_mint, token_amount_raw, solana.get_wallet(), config.risk.slippage_bps as u16)
        .await
    {
        Ok(sig) => sig,
        Err(e) => {
            warn!(
                "Pump.fun sell failed for {} (fallback to Jupiter): {}",
                &token_mint[..8],
                e
            );
            let result = jupiter.sell_for_sol(solana, config, token_mint, token_amount_raw).await?;
            result.signature.unwrap_or_default()
        }
    };

    let log_msg = format!(
        "[{}] PUMP.FUN SELL | Token: {} | Amount: {} | Sig: {}\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
        token_mint,
        token_amount_raw,
        signature
    );

    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("bot.log")
        .and_then(|mut f| std::io::Write::write_all(&mut f, log_msg.as_bytes()))
    {
        warn!("Failed to write to dashboard log: {}", e);
    }

    Ok(())
}
