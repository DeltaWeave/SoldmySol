/// Token Creation + First Liquidity Monitor
///
/// Monitors for:
/// 1. NEW SPL token mint creation events
/// 2. First liquidity addition to those tokens
/// 3. Immediate sniping when liquidity is detected
///
/// Strategy:
/// - Subscribe to Token Program for mint initialization
/// - Track new tokens in watchlist
/// - Monitor for liquidity pool creation (Raydium/Orca/Meteora)
/// - Buy immediately when first liquidity is added
/// - Exit at 2-5x or if liquidity removed

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
use crate::services::{Database, JupiterService, SolanaConnection};
use crate::services::jupiter::SOL_MINT;
use crate::strategies::route_validation::validate_entry_exit_routes;
use solana_sdk::pubkey::Pubkey;

/// SPL Token Program
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// Raydium AMM Program (for liquidity detection)
const RAYDIUM_AMM: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";

/// Orca Whirlpool Program
const ORCA_WHIRLPOOL: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

/// New token detected
#[derive(Debug, Clone)]
pub struct NewToken {
    pub mint: String,
    pub decimals: u8,
    pub created_at: i64,
}

/// Token with first liquidity
#[derive(Debug, Clone)]
pub struct TokenWithLiquidity {
    pub mint: String,
    pub pool_address: String,
    pub initial_liquidity_sol: f64,
    pub dex: String, // "Raydium", "Orca", etc.
}

/// Active sniped position
#[derive(Debug, Clone)]
struct SnipedPosition {
    token_mint: String,
    entry_price_sol: f64,
    amount_sol: f64,
    target_multiplier: f64,
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

pub struct TokenCreationMonitor {
    config: Config,
    solana: Arc<SolanaConnection>,
    jupiter: Arc<JupiterService>,
    db: Arc<Database>,

    /// Watchlist of new tokens (waiting for liquidity)
    token_watchlist: Arc<tokio::sync::RwLock<HashMap<String, NewToken>>>,

    /// Active sniped positions
    active_positions: Arc<tokio::sync::RwLock<HashMap<String, SnipedPosition>>>,

    /// Seen signatures for token program
    seen_token_sigs: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,

    /// Seen signatures for DEX programs
    seen_dex_sigs: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,

    /// Position size (SOL)
    position_size_sol: f64,

    /// Target profit multiplier
    target_multiplier: f64,
}

impl TokenCreationMonitor {
    pub fn new(
        config: Config,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        db: Arc<Database>,
    ) -> Self {
        Self {
            config,
            solana,
            jupiter,
            db,
            token_watchlist: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_positions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            seen_token_sigs: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            seen_dex_sigs: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
            position_size_sol: 0.2, // Slightly larger than bonding curve positions
            target_multiplier: 5.0, // 5x target for fresh tokens
        }
    }

    /// Start the token creation monitor
    pub async fn start(&self) -> Result<()> {
        info!("🔍 Starting Token Creation Monitor...");
        info!("   Position Size: {} SOL", self.position_size_sol);
        info!("   Target: {}x profit", self.target_multiplier);
        info!("   Monitoring: SPL token creation + first liquidity");

        // Spawn background tasks
        let mint_monitor = self.spawn_mint_monitor();
        let liquidity_monitor = self.spawn_liquidity_monitor();
        let position_monitor = self.spawn_position_monitor();

        // Wait for any task to complete (should never happen)
        tokio::select! {
            _ = mint_monitor => {},
            _ = liquidity_monitor => {},
            _ = position_monitor => {},
        }

        Ok(())
    }

    /// Monitor Token Program for new mint creation
    fn spawn_mint_monitor(&self) -> tokio::task::JoinHandle<()> {
        let ws_url = self.config.rpc.url.replace("https://", "wss://");
        let watchlist = self.token_watchlist.clone();
        let seen_sigs = self.seen_token_sigs.clone();

        tokio::spawn(async move {
            loop {
                if let Err(e) = Self::monitor_token_mints(&ws_url, watchlist.clone(), seen_sigs.clone()).await {
                    error!("Mint monitor error: {}", e);
                    sleep(Duration::from_secs(5)).await;
                }
            }
        })
    }

    /// Monitor for new token mints
    async fn monitor_token_mints(
        ws_url: &str,
        watchlist: Arc<tokio::sync::RwLock<HashMap<String, NewToken>>>,
        seen_sigs: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
    ) -> Result<()> {
        info!("📡 Connecting to WebSocket for Token Program monitoring...");

        let (ws_stream, _) = connect_async(ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("✅ WebSocket connected for token mints");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to Token Program
        let subscribe_msg = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "logsSubscribe",
            "params": [
                {
                    "mentions": [TOKEN_PROGRAM]
                },
                {
                    "commitment": "confirmed"
                }
            ]
        });

        let msg = Message::Text(subscribe_msg.to_string());
        write.send(msg).await?;
        info!("📌 Subscribed to Token Program: {}", TOKEN_PROGRAM);

        let mut message_count = 0u64;

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    message_count += 1;

                    if message_count % 500 == 0 {
                        debug!("📡 Token mint monitor active - {} messages", message_count);
                    }

                    Self::process_token_message(&text, watchlist.clone(), seen_sigs.clone()).await;
                }
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await?;
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    error!("WebSocket error: {:?}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Process token program message
    async fn process_token_message(
        text: &str,
        watchlist: Arc<tokio::sync::RwLock<HashMap<String, NewToken>>>,
        seen_sigs: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
    ) {
        let notification: Result<LogsNotification, _> = serde_json::from_str(text);

        if let Ok(notif) = notification {
            if notif.method == "logsNotification" {
                let logs = &notif.params.result.value.logs;
                let signature = &notif.params.result.value.signature;

                // Deduplicate
                {
                    let mut seen = seen_sigs.write().await;
                    if seen.contains(signature) {
                        return;
                    }
                    seen.insert(signature.clone());

                    if seen.len() > 5000 {
                        let to_remove: Vec<String> = seen.iter().take(2500).cloned().collect();
                        for sig in to_remove {
                            seen.remove(&sig);
                        }
                    }
                }

                // Skip if failed
                if notif.params.result.value.err.is_some() {
                    return;
                }

                // Check for InitializeMint
                if Self::is_mint_initialization(logs) {
                    if let Some(mint) = Self::extract_mint_address(logs) {
                        info!("🆕 NEW TOKEN MINT: {}", &mint[..12]);

                        let new_token = NewToken {
                            mint: mint.clone(),
                            decimals: 9, // Default
                            created_at: chrono::Utc::now().timestamp(),
                        };

                        let mut wl = watchlist.write().await;
                        wl.insert(mint, new_token);

                        // Cleanup old entries (> 10 minutes without liquidity)
                        let now = chrono::Utc::now().timestamp();
                        wl.retain(|_, token| now - token.created_at < 600);

                        info!("📋 Watchlist size: {} tokens", wl.len());
                    }
                }
            }
        }
    }

    /// Check if logs indicate mint initialization
    fn is_mint_initialization(logs: &[String]) -> bool {
        for log in logs {
            if log.contains("InitializeMint") || log.contains("initialize2") {
                return true;
            }
        }
        false
    }

    /// Extract mint address from logs
    fn extract_mint_address(logs: &[String]) -> Option<String> {
        for log in logs {
            if log.contains("Program log:") || log.contains("Program data:") {
                let parts: Vec<&str> = log.split_whitespace().collect();
                for part in parts {
                    if part.len() >= 32 && part.len() <= 44 && part.chars().all(|c| c.is_alphanumeric()) {
                        return Some(part.to_string());
                    }
                }
            }
        }
        None
    }

    /// Monitor DEX programs for liquidity additions
    fn spawn_liquidity_monitor(&self) -> tokio::task::JoinHandle<()> {
        let ws_url = self.config.rpc.url.replace("https://", "wss://");
        let watchlist = self.token_watchlist.clone();
        let positions = self.active_positions.clone();
        let seen_sigs = self.seen_dex_sigs.clone();
        let jupiter = self.jupiter.clone();
        let solana = self.solana.clone();
        let config = self.config.clone();
        let position_size = self.position_size_sol;
        let target_multiplier = self.target_multiplier;

        tokio::spawn(async move {
            loop {
                if let Err(e) = Self::monitor_liquidity_additions(
                    &ws_url,
                    watchlist.clone(),
                    positions.clone(),
                    seen_sigs.clone(),
                    jupiter.clone(),
                    solana.clone(),
                    config.clone(),
                    position_size,
                    target_multiplier,
                ).await {
                    error!("Liquidity monitor error: {}", e);
                    sleep(Duration::from_secs(5)).await;
                }
            }
        })
    }

    /// Monitor Raydium/Orca for liquidity additions
    async fn monitor_liquidity_additions(
        ws_url: &str,
        watchlist: Arc<tokio::sync::RwLock<HashMap<String, NewToken>>>,
        positions: Arc<tokio::sync::RwLock<HashMap<String, SnipedPosition>>>,
        seen_sigs: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
        jupiter: Arc<JupiterService>,
        solana: Arc<SolanaConnection>,
        config: Config,
        position_size: f64,
        target_multiplier: f64,
    ) -> Result<()> {
        info!("📡 Connecting to WebSocket for liquidity monitoring...");

        let (ws_stream, _) = connect_async(ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("✅ WebSocket connected for liquidity events");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to multiple DEX programs
        for (id, program) in [(2, RAYDIUM_AMM), (3, ORCA_WHIRLPOOL)].iter() {
            let subscribe_msg = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "logsSubscribe",
                "params": [
                    {
                        "mentions": [program]
                    },
                    {
                        "commitment": "confirmed"
                    }
                ]
            });

            let msg = Message::Text(subscribe_msg.to_string());
            write.send(msg).await?;
            info!("📌 Subscribed to DEX: {}", program);
        }

        let mut message_count = 0u64;

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    message_count += 1;

                    if message_count % 100 == 0 {
                        debug!("📡 Liquidity monitor active - {} messages", message_count);
                    }

                    Self::process_liquidity_message(
                        &text,
                        watchlist.clone(),
                        positions.clone(),
                        seen_sigs.clone(),
                        jupiter.clone(),
                        solana.clone(),
                        config.clone(),
                        position_size,
                        target_multiplier,
                    ).await;
                }
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await?;
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    error!("WebSocket error: {:?}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Process liquidity event message
    async fn process_liquidity_message(
        text: &str,
        watchlist: Arc<tokio::sync::RwLock<HashMap<String, NewToken>>>,
        positions: Arc<tokio::sync::RwLock<HashMap<String, SnipedPosition>>>,
        seen_sigs: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
        jupiter: Arc<JupiterService>,
        _solana: Arc<SolanaConnection>,
        config: Config,
        position_size: f64,
        target_multiplier: f64,
    ) {
        let notification: Result<LogsNotification, _> = serde_json::from_str(text);

        if let Ok(notif) = notification {
            if notif.method == "logsNotification" {
                let logs = &notif.params.result.value.logs;
                let signature = &notif.params.result.value.signature;

                // Deduplicate
                {
                    let mut seen = seen_sigs.write().await;
                    if seen.contains(signature) {
                        return;
                    }
                    seen.insert(signature.clone());

                    if seen.len() > 5000 {
                        let to_remove: Vec<String> = seen.iter().take(2500).cloned().collect();
                        for sig in to_remove {
                            seen.remove(&sig);
                        }
                    }
                }

                // Skip if failed
                if notif.params.result.value.err.is_some() {
                    return;
                }

                // Check for pool initialization
                if Self::is_pool_initialization(logs) {
                    info!("💧 LIQUIDITY EVENT detected | sig={}", &signature[..12]);

                    // Extract potential token mints
                    if let Some(token_mint) = Self::extract_token_from_pool_logs(logs) {
                        // Check if token is in watchlist
                        let wl = watchlist.read().await;
                        if wl.contains_key(&token_mint) {
                            info!("🎯 WATCHLIST TOKEN GOT LIQUIDITY: {}", &token_mint[..12]);
                            drop(wl);

                            // Execute snipe
                            info!("🚀 Attempting to snipe...");

                            let amount_lamports = (position_size * 1e9) as u64;
                            match validate_entry_exit_routes(
                                &jupiter,
                                &config,
                                SOL_MINT,
                                &token_mint,
                                amount_lamports,
                                config.risk.slippage_bps,
                            )
                            .await
                            {
                                Ok(validation) => {
                                    info!(
                                        "✅ Route validated for {} (impact {}bps)",
                                        &token_mint[..8],
                                        validation.impact_bps
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "⛔ Entry blocked for {}: {}",
                                        &token_mint[..8],
                                        e
                                    );
                                    return;
                                }
                            }

                            // Execute actual buy via Jupiter
                            match jupiter.buy_with_sol(
                                &_solana,
                                &config,
                                &token_mint,
                                position_size
                            ).await {
                                Ok(result) => {
                                    info!("✅ REAL SNIPE EXECUTED: {} | Sig: {:?}", &token_mint[..8], result.signature);

                                    // Log to dashboard
                                    Self::log_snipe_to_dashboard(&token_mint, position_size, &result.signature.unwrap_or_default()).await;

                                    let position = SnipedPosition {
                                        token_mint: token_mint.clone(),
                                        entry_price_sol: position_size,
                                        amount_sol: position_size,
                                        target_multiplier,
                                        entry_time: std::time::Instant::now(),
                                    };

                                    let mut pos = positions.write().await;
                                    pos.insert(token_mint, position);
                                }
                                Err(e) => {
                                    error!("❌ Snipe failed: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Check if logs indicate pool initialization
    fn is_pool_initialization(logs: &[String]) -> bool {
        for log in logs {
            let log_lower = log.to_lowercase();
            if (log_lower.contains("initialize") && log_lower.contains("pool"))
                || log_lower.contains("addliquidity")
                || log_lower.contains("create_pool")
            {
                return true;
            }
        }
        false
    }

    /// Extract token mint from pool logs
    fn extract_token_from_pool_logs(logs: &[String]) -> Option<String> {
        for log in logs {
            if log.contains("Program log:") || log.contains("Program data:") {
                let parts: Vec<&str> = log.split_whitespace().collect();
                for part in parts {
                    if part.len() >= 32 && part.len() <= 44 && part.chars().all(|c| c.is_alphanumeric()) {
                        return Some(part.to_string());
                    }
                }
            }
        }
        None
    }

    /// Log snipe to dashboard log file
    async fn log_snipe_to_dashboard(token_mint: &str, amount_sol: f64, signature: &str) {
        let log_msg = format!(
            "[{}] TOKEN CREATION SNIPE | Token: {} | Amount: {} SOL | Sig: {}\n",
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
            use tracing::warn;
            warn!("Failed to write to dashboard log: {}", e);
        }
    }

    /// Monitor active positions
    fn spawn_position_monitor(&self) -> tokio::task::JoinHandle<()> {
        let positions = self.active_positions.clone();
        let jupiter = self.jupiter.clone();
        let solana = self.solana.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(15)).await;

                let positions_snapshot: Vec<SnipedPosition> = {
                    let pos_guard = positions.read().await;
                    pos_guard.values().cloned().collect()
                };

                if positions_snapshot.is_empty() {
                    continue;
                }

                debug!("📊 Monitoring {} sniped positions", positions_snapshot.len());

                let mut positions_to_remove = Vec::new();

                for position in positions_snapshot {
                    let age_secs = position.entry_time.elapsed().as_secs();

                    // Check current price via Jupiter
                    match jupiter.get_token_price(&position.token_mint).await {
                        Ok(current_price) => {
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

                            debug!("   {} | Age: {}s | Current: {:.2}x",
                                &position.token_mint[..8],
                                age_secs,
                                multiplier
                            );

                            // Exit if target hit
                            if multiplier >= position.target_multiplier {
                                info!("🎯 TARGET HIT: {} at {:.2}x", &position.token_mint[..8], multiplier);
                                if let Err(e) = execute_exit(
                                    &jupiter,
                                    &solana,
                                    &config,
                                    &position.token_mint,
                                ).await {
                                    warn!("Failed to exit position {}: {}", &position.token_mint[..8], e);
                                } else {
                                    positions_to_remove.push(position.token_mint.clone());
                                }
                            }

                            // Emergency exit if held too long (2 hours)
                            if age_secs > 7200 {
                                warn!("⏰ Position {} held too long, should exit", &position.token_mint[..8]);
                                if let Err(e) = execute_exit(
                                    &jupiter,
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
                        Err(e) => {
                            debug!("Failed to get price for {}: {}", &position.token_mint[..8], e);
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

    /// Stop the monitor gracefully
    pub async fn stop(&self) -> Result<()> {
        info!("⏸️  Stopping Token Creation Monitor...");
        info!("✅ Token Creation Monitor stopped");
        Ok(())
    }
}

async fn execute_exit(
    jupiter: &Arc<JupiterService>,
    solana: &Arc<SolanaConnection>,
    config: &Config,
    token_mint: &str,
) -> Result<()> {
    let token_pubkey = Pubkey::from_str(token_mint)?;
    let (token_amount_raw, _decimals) = solana.get_token_balance_raw(&token_pubkey).await?;

    if token_amount_raw == 0 {
        return Err(anyhow::anyhow!("No token balance available for exit"));
    }

    let result = jupiter.sell_for_sol(solana, config, token_mint, token_amount_raw).await?;

    let signature = result.signature.unwrap_or_default();
    let log_msg = format!(
        "[{}] TOKEN CREATION SELL | Token: {} | Amount: {} | Sig: {}\n",
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
