use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_transaction_status::{UiTransactionEncoding, EncodedConfirmedTransactionWithStatusMeta};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

// Solana program IDs we're monitoring (EXPANDED)
const RAYDIUM_AMM_PROGRAM: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
const RAYDIUM_CLMM_PROGRAM: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"; // ⭐ NEW: Concentrated Liquidity
const RAYDIUM_MIGRATION_PROGRAM: &str = "39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg"; // ⭐ NEW: Pump.fun → Raydium migration
const PUMPFUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const METEORA_PROGRAM: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
const ORCA_WHIRLPOOL_PROGRAM: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
const PHOENIX_PROGRAM: &str = "PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY"; // ⭐ NEW
const LIFINITY_PROGRAM: &str = "2wT8Yq49kHgDzXuPxZSaeLaH1qbmGXtEyPy64bL7aD3c"; // ⭐ NEW

// Known system programs to IGNORE
const SYSTEM_PROGRAMS: &[&str] = &[
    "ComputeBudget111111111111111111111111111111",
    "11111111111111111111111111111111", // System program
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", // Token program
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", // Token-2022 program
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", // Associated token
];

#[derive(Debug, Clone)]
pub struct NewPoolEvent {
    pub pool_address: String,
    pub token_mint: Option<String>,  // ⭐ NEW: Actual token mint address
    pub program_id: String,
    pub slot: u64,
    pub signature: String,
}

/// Watchlist entry for pools that failed initial extraction
#[derive(Debug, Clone)]
struct WatchlistEntry {
    signature: String,
    program_id: String,
    first_seen: std::time::Instant,
    retry_count: u32,
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

pub struct ProgramMonitor {
    ws_url: String,
    rpc_url: String,
    tx_sender: mpsc::UnboundedSender<NewPoolEvent>,
    seen_signatures: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
    watchlist: Arc<tokio::sync::RwLock<Vec<WatchlistEntry>>>,
}

impl ProgramMonitor {
    pub fn new(ws_url: String, rpc_url: String) -> (Self, mpsc::UnboundedReceiver<NewPoolEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let seen_signatures = Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()));
        let watchlist = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        (Self { ws_url, rpc_url, tx_sender: tx, seen_signatures, watchlist }, rx)
    }

    pub async fn start(&self) -> Result<()> {
        info!("🚀 Starting Solana program monitor...");

        // Spawn watchlist polling task
        let watchlist_clone = self.watchlist.clone();
        let rpc_url_clone = self.rpc_url.clone();
        let tx_sender_clone = self.tx_sender.clone();
        tokio::spawn(async move {
            Self::poll_watchlist(watchlist_clone, rpc_url_clone, tx_sender_clone).await;
        });

        loop {
            if let Err(e) = self.run_monitor().await {
                error!("Program monitor error: {:?}", e);
                warn!("Reconnecting in 5 seconds...");
                sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn run_monitor(&self) -> Result<()> {
        info!("📡 Connecting to Solana WebSocket: {}", self.ws_url);

        let (ws_stream, _) = connect_async(&self.ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        info!("✅ WebSocket connected successfully");

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to ALL program logs (EXPANDED LIST)
        let programs = vec![
            RAYDIUM_AMM_PROGRAM,
            RAYDIUM_CLMM_PROGRAM,        // ⭐ NEW: Concentrated liquidity pools
            RAYDIUM_MIGRATION_PROGRAM,   // ⭐ NEW: Pump.fun graduates migrating to Raydium
            PUMPFUN_PROGRAM,
            METEORA_PROGRAM,
            ORCA_WHIRLPOOL_PROGRAM,
            PHOENIX_PROGRAM,              // ⭐ NEW
            LIFINITY_PROGRAM,             // ⭐ NEW
        ];

        for (i, program_id) in programs.iter().enumerate() {
            let subscribe_msg = json!({
                "jsonrpc": "2.0",
                "id": i + 1,
                "method": "logsSubscribe",
                "params": [
                    {
                        "mentions": [program_id]
                    },
                    {
                        "commitment": "confirmed"
                    }
                ]
            });

            let msg = Message::Text(subscribe_msg.to_string());
            write.send(msg).await?;
            info!("📌 Subscribed to program: {}", program_id);
        }

        info!("✅ Real-time monitoring active - listening for pool creations on {} DEXes...", programs.len());

        let mut message_count = 0u64;
        let mut last_heartbeat = std::time::Instant::now();

        // Process incoming messages
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    message_count += 1;

                    // Heartbeat every 100 messages or 60 seconds
                    if message_count % 100 == 0 || last_heartbeat.elapsed().as_secs() >= 60 {
                        debug!("📡 WebSocket active - received {} messages (listening...)", message_count);
                        last_heartbeat = std::time::Instant::now();
                    }

                    self.process_message(&text).await;
                }
                Ok(Message::Ping(data)) => {
                    debug!("🏓 WebSocket ping received");
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

    async fn process_message(&self, text: &str) {
        // Parse the logs notification
        let notification: Result<LogsNotification, _> = serde_json::from_str(text);

        if let Ok(notif) = notification {
            if notif.method == "logsNotification" {
                let logs = &notif.params.result.value.logs;
                let signature = &notif.params.result.value.signature;

                // ⭐ FIX: Deduplicate signatures
                {
                    let mut seen = self.seen_signatures.write().await;
                    if seen.contains(signature) {
                        return; // Already processed this transaction
                    }
                    seen.insert(signature.clone());

                    // Keep cache size bounded (last 10,000 signatures)
                    if seen.len() > 10000 {
                        // Clear oldest half
                        let to_remove: Vec<String> = seen.iter().take(5000).cloned().collect();
                        for sig in to_remove {
                            seen.remove(&sig);
                        }
                    }
                }

                // Skip if transaction failed
                if notif.params.result.value.err.is_some() {
                    return;
                }

                // ⭐ CRITICAL: Check for NEW POOL CREATION events
                if self.is_liquidity_add_event(logs) {
                    let program_id = self.identify_program(logs);
                    info!("🆕 NEW POOL CREATION detected | program={} | sig={}",
                        Self::get_program_name(&program_id), &signature[..12]);

                    // ⭐ EXTRACT BOTH pool AND token mint from transaction
                    let (pool_address, token_mint) = match self.extract_pool_and_token_from_transaction(signature, &program_id).await {
                        Some((pool, token)) => {
                            info!("✅ Extraction SUCCESS | pool={} | token={:?}", &pool[..12], token.as_ref().map(|t| &t[..12]));
                            (pool, token)
                        }
                        None => {
                            // ADD TO WATCHLIST instead of giving up
                            info!("⏳ WATCHLIST_ADD | sig={} | will_retry", &signature[..12]);
                            let mut watchlist = self.watchlist.write().await;
                            watchlist.push(WatchlistEntry {
                                signature: signature.to_string(),
                                program_id: program_id.clone(),
                                first_seen: std::time::Instant::now(),
                                retry_count: 0,
                            });
                            return;
                        }
                    };

                    // ⭐ CRITICAL FIX: Filter out system programs
                    if self.is_system_program(&pool_address) {
                        info!("⚠️  Skipping system program | pool={}", &pool_address[..12]);
                        return;
                    }

                    // ⭐ CRITICAL FIX: Validate token mint if present
                    if let Some(ref mint) = token_mint {
                        if !self.is_valid_token_mint(mint) {
                            debug!("Skipping invalid token mint: {}", mint);
                            return;
                        }
                    } else {
                        debug!("No token mint extracted for pool: {}", pool_address);
                        // Continue anyway - we'll let Jupiter validation handle it
                    }

                    info!(
                        "🚀 NEW POOL DETECTED! DEX: {}, Pool: {}, Token: {}, Sig: {}",
                        Self::get_program_name(&program_id), pool_address,
                        token_mint.as_ref().unwrap_or(&"UNKNOWN".to_string()),
                        signature
                    );

                    let event = NewPoolEvent {
                        pool_address,
                        token_mint,  // ⭐ Include extracted token mint
                        program_id,
                        slot: 0,
                        signature: signature.clone(),
                    };

                    if let Err(e) = self.tx_sender.send(event) {
                        error!("Failed to send pool event: {:?}", e);
                    }
                }
            }
        }
    }

    /// ⭐ CRITICAL: Detect NEW POOL CREATION events with initial liquidity
    /// We want pools that are being CREATED and initialized with tradeable liquidity
    /// NOT existing pools getting more liquidity added (InitializePosition, OpenPosition, etc.)
    fn is_liquidity_add_event(&self, logs: &[String]) -> bool {
        for log in logs {
            let log_lower = log.to_lowercase();

            // ⭐ RAYDIUM AMM: "Initialize2" = NEW pool creation
            // This creates pool + adds initial liquidity in one transaction
            if log_lower.contains("instruction: initialize2") ||
               (log_lower.contains("initialize2") && log_lower.contains("raydium")) {
                return true;
            }

            // ⭐ RAYDIUM CLMM: "CreatePool" or "InitializePool" = NEW pool
            if log_lower.contains("instruction: createpool") ||
               log_lower.contains("instruction: initializepool") {
                return true;
            }

            // ⭐ ORCA WHIRLPOOL: "InitializePool" = NEW pool creation
            // Note: Different from "OpenPosition" which is adding liquidity to existing pool
            if log_lower.contains("instruction: initializepool") ||
               (log_lower.contains("initializepool") && log_lower.contains("whirlpool")) {
                return true;
            }

            // ⭐ METEORA DLMM: REMOVED - InitializeLbPair/InitializeBinArray are NOT tradable yet
            // These events happen BEFORE liquidity is actually added
            // We need to detect LIQUIDITY ADDITION events for Meteora, not pool init
            // For now, skip Meteora until we have proper liquidity add detection

            // ⭐ PUMP.FUN: Migration to Raydium = NEW tradeable pool
            if log_lower.contains("migrate") || log_lower.contains("graduate") {
                return true;
            }

            // ⭐ PHOENIX: "CreateMarket" = NEW market
            if log_lower.contains("instruction: createmarket") {
                return true;
            }
        }

        false
    }

    // ⭐ NEW: Fetch transaction and extract pool address from account keys
    /// ⭐ NEW: Extract BOTH pool address AND token mint from transaction
    /// This is the KEY to not depending on DexScreener!
    async fn extract_pool_and_token_from_transaction(&self, signature: &str, program_name: &str) -> Option<(String, Option<String>)> {
        let rpc_client = RpcClient::new(self.rpc_url.clone());

        let sig = match Signature::from_str(signature) {
            Ok(s) => s,
            Err(_) => return None,
        };

        // Fetch transaction with retries
        // Start with processed for speed, fall back to confirmed if needed
        let mut retries = 0;
        let transaction = loop {
            let commitment = if retries < 2 {
                CommitmentConfig::processed() // Fast initial attempts
            } else {
                CommitmentConfig::confirmed() // Fall back to confirmed
            };

            match rpc_client.get_transaction_with_config(
                &sig,
                RpcTransactionConfig {
                    encoding: Some(UiTransactionEncoding::JsonParsed),
                    commitment: Some(commitment),
                    max_supported_transaction_version: Some(0),
                }
            ) {
                Ok(tx) => {
                    if retries > 0 {
                        debug!("TX fetched on retry {} with {:?}", retries, commitment);
                    }
                    break tx;
                }
                Err(e) => {
                    retries += 1;
                    if retries > 4 {
                        info!("🔴 RPC_FETCH_FAIL | sig={} | retries={} | error={:?}",
                            &signature[..12], retries, e);
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(100 * retries as u64)).await;
                }
            }
        };

        // Extract accounts from transaction
        use solana_transaction_status::{EncodedTransaction, UiMessage, UiParsedMessage};

        if let EncodedTransaction::Json(ui_tx) = transaction.transaction.transaction {
            if let UiMessage::Parsed(parsed_msg) = ui_tx.message {
                let account_keys: Vec<String> = parsed_msg.account_keys.iter()
                    .map(|key| key.pubkey.clone())
                    .collect();

                // ⭐ CRITICAL: For Raydium AMM initialize2, use FIXED ACCOUNT INDICES
                // Reference: https://www.quicknode.com/guides/solana-development/3rd-party-integrations/track-raydium-lps
                // Account[8] = Token A mint
                // Account[9] = Token B mint
                // Account[4] = AMM ID (pool address)

                if program_name == "Raydium AMM" {
                    let pool_address = if account_keys.len() > 4 {
                        Some(account_keys[4].clone())
                    } else {
                        None
                    };

                    // Extract both token mints
                    let token_a = if account_keys.len() > 8 {
                        Some(account_keys[8].clone())
                    } else {
                        None
                    };

                    let token_b = if account_keys.len() > 9 {
                        Some(account_keys[9].clone())
                    } else {
                        None
                    };

                    // Determine which token is the NEW token (not SOL/USDC)
                    let sol_mint = "So11111111111111111111111111111111111111112";
                    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

                    let token_mint = if let Some(ref ta) = token_a {
                        if ta == sol_mint || ta == usdc_mint {
                            token_b.clone() // Token A is base, so Token B is the new token
                        } else {
                            token_a.clone() // Token A is the new token
                        }
                    } else {
                        token_b.clone()
                    };

                    debug!(
                        "🎯 Raydium Pool Detected - Pool: {:?}, TokenA: {:?}, TokenB: {:?}, New Token: {:?}",
                        pool_address, token_a, token_b, token_mint
                    );

                    if let Some(pool) = pool_address {
                        return Some((pool, token_mint));
                    }
                } else {
                    // For CLMM/Whirlpool: Use known account index patterns
                    // Based on analysis: Pool state is typically at index 4
                    let pool_account_index = match program_name {
                        "Orca Whirlpool" => 4,
                        "Raydium CLMM" => 4,
                        "Meteora" => 4,
                        _ => return None,
                    };

                    let pool_address = if account_keys.len() > pool_account_index {
                        Some(account_keys[pool_account_index].clone())
                    } else {
                        None
                    };

                    // Try to extract token mint (typically in first few token accounts)
                    let mut token_mint: Option<String> = None;
                    for account in account_keys.iter().skip(1).take(5) {
                        if !self.is_system_program(account) && !self.is_monitored_program(account) {
                            token_mint = Some(account.clone());
                            break;
                        }
                    }

                    if let Some(pool) = pool_address {
                        return Some((pool, token_mint));
                    }
                }
            }
        }

        None
    }

    async fn extract_pool_from_transaction(&self, signature: &str) -> Option<String> {
        // Legacy method - calls new method and returns just pool
        self.extract_pool_and_token_from_transaction(signature, "Unknown").await
            .map(|(pool, _token)| pool)
    }

    fn extract_pool_address(&self, logs: &[String]) -> Option<String> {
        // Strategy 1: Look for "Program invoke" followed by address
        // Format: "Program <PROGRAM_ID> invoke [1]" then "Program log: Initialize <POOL_ADDRESS>"
        for (i, log) in logs.iter().enumerate() {
            // Look for initialization patterns with addresses
            if log.contains("Program log:") || log.contains("Program data:") {
                // Extract addresses from this line and next few lines
                for word in log.split_whitespace() {
                    if word.len() >= 32 && word.len() <= 44 {
                        if let Ok(pubkey) = Pubkey::from_str(word) {
                            let addr = pubkey.to_string();
                            // Skip if it's a known program or system account
                            if !self.is_system_program(&addr) && !self.is_monitored_program(&addr) {
                                return Some(addr);
                            }
                        }
                    }
                }
            }
        }

        // Strategy 2: Look for any valid base58 address that's NOT a known program
        for log in logs {
            for word in log.split_whitespace() {
                if word.len() >= 32 && word.len() <= 44 {
                    if let Ok(pubkey) = Pubkey::from_str(word) {
                        let addr = pubkey.to_string();
                        // Skip system programs and monitored programs
                        if !self.is_system_program(&addr) && !self.is_monitored_program(&addr) {
                            return Some(addr);
                        }
                    }
                }
            }
        }

        None
    }

    // ⭐ NEW: Check if address is one of our monitored programs
    fn is_monitored_program(&self, address: &str) -> bool {
        address == RAYDIUM_AMM_PROGRAM
            || address == RAYDIUM_CLMM_PROGRAM
            || address == RAYDIUM_MIGRATION_PROGRAM
            || address == PUMPFUN_PROGRAM
            || address == METEORA_PROGRAM
            || address == ORCA_WHIRLPOOL_PROGRAM
            || address == PHOENIX_PROGRAM
            || address == LIFINITY_PROGRAM
    }

    /// ⭐ FIX: Filter out system programs (ComputeBudget, Jito, etc.)
    fn is_system_program(&self, address: &str) -> bool {
        SYSTEM_PROGRAMS.contains(&address)
            || address.contains("ComputeBudget")
            || address.contains("jitono")
            || address.contains("System")
            || address.contains("Token")
            || address == "11111111111111111111111111111111"
    }

    /// ⭐ FIX: Validate that address is a valid SPL token mint
    fn is_valid_token_mint(&self, address: &str) -> bool {
        // Basic validation: must be 32-44 characters (base58 pubkey)
        if address.len() < 32 || address.len() > 44 {
            return false;
        }

        // Must not be a system program
        if self.is_system_program(address) {
            return false;
        }

        // Must be valid base58
        if let Ok(_pubkey) = Pubkey::from_str(address) {
            true
        } else {
            false
        }
    }

    fn identify_program(&self, logs: &[String]) -> String {
        for log in logs {
            if log.contains(RAYDIUM_AMM_PROGRAM) {
                return RAYDIUM_AMM_PROGRAM.to_string();
            }
            if log.contains(RAYDIUM_CLMM_PROGRAM) {
                return RAYDIUM_CLMM_PROGRAM.to_string();
            }
            if log.contains(RAYDIUM_MIGRATION_PROGRAM) {
                return RAYDIUM_MIGRATION_PROGRAM.to_string();
            }
            if log.contains(PUMPFUN_PROGRAM) {
                return PUMPFUN_PROGRAM.to_string();
            }
            if log.contains(METEORA_PROGRAM) {
                return METEORA_PROGRAM.to_string();
            }
            if log.contains(ORCA_WHIRLPOOL_PROGRAM) {
                return ORCA_WHIRLPOOL_PROGRAM.to_string();
            }
            if log.contains(PHOENIX_PROGRAM) {
                return PHOENIX_PROGRAM.to_string();
            }
            if log.contains(LIFINITY_PROGRAM) {
                return LIFINITY_PROGRAM.to_string();
            }
        }

        "Unknown".to_string()
    }

    /// Get friendly name for a program ID (for logging)
    fn get_program_name(program_id: &str) -> &str {
        match program_id {
            RAYDIUM_AMM_PROGRAM => "Raydium AMM",
            RAYDIUM_CLMM_PROGRAM => "Raydium CLMM",
            RAYDIUM_MIGRATION_PROGRAM => "Raydium Migration",
            PUMPFUN_PROGRAM => "PumpFun",
            METEORA_PROGRAM => "Meteora",
            ORCA_WHIRLPOOL_PROGRAM => "Orca Whirlpool",
            PHOENIX_PROGRAM => "Phoenix",
            LIFINITY_PROGRAM => "Lifinity",
            _ => "Unknown",
        }
    }

    /// ⭐ WATCHLIST POLLING: Retry failed extractions every 500ms for up to 15 seconds
    async fn poll_watchlist(
        watchlist: Arc<tokio::sync::RwLock<Vec<WatchlistEntry>>>,
        rpc_url: String,
        tx_sender: mpsc::UnboundedSender<NewPoolEvent>,
    ) {
        info!("🔄 Watchlist polling started (15s TTL)");
        let mut interval = tokio::time::interval(Duration::from_millis(500));

        loop {
            interval.tick().await;

            let mut watchlist_write = watchlist.write().await;
            let mut to_remove = Vec::new();

            for (idx, entry) in watchlist_write.iter_mut().enumerate() {
                // Skip if too old (15 seconds TTL for RPC indexing)
                if entry.first_seen.elapsed().as_secs() > 15 {
                    info!("⏰ WATCHLIST_EXPIRE | sig={} | retries={}",
                        &entry.signature[..12], entry.retry_count);
                    to_remove.push(idx);
                    continue;
                }

                // Try extraction again
                entry.retry_count += 1;
                let rpc_client = RpcClient::new(rpc_url.clone());

                match Signature::from_str(&entry.signature) {
                    Ok(sig) => {
                        match rpc_client.get_transaction_with_config(
                            &sig,
                            RpcTransactionConfig {
                                encoding: Some(UiTransactionEncoding::JsonParsed),
                                commitment: Some(CommitmentConfig::confirmed()),
                                max_supported_transaction_version: Some(0),
                            }
                        ) {
                            Ok(tx) => {
                                // Extract and send event
                                if let Some((pool, token)) = Self::extract_from_transaction_static(&tx, &entry.program_id) {
                                    info!("✅ WATCHLIST_SUCCESS | sig={} | retries={} | pool={}",
                                        &entry.signature[..12], entry.retry_count, &pool[..12]);

                                    let event = NewPoolEvent {
                                        pool_address: pool,
                                        token_mint: token,
                                        program_id: entry.program_id.clone(),
                                        slot: 0,
                                        signature: entry.signature.clone(),
                                    };

                                    if let Err(e) = tx_sender.send(event) {
                                        error!("Failed to send watchlist event: {:?}", e);
                                    }

                                    to_remove.push(idx);
                                } else {
                                    debug!("🔄 WATCHLIST_NO_EXTRACT | sig={} | attempt={} | tx_ok_but_extraction_failed",
                                        &entry.signature[..12], entry.retry_count);
                                }
                            }
                            Err(e) => {
                                // Still not ready, keep trying
                                if entry.retry_count % 5 == 0 {
                                    // Log every 5th attempt to avoid spam
                                    info!("🔄 WATCHLIST_RETRY | sig={} | attempt={} | error={:?}",
                                        &entry.signature[..12], entry.retry_count, e);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        to_remove.push(idx);
                    }
                }
            }

            // Remove processed/expired entries
            for &idx in to_remove.iter().rev() {
                watchlist_write.remove(idx);
            }

            // Log watchlist size periodically
            if watchlist_write.len() > 0 {
                debug!("📊 Watchlist size: {}", watchlist_write.len());
            }
        }
    }

    /// Static version of extraction for watchlist (doesn't need &self)
    /// Must match the logic of extract_pool_and_token_from_transaction exactly!
    fn extract_from_transaction_static(
        transaction: &EncodedConfirmedTransactionWithStatusMeta,
        program_name: &str,
    ) -> Option<(String, Option<String>)> {
        use solana_transaction_status::{EncodedTransaction, UiMessage};

        if let EncodedTransaction::Json(ui_tx) = &transaction.transaction.transaction {
            if let UiMessage::Parsed(parsed_msg) = &ui_tx.message {
                let account_keys: Vec<String> = parsed_msg.account_keys.iter()
                    .map(|key| key.pubkey.clone())
                    .collect();

                // Handle Raydium AMM differently (fixed indices from Quicknode guide)
                if program_name == "Raydium AMM" {
                    let pool_address = if account_keys.len() > 4 {
                        Some(account_keys[4].clone())
                    } else {
                        None
                    };

                    let token_a = if account_keys.len() > 8 {
                        Some(account_keys[8].clone())
                    } else {
                        None
                    };

                    let token_b = if account_keys.len() > 9 {
                        Some(account_keys[9].clone())
                    } else {
                        None
                    };

                    // Determine which token is NEW (not SOL/USDC)
                    let sol_mint = "So11111111111111111111111111111111111111112";
                    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

                    let token_mint = if let Some(ref ta) = token_a {
                        if ta == sol_mint || ta == usdc_mint {
                            token_b.clone()
                        } else {
                            token_a.clone()
                        }
                    } else {
                        token_b.clone()
                    };

                    if let Some(pool) = pool_address {
                        return Some((pool, token_mint));
                    }
                } else {
                    // For CLMM/Whirlpool/PumpFun/Others: Try pool state at index 4 first
                    let pool_account_index = match program_name {
                        "Orca Whirlpool" => 4,
                        "Raydium CLMM" => 4,
                        "Meteora" => 4,
                        "PumpFun" => 4,
                        "Raydium Migration" => 4,
                        "Phoenix" => 4,
                        "Lifinity" => 4,
                        _ => {
                            // Unknown program - try generic account scanning
                            // Look for first non-system, non-program account as pool
                            let mut pool_address: Option<String> = None;
                            let mut token_mint: Option<String> = None;

                            for account in account_keys.iter().skip(1).take(10) {
                                if !Self::is_system_program_static(account) && !Self::is_monitored_program_static(account) {
                                    if pool_address.is_none() {
                                        pool_address = Some(account.clone());
                                    } else if token_mint.is_none() {
                                        token_mint = Some(account.clone());
                                        break;
                                    }
                                }
                            }

                            if let Some(pool) = pool_address {
                                return Some((pool, token_mint));
                            } else {
                                return None;
                            }
                        }
                    };

                    if account_keys.len() > pool_account_index {
                        let pool = account_keys[pool_account_index].clone();

                        // Extract token mint - skip system programs AND monitored programs
                        let mut token_mint: Option<String> = None;
                        for account in account_keys.iter().skip(1).take(10) {
                            if !Self::is_system_program_static(account) && !Self::is_monitored_program_static(account) {
                                token_mint = Some(account.clone());
                                break;
                            }
                        }

                        return Some((pool, token_mint));
                    }
                }
            }
        }

        None
    }

    fn is_system_program_static(address: &str) -> bool {
        SYSTEM_PROGRAMS.contains(&address)
    }

    fn is_monitored_program_static(address: &str) -> bool {
        address == RAYDIUM_AMM_PROGRAM
            || address == RAYDIUM_CLMM_PROGRAM
            || address == RAYDIUM_MIGRATION_PROGRAM
            || address == PUMPFUN_PROGRAM
            || address == METEORA_PROGRAM
            || address == ORCA_WHIRLPOOL_PROGRAM
            || address == PHOENIX_PROGRAM
            || address == LIFINITY_PROGRAM
    }
}
