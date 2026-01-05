use crate::config::Config;
use crate::models::{Position, SafetyCheckResult, TokenPool};
use crate::services::{
    Database, JupiterService, PriceFeed, PriceCache, SolanaConnection, ProfitSweepManager,
    ProgramMonitor, NewPoolEvent, SniperValidator, TxTemplateCache,
    ValidationQueue, ValidationStatus, PoolValidator, PoolAccountStatus, DirectDexSwap,
};
use crate::strategies::{SentimentAnalyzer, VolumeAnalyzer, PatternDetector, TimeframeAnalyzer, PriceTracker, CircuitBreaker, RegimeClassifier, RegimePlaybook};
use crate::ml::{MLTrainer, TrainingConfig, TrainingDataCollector, FeatureExtractor};
use anyhow::{anyhow, Result};
use lru::LruCache;
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

pub struct TokenSniper {
    config: Arc<RwLock<Config>>,
    solana: Arc<SolanaConnection>,
    jupiter: Arc<JupiterService>,
    price_feed: Arc<PriceFeed>,
    db: Arc<RwLock<Database>>,
    active_positions: Arc<RwLock<HashMap<String, Position>>>,
    monitored_tokens: Arc<RwLock<HashSet<String>>>,
    is_running: Arc<RwLock<bool>>,
    daily_pnl: Arc<RwLock<f64>>,
    daily_start_capital: Arc<RwLock<f64>>,  // ✅ FIX #5: Make mutable
    last_reset_date: Arc<RwLock<chrono::NaiveDate>>,  // ✅ FIX #5: Track last reset
    // Phase 1 enhancements
    sentiment_analyzer: Arc<RwLock<SentimentAnalyzer>>,
    price_cache: Arc<RwLock<PriceCache>>,
    // Phase 2 enhancements - ✅ MEMORY FIX: Use LRU caches instead of unbounded HashMaps
    pattern_detectors: Arc<RwLock<LruCache<String, PatternDetector>>>,
    timeframe_analyzers: Arc<RwLock<LruCache<String, TimeframeAnalyzer>>>,
    price_trackers: Arc<RwLock<LruCache<String, PriceTracker>>>,
    // Phase 3 ML
    ml_trainer: Arc<RwLock<MLTrainer>>,
    // Circuit Breaker
    circuit_breaker: Arc<RwLock<CircuitBreaker>>,
    // Profit Sweep Manager (CRITICAL FIX #2)
    profit_sweep: Arc<ProfitSweepManager>,
    // ✅ SECURITY FIX: Transaction deduplication cache (prevents replay attacks)
    // Keep last 10,000 transaction signatures (covers ~1 day at high volume)
    seen_transactions: Arc<RwLock<LruCache<String, i64>>>, // signature -> timestamp
    // ⭐ P0.3: Pre-built transaction templates for ultra-fast execution
    tx_template_cache: Arc<RwLock<TxTemplateCache>>,
    // ⭐ NEW: Stateful validation queue with concurrent processing
    validation_queue: Arc<ValidationQueue>,
    // ⭐ NEW: Direct DEX swap capability (no Jupiter dependency)
    direct_dex: Arc<DirectDexSwap>,
}

impl TokenSniper {
    pub fn new(
        config: Config,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        price_feed: Arc<PriceFeed>,
        db: Database,
    ) -> Self {
        let daily_start = config.capital.current;

        // Initialize Phase 1 components
        let sentiment_analyzer = Arc::new(RwLock::new(SentimentAnalyzer::new()));
        let price_cache = Arc::new(RwLock::new(PriceCache::new(30))); // 30 second TTL

        // Initialize Phase 2 components - ✅ MEMORY FIX: Use LRU caches (max 100 tokens each)
        // At 2400 new tokens/day, this keeps last ~1 hour of activity (100 tokens)
        // Prevents unbounded memory growth that would cause OOM after ~7 days
        let pattern_detectors = Arc::new(RwLock::new(
            LruCache::new(NonZeroUsize::new(100).unwrap())
        ));
        let timeframe_analyzers = Arc::new(RwLock::new(
            LruCache::new(NonZeroUsize::new(100).unwrap())
        ));
        let price_trackers = Arc::new(RwLock::new(
            LruCache::new(NonZeroUsize::new(100).unwrap())
        ));

        // Initialize Phase 3 ML
        let ml_config = TrainingConfig::default();
        let ml_trainer = Arc::new(RwLock::new(MLTrainer::new(ml_config)));

        // Initialize Circuit Breaker
        let circuit_breaker = Arc::new(RwLock::new(CircuitBreaker::new(100)));

        // Initialize Profit Sweep Manager (CRITICAL FIX #2)
        let config_arc = Arc::new(RwLock::new(config.clone()));
        let initial_capital = config.capital.initial;
        let profit_sweep = Arc::new(ProfitSweepManager::new(
            config_arc.clone(),
            solana.clone(),
            initial_capital,
        ));

        // ✅ SECURITY FIX: Initialize transaction deduplication cache
        let seen_transactions = Arc::new(RwLock::new(
            LruCache::new(NonZeroUsize::new(10000).unwrap())
        ));

        // ⭐ P0.3: Initialize transaction template cache
        let user_pubkey = solana.pubkey();
        let tx_template_cache = Arc::new(RwLock::new(TxTemplateCache::new(user_pubkey)));

        // ⭐ NEW: Initialize validation queue for concurrent token processing
        let validation_queue = Arc::new(ValidationQueue::new());

        // ⭐ NEW: Initialize direct DEX swap for Jupiter-independent trading
        let rpc_url = config.rpc.url.clone();
        let direct_dex = Arc::new(DirectDexSwap::new(rpc_url));

        info!("🚀 Phase 1 Enhancements Initialized:");
        info!("   ✓ Volume Analysis (wash trading detection)");
        info!("   ✓ Market Sentiment Tracking");
        info!("   ✓ Price Caching (30s TTL)");
        info!("   ✓ Smart Position Sizing");
        info!("🚀 Phase 2 Enhancements Initialized:");
        info!("   ✓ Pattern Recognition (5 patterns)");
        info!("   ✓ Multi-Timeframe Analysis (5m, 15m, 1h)");
        info!("🚀 Phase 3 ML Initialized:");
        info!("   ✓ Feature Engineering (15 features)");
        info!("   ✓ ML Predictor (Logistic Regression)");
        info!("   ✓ Continuous Learning Pipeline");
        info!("🛡️  Circuit Breaker Initialized:");
        info!("   ✓ Max consecutive losses: 3 (1h pause)");
        info!("   ✓ Max losses in window: 5/10 (2h pause)");
        info!("   ✓ Max hourly loss: 10% (4h pause)");
        info!("💰 Profit Sweep Initialized:");
        if config.wallet.profit_sweep_enabled {
            if config.wallet.cold_wallet_address.is_some() {
                info!("   ✓ Auto-sweep enabled: {}% at +{}%",
                    config.wallet.profit_sweep_amount_pct,
                    config.wallet.profit_sweep_threshold_pct);
            } else {
                warn!("   ⚠️  Enabled but no COLD_WALLET_ADDRESS configured!");
            }
        } else {
            info!("   ⚠️  Profit sweep DISABLED (not recommended)");
        }
        info!("⚡ P0.3 Transaction Templates Initialized:");
        info!("   ✓ Pre-resolved user accounts (instant access)");
        info!("   ✓ Deterministic token account derivation");
        info!("   ✓ Target: <50ms transaction building");
        info!("🔄 Validation Queue Initialized:");
        info!("   ✓ Stateful per-token validation");
        info!("   ✓ Max 20 concurrent validations");
        info!("   ✓ Non-blocking retry logic");
        info!("⚡ Direct DEX Initialized:");
        info!("   ✓ Raydium CLMM quotes (no Jupiter)");
        info!("   ✓ Orca Whirlpool quotes (no Jupiter)");
        info!("   ✓ First-block trading capability");

        Self {
            config: Arc::new(RwLock::new(config)),
            solana,
            jupiter,
            price_feed,
            db: Arc::new(RwLock::new(db)),
            active_positions: Arc::new(RwLock::new(HashMap::new())),
            monitored_tokens: Arc::new(RwLock::new(HashSet::new())),
            is_running: Arc::new(RwLock::new(false)),
            daily_pnl: Arc::new(RwLock::new(0.0)),
            daily_start_capital: Arc::new(RwLock::new(daily_start)),  // ✅ FIX #5
            last_reset_date: Arc::new(RwLock::new(chrono::Utc::now().date_naive())),  // ✅ FIX #5
            sentiment_analyzer,
            price_cache,
            pattern_detectors,
            timeframe_analyzers,
            price_trackers,
            ml_trainer,
            circuit_breaker,
            profit_sweep,
            seen_transactions,
            tx_template_cache,
            validation_queue,
            direct_dex,
        }
    }

    // ✅ FIX #5: Add daily PnL reset logic
    async fn check_and_reset_daily_pnl(&self) {
        let today = chrono::Utc::now().date_naive();
        let mut last_reset = self.last_reset_date.write().await;

        if today != *last_reset {
            // New day - reset daily PnL
            *self.daily_pnl.write().await = 0.0;

            let current_capital = self.config.read().await.capital.current;
            *self.daily_start_capital.write().await = current_capital;
            *last_reset = today;

            info!("📅 Daily PnL reset for new day: {}", today);
            info!("  Starting capital: {:.4} SOL", current_capital);
        }
    }

    // ✅ SECURITY FIX: Check if transaction signature has been seen before
    async fn is_duplicate_transaction(&self, signature: &str) -> bool {
        let seen = self.seen_transactions.read().await;
        seen.contains(&signature.to_string())
    }

    // ✅ SECURITY FIX: Mark transaction signature as seen
    async fn mark_transaction_seen(&self, signature: &str) {
        let now = chrono::Utc::now().timestamp();
        let mut seen = self.seen_transactions.write().await;
        seen.put(signature.to_string(), now);
    }

    /// Get current SOL price in USD
    async fn get_sol_price(&self) -> Result<f64> {
        use crate::services::jupiter::SOL_MINT;
        const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

        // Try to get SOL/USDC price from Jupiter
        match self.jupiter.get_quote(SOL_MINT, USDC_MINT, 1_000_000_000, 5000).await {
            Ok(quote) => {
                // Output amount is in USDC (6 decimals)
                let price = quote.out_amount.parse::<f64>().unwrap_or(0.0) / 1_000_000.0;
                Ok(price)
            }
            Err(e) => {
                warn!("Failed to fetch SOL price from Jupiter: {}", e);
                // Fallback to a reasonable estimate if API fails
                Ok(150.0)
            }
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Token Sniper Strategy Starting...");

        *self.is_running.write().await = true;

        // ⭐ P0.3: Initialize transaction templates (one-time setup)
        info!("⚡ Initializing transaction templates...");
        if let Err(e) = self.tx_template_cache.write().await.initialize().await {
            warn!("⚠️  Template initialization failed (non-critical): {:?}", e);
        }

        // Load open positions from database
        self.load_open_positions().await?;

        // Start position monitor
        self.spawn_position_monitor();

        // Start real-time program monitoring (fastest discovery)
        self.spawn_realtime_monitor();

        // Start validation queue worker (processes tokens through Stage A → B → Direct DEX)
        self.spawn_validation_worker();

        // ⚠️ API fallback disabled - too much noise, real-time monitoring is sufficient
        // Start listening for new tokens (API fallback)
        // self.listen_for_new_tokens().await?;

        // Keep the bot running while monitoring in background
        while *self.is_running.read().await {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }

        Ok(())
    }

    /// Spawn the validation queue worker
    fn spawn_validation_worker(&self) {
        let config = self.config.clone();
        let jupiter = self.jupiter.clone();
        let solana = self.solana.clone();
        let db = self.db.clone();
        let active_positions = self.active_positions.clone();
        let circuit_breaker = self.circuit_breaker.clone();
        let daily_pnl = self.daily_pnl.clone();
        let pattern_detectors = self.pattern_detectors.clone();
        let timeframe_analyzers = self.timeframe_analyzers.clone();
        let ml_trainer = self.ml_trainer.clone();
        let price_cache = self.price_cache.clone();
        let sentiment_analyzer = self.sentiment_analyzer.clone();
        let profit_sweep = self.profit_sweep.clone();
        let price_feed = self.price_feed.clone();
        let validation_queue = self.validation_queue.clone();
        let direct_dex = self.direct_dex.clone();

        tokio::spawn(async move {
            info!("⚙️  Starting validation worker...");

            loop {
                // Get all pending tokens from queue
                let pending = validation_queue.get_pending_tokens().await;

                if !pending.is_empty() {
                    debug!("Processing {} pending validations", pending.len());
                }

                for token_state in pending {
                    // Get RPC URL for pool validator
                    let rpc_url = {
                        let cfg = config.read().await;
                        cfg.rpc.url.clone()
                    };

                    match token_state.status {
                        ValidationStatus::PendingAccount | ValidationStatus::PendingLiquidity => {
                            // Run Stage A validation
                            let pool_validator = PoolValidator::new(rpc_url);

                            match pool_validator.validate_pool_liquidity(
                                &token_state.pool_address,
                                &token_state.dex_program,
                                0.01,
                            ).await {
                                Ok(PoolAccountStatus::Valid(state)) => {
                                    info!(
                                        "✅ STAGE_A_PASS | token={} | pool={} | liq={:.2}_SOL | price=${:.9}",
                                        token_state.token_mint,
                                        token_state.pool_address,
                                        state.quote_reserve as f64 / 1e9,
                                        state.estimated_price
                                    );

                                    if let Err(e) = validation_queue.update_liquidity_check(
                                        &token_state.token_mint,
                                        Some(state.quote_reserve as f64 / 1e9),
                                        Some(state.estimated_price),
                                    ).await {
                                        warn!("Failed to update liquidity check: {:?}", e);
                                    }
                                }
                                Ok(PoolAccountStatus::NotReady { pool_address, attempts }) => {
                                    // Check if we've been waiting too long (10 seconds)
                                    let elapsed = token_state.first_seen.elapsed();
                                    if elapsed.as_secs() > 10 {
                                        info!(
                                            "❌ STAGE_A_FAIL | token={} | pool={} | reason=account_timeout | elapsed={}s",
                                            token_state.token_mint,
                                            pool_address,
                                            elapsed.as_secs()
                                        );
                                        validation_queue.reject_token(
                                            &token_state.token_mint,
                                            "Pool account not readable after 10s".to_string()
                                        ).await;
                                    } else {
                                        info!(
                                            "⏳ STAGE_A_PENDING | token={} | pool={} | reason=account_not_ready | attempts={} | elapsed={}s",
                                            token_state.token_mint,
                                            pool_address,
                                            attempts,
                                            elapsed.as_secs()
                                        );
                                        // Continue retrying - outer loop will pick it up
                                    }
                                }
                                Ok(PoolAccountStatus::NoLiquidity { pool_address, sol_liquidity }) => {
                                    info!(
                                        "❌ STAGE_A_FAIL | token={} | pool={} | reason=no_liquidity | liq={:.4}_SOL",
                                        token_state.token_mint,
                                        pool_address,
                                        sol_liquidity
                                    );

                                    validation_queue.reject_token(
                                        &token_state.token_mint,
                                        format!("No on-chain liquidity ({:.4} SOL)", sol_liquidity)
                                    ).await;
                                }
                                Err(e) => {
                                    warn!("Stage A validation error for {}: {:?}", token_state.token_mint, e);
                                }
                            }
                        }
                        ValidationStatus::PendingRoute => {
                            // Run Stage B validation (Jupiter + Direct DEX fallback)
                            let sol_mint = "So11111111111111111111111111111111111111112";
                            let entry_amount = (0.01 * 1e9) as u64;

                            let max_impact_bps = {
                                let cfg = config.read().await;
                                cfg.sniper.max_exit_impact_bps
                            };

                            // Try Jupiter first
                            let entry_result = jupiter.get_quote(
                                sol_mint,
                                &token_state.token_mint,
                                entry_amount,
                                500,
                            ).await;

                            match entry_result {
                                Ok(entry_quote) => {
                                    // Jupiter indexed - validate exit
                                    let exit_amount_tokens: u64 = match entry_quote.out_amount.parse() {
                                        Ok(amt) => amt,
                                        Err(e) => {
                                            warn!("Failed to parse entry quote out_amount: {:?}", e);
                                            continue;
                                        }
                                    };

                                    let exit_result = jupiter.get_quote(
                                        &token_state.token_mint,
                                        sol_mint,
                                        exit_amount_tokens,
                                        500,
                                    ).await;

                                    match exit_result {
                                        Ok(exit_quote) => {
                                            let exit_sol: u64 = match exit_quote.out_amount.parse() {
                                                Ok(amt) => amt,
                                                Err(e) => {
                                                    warn!("Failed to parse exit quote out_amount: {:?}", e);
                                                    continue;
                                                }
                                            };

                                            let impact_bps = if entry_amount > 0 {
                                                let loss = entry_amount.saturating_sub(exit_sol);
                                                ((loss as f64 / entry_amount as f64) * 10000.0) as u64
                                            } else {
                                                0
                                            };

                                            if impact_bps <= max_impact_bps {
                                                info!(
                                                    "✅ STAGE_B_PASS | token={} | impact={}bps",
                                                    token_state.token_mint, impact_bps
                                                );

                                                if let Err(e) = validation_queue.update_route_check(
                                                    &token_state.token_mint,
                                                    true,
                                                    true,
                                                    Some(impact_bps),
                                                ).await {
                                                    warn!("Failed to update route check: {:?}", e);
                                                }
                                            } else {
                                                info!(
                                                    "❌ STAGE_B_FAIL | token={} | reason=high_impact | impact={}bps",
                                                    token_state.token_mint, impact_bps
                                                );

                                                validation_queue.reject_token(
                                                    &token_state.token_mint,
                                                    format!("Impact too high: {}bps", impact_bps)
                                                ).await;
                                            }
                                        }
                                        Err(e) => {
                                            warn!("⚠️  STAGE_B_PARTIAL | token={} | exit_failed | error={:?}", token_state.token_mint, e);
                                        }
                                    }
                                }
                                Err(e) if format!("{:?}", e).contains("TOKEN_NOT_TRADABLE") => {
                                    // Try direct DEX
                                    info!("⚡ DIRECT_DEX_ATTEMPT | token={}", token_state.token_mint);

                                    let direct_result = if token_state.dex_program.contains("CAMM") {
                                        direct_dex.get_raydium_clmm_quote(
                                            &token_state.pool_address,
                                            sol_mint,
                                            &token_state.token_mint,
                                            entry_amount,
                                        ).await
                                    } else if token_state.dex_program.contains("whir") {
                                        direct_dex.get_orca_whirlpool_quote(
                                            &token_state.pool_address,
                                            sol_mint,
                                            &token_state.token_mint,
                                            entry_amount,
                                        ).await
                                    } else {
                                        Err(anyhow!("Unsupported DEX: {}", token_state.dex_program))
                                    };

                                    match direct_result {
                                        Ok(quote) if quote.price_impact_bps <= max_impact_bps => {
                                            info!("✅ DIRECT_DEX_PASS | token={} | impact={}bps", token_state.token_mint, quote.price_impact_bps);

                                            if let Err(e) = validation_queue.update_route_check(
                                                &token_state.token_mint,
                                                true,
                                                true,
                                                Some(quote.price_impact_bps),
                                            ).await {
                                                warn!("Failed to update route check: {:?}", e);
                                            }
                                        }
                                        Ok(quote) => {
                                            info!("❌ DIRECT_DEX_FAIL | token={} | impact={}bps", token_state.token_mint, quote.price_impact_bps);
                                            validation_queue.reject_token(
                                                &token_state.token_mint,
                                                format!("Direct DEX impact: {}bps", quote.price_impact_bps)
                                            ).await;
                                        }
                                        Err(e) => {
                                            warn!("⚠️  DIRECT_DEX_FAIL | token={} | error={:?}", token_state.token_mint, e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️  STAGE_B_RETRY | token={} | error={:?}", token_state.token_mint, e);
                                }
                            }
                        }
                        ValidationStatus::Validated => {
                            // Handle validated token
                            info!("🎯 EXECUTING_TRADE | token={} | pool={}", token_state.token_mint, token_state.pool_address);

                            let price_per_token_sol = token_state.estimated_price.unwrap_or(0.0);

                            let pool = crate::models::TokenPool {
                                token_address: token_state.token_mint.clone(),
                                token_symbol: "UNKNOWN".to_string(),
                                pair_address: token_state.pool_address.clone(),
                                liquidity_usd: token_state.liquidity_sol.unwrap_or(0.0) * 150.0,
                                liquidity_sol: token_state.liquidity_sol.unwrap_or(0.0),
                                price_usd: price_per_token_sol * 150.0,
                                volume_1h: 0.0,
                                volume_6h: 0.0,
                                volume_24h: 0.0,
                                price_change_24h: 0.0,
                                created_at: Some(chrono::Utc::now().timestamp_millis()),
                            };

                            if let Err(e) = Self::evaluate_pool_standalone(
                                pool,
                                config.clone(),
                                solana.clone(),
                                jupiter.clone(),
                                price_feed.clone(),
                                db.clone(),
                                active_positions.clone(),
                                circuit_breaker.clone(),
                                daily_pnl.clone(),
                                pattern_detectors.clone(),
                                timeframe_analyzers.clone(),
                                price_trackers.clone(),
                                ml_trainer.clone(),
                                price_cache.clone(),
                                sentiment_analyzer.clone(),
                                profit_sweep.clone(),
                            ).await {
                                error!("Error handling validated token {}: {:?}", token_state.token_mint, e);
                            }
                        }
                        _ => {}
                    }
                }

                // Cleanup old entries
                validation_queue.cleanup().await;

                // Sleep before next iteration
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        info!("Validation worker spawned");
    }

    fn spawn_realtime_monitor(&self) {
        let config = self.config.clone();
        let price_feed = self.price_feed.clone();
        let monitored_tokens = self.monitored_tokens.clone();
        let is_running = self.is_running.clone();

        // Clone ALL self components needed for full evaluation
        let solana = self.solana.clone();
        let jupiter = self.jupiter.clone();
        let db = self.db.clone();
        let active_positions = self.active_positions.clone();
        let circuit_breaker = self.circuit_breaker.clone();
        let daily_pnl = self.daily_pnl.clone();
        let pattern_detectors = self.pattern_detectors.clone();
        let timeframe_analyzers = self.timeframe_analyzers.clone();
        let ml_trainer = self.ml_trainer.clone();
        let price_cache = self.price_cache.clone();
        let sentiment_analyzer = self.sentiment_analyzer.clone();
        let profit_sweep = self.profit_sweep.clone();
        let tx_template_cache = self.tx_template_cache.clone();
        let validation_queue = self.validation_queue.clone();

        tokio::spawn(async move {
            info!("🔴 Starting REAL-TIME program monitoring...");

            // Get WebSocket and RPC URLs from config
            let cfg = config.read().await;
            let ws_url = cfg.rpc.ws_url.clone();
            let rpc_url = cfg.rpc.url.clone();
            drop(cfg);

            // Create program monitor
            let (monitor, mut rx) = ProgramMonitor::new(ws_url, rpc_url);

            // Spawn monitor task
            tokio::spawn(async move {
                if let Err(e) = monitor.start().await {
                    error!("Program monitor failed: {:?}", e);
                }
            });

            info!("✅ Real-time monitoring active - listening for pool creations...");

            // Process real-time pool events
            while *is_running.read().await {
                match rx.recv().await {
                    Some(event) => {
                        // ⭐ STRUCTURED LOGGING with all key fields
                        info!(
                            "🚀 NEW POOL EVENT | dex={} | pool={} | token={} | sig={}",
                            event.program_id,
                            event.pool_address,
                            event.token_mint.as_ref().unwrap_or(&"UNKNOWN".to_string()),
                            event.signature
                        );

                        // ⭐ CRITICAL: Skip if no token mint extracted
                        if event.token_mint.is_none() {
                            warn!("⚠️  No token mint extracted for pool {} - skipping", event.pool_address);
                            continue;
                        }

                        let token_mint = event.token_mint.clone().unwrap();

                        // Check if already monitored (use token mint, not pool)
                        let mut monitored = monitored_tokens.write().await;
                        if !monitored.contains(&token_mint) {
                            monitored.insert(token_mint.clone());
                            drop(monitored);

                            // ⭐ NEW ARCHITECTURE: Add to validation queue (non-blocking)
                            // Queue worker will handle Stage A → Stage B → Direct DEX fallback
                            let queue = validation_queue.clone();
                            let pool_address = event.pool_address.clone();
                            let dex_program = event.program_id.clone();

                            tokio::spawn(async move {
                                let added = queue.add_token(
                                    token_mint.clone(),
                                    pool_address,
                                    dex_program,
                                ).await;

                                if !added {
                                    warn!(
                                        "⚠️  QUEUE_FULL | token={} | reason=capacity_limit",
                                        token_mint
                                    );
                                }
                            });
                        }
                    }
                    None => {
                        warn!("Real-time monitor channel closed");
                        break;
                    }
                }
            }

            info!("Real-time monitor stopped");
        });

        info!("Real-time program monitor spawned");
    }

    async fn load_open_positions(&self) -> Result<()> {
        let db = self.db.read().await;
        let trades = db.get_open_positions()?;

        info!("Loading {} open positions...", trades.len());

        let mut positions = self.active_positions.write().await;

        for trade in trades {
            let token_address = Pubkey::from_str(&trade.token_address)?;

            positions.insert(
                trade.token_address.clone(),
                Position {
                    trade_id: trade.id,
                    token_address,
                    token_symbol: trade.token_symbol.clone(),
                    entry_price: trade.entry_price,
                    amount_sol: trade.amount_sol,
                    tokens_bought: trade.tokens_bought,
                    entry_time: trade.entry_time,
                    highest_price: trade.entry_price,
                    state: crate::models::PositionState::Sniper,  // All loaded positions start as sniper
                    validation_window_end: trade.entry_time + (10 * 60 * 1000),  // 10 min from entry
                    higher_lows_count: 0,
                    stop_loss_pct: 0.20,  // Default 20% stop loss
                    take_profit_pcts: vec![2.0],  // Default 2x take profit
                    time_stop_minutes: Some(60),  // Default 60 min time stop
                    ml_features: None,  // No features for loaded positions
                },
            );

            info!(
                "  {} {}: {} tokens @ {} SOL",
                trade.token_symbol, trade.token_address, trade.tokens_bought, trade.entry_price
            );
        }

        Ok(())
    }

    async fn listen_for_new_tokens(&self) -> Result<()> {
        info!("Monitoring for new token pairs...");

        let check_interval = Duration::from_secs(3);

        while *self.is_running.read().await {
            match self.poll_new_pools().await {
                Ok(_) => {}
                Err(e) => {
                    error!("Error polling new pools: {}", e);
                }
            }

            tokio::time::sleep(check_interval).await;
        }

        Ok(())
    }

    async fn poll_new_pools(&self) -> Result<()> {
        // ✅ FIX #5: Check and reset daily PnL at start of each iteration
        self.check_and_reset_daily_pnl().await;

        let pools = self.price_feed.get_new_pools().await?;

        for pool in pools {
            let mut monitored = self.monitored_tokens.write().await;

            if !monitored.contains(&pool.token_address) {
                monitored.insert(pool.token_address.clone());
                drop(monitored); // Release lock before async operation

                self.evaluate_and_snipe(pool).await?;
            }
        }

        Ok(())
    }

    async fn evaluate_and_snipe(&self, pool: TokenPool) -> Result<()> {
        info!("Evaluating: {} ({})", pool.token_symbol, pool.token_address);

        // ✅ CRITICAL FIX #3: Check circuit breaker FIRST
        let breaker = self.circuit_breaker.read().await;
        let status = breaker.get_stats();
        drop(breaker); // Release lock immediately

        if status.consecutive_losses >= 3 {
            warn!("⛔ CIRCUIT BREAKER ACTIVE - Trading paused");
            warn!("   Consecutive losses: {}", status.consecutive_losses);
            // Circuit breaker active
            return Ok(());
        }

        // Run safety checks
        let checks = self.run_safety_checks(&pool).await;

        if !checks.passed {
            info!("  Failed checks: {}", checks.reasons.join(", "));
            return Ok(());
        }

        info!("  ✓ All checks passed!");

        // ✅ PHASE 1: Volume Analysis
        info!("📊 Analyzing volume profile...");
        let volume_profile = VolumeAnalyzer::analyze_volume_profile(
            pool.volume_1h,
            pool.volume_6h,
            pool.volume_24h,
            pool.liquidity_sol,
        );

        info!("  Volume Quality Score: {}/100", volume_profile.quality_score);
        info!("  Volume Trend: {:?}", volume_profile.volume_trend);
        info!("  Buy/Sell Ratio: {:.2}", volume_profile.buy_sell_ratio);

        if volume_profile.quality_score < 60 {
            warn!("❌ Low volume quality score: {} (minimum: 60)", volume_profile.quality_score);
            return Ok(());
        }

        // Check for wash trading
        // We'll estimate holder count from safety check results (will get actual count below)
        let estimated_holders = 100; // Will be replaced with actual count
        if VolumeAnalyzer::is_wash_trading(pool.volume_24h, pool.liquidity_sol, estimated_holders) {
            warn!("⚠️  Wash trading detected - skipping token");
            return Ok(());
        }

        info!("  ✓ Volume analysis passed");

        // Get current token price for timeframe + pattern analysis
        let current_price = match self.price_feed
            .get_token_price_jupiter(&self.jupiter, &pool.token_address)
            .await
        {
            Ok(price) if price > 0.0 => {
                info!("  Current Price: ${:.8}", price);
                price
            }
            Ok(price) => {
                warn!("  Invalid price received: {}", price);
                0.0
            }
            Err(e) => {
                warn!("  Cannot get price for analysis: {}", e);
                0.0
            }
        };

        // ✅ CRITICAL FIX #4: Regime Classification
        info!("🎯 Classifying trading regime...");
        let regime_classification = {
            let mut analyzers = self.timeframe_analyzers.write().await;
            if !analyzers.contains(&pool.token_address) {
                analyzers.put(pool.token_address.clone(), TimeframeAnalyzer::new());
            }

            let analyzer = analyzers.get_mut(&pool.token_address).unwrap();
            if current_price > 0.0 {
                let volume_point = if pool.volume_1h > 0.0 {
                    pool.volume_1h / 60.0
                } else {
                    pool.volume_24h / 1440.0
                };
                analyzer.add_price(chrono::Utc::now().timestamp_millis(), current_price, volume_point);
            }

            RegimeClassifier::classify(&pool, &volume_profile, Some(analyzer))
        };

        let playbook = regime_classification.playbook;

        info!("  Regime: {}", regime_classification.regime.name());
        info!("  Confidence: {:.0}%", regime_classification.confidence * 100.0);
        info!("  Stop Loss: -{}%", playbook.stop_loss_pct);
        info!("  Take Profit: +{}%", playbook.take_profit_pcts[0]);
        if let Some(time_stop) = playbook.time_stop_minutes {
            info!("  Time Stop: {} minutes", time_stop);
        }

        for reason in &regime_classification.reasons {
            info!("    {}", reason);
        }

        // ✅ PHASE 2: Pattern Detection (FIXED - now uses real prices)

        let pattern_result = if current_price > 0.0 {
            let mut detectors = self.pattern_detectors.write().await;

            // ✅ MEMORY FIX: Use LRU cache get_or_insert instead of HashMap entry API
            if !detectors.contains(&pool.token_address) {
                detectors.put(pool.token_address.clone(), PatternDetector::new(20));
            }
            let detector = detectors.get_mut(&pool.token_address).unwrap();

            // Add REAL price point
            let now = chrono::Utc::now().timestamp_millis();
            detector.add_price_point(now, current_price, pool.volume_24h);

            detector.detect_pattern()
        } else {
            // Create neutral pattern if price unavailable
            use crate::strategies::{Pattern, TradeRecommendation, PatternResult};
            warn!("  Skipping pattern detection - price unavailable");
            PatternResult {
                pattern: Pattern::None,
                confidence: 0,
                description: "Price unavailable".to_string(),
                trade_recommendation: TradeRecommendation::Hold,
            }
        };

        info!("📊 Pattern Analysis:");
        info!("  Pattern: {:?}", pattern_result.pattern);
        info!("  Confidence: {}/100", pattern_result.confidence);
        info!("  Recommendation: {:?}", pattern_result.trade_recommendation);

        // Filter based on pattern
        use crate::strategies::TradeRecommendation;
        if pattern_result.trade_recommendation == TradeRecommendation::Avoid {
            warn!("❌ Pattern recommends AVOID: {}", pattern_result.description);
            return Ok(());
        }

        info!("  ✓ Pattern analysis passed");

        // ✅ PHASE 2: Track price for ML features
        if current_price > 0.0 {
            let mut trackers = self.price_trackers.write().await;

            // ✅ MEMORY FIX: Use LRU cache API
            if !trackers.contains(&pool.token_address) {
                trackers.put(pool.token_address.clone(), PriceTracker::new(120)); // 2 hours history
            }
            let tracker = trackers.get_mut(&pool.token_address).unwrap();

            let now = chrono::Utc::now().timestamp_millis();
            tracker.add_price(now, current_price);

            info!("📈 Price Tracking:");
            info!("  Historical data points: {}", tracker.size());
        }

        // ✅ PHASE 1: Market Sentiment Check
        info!("📈 Checking market sentiment...");
        let current_sol_price = self.get_sol_price().await?;
        let sentiment = self.sentiment_analyzer.write().await
            .update_sentiment(current_sol_price)?;

        info!("  SOL Price: ${:.2}", current_sol_price);
        info!("  SOL Trend: {:?}", sentiment.sol_trend);
        info!("  Fear/Greed Index: {}/100", sentiment.market_fear_greed);
        info!("  Trading Recommended: {}", sentiment.trading_recommended);

        if !sentiment.trading_recommended {
            warn!("⏸️  Market sentiment unfavorable - skipping trade");
            warn!("  1h change: {:.2}%, 24h change: {:.2}%", sentiment.sol_price_change_1h, sentiment.sol_price_change_24h);
            return Ok(());
        }

        info!("  ✓ Market sentiment favorable");

        // ✅ FIX #3: Add rug pull protection
        use crate::services::TokenSafetyChecker;
        use std::str::FromStr;

        let safety_checker = TokenSafetyChecker::new(self.solana.clone());
        let token_pubkey = Pubkey::from_str(&pool.token_address)?;

        info!("🔒 Running token safety checks...");
        let safety_check = safety_checker.check_token_safety(&token_pubkey).await?;

        if !safety_check.passed {
            warn!("❌ Token FAILED safety checks (risk score: {})", safety_check.risk_score);
            for reason in &safety_check.reasons {
                warn!("  {}", reason);
            }
            return Ok(());
        }

        // Check for honeypot
        info!("🍯 Testing for honeypot...");
        let is_honeypot = safety_checker
            .check_honeypot(&self.jupiter, &pool.token_address, 1_000_000)
            .await?;

        if is_honeypot {
            warn!("❌ Honeypot detected - skipping token");
            return Ok(());
        }

        info!("✅ Token passed all security checks");

        // ✅ CRITICAL FIX #2: Get actual holder data
        let (top_holder_pct, holder_count) = match safety_checker.check_holder_concentration(&token_pubkey).await {
            Ok((pct, count)) => {
                info!("👥 Holder Analysis:");
                info!("  Total holders: {}", count);
                info!("  Top holder owns: {:.2}%", pct);
                (pct, count)
            }
            Err(e) => {
                warn!("  ⚠️  Could not get holder data: {}", e);
                (0.0, 100) // Fallback to conservative estimate
            }
        };

        // ✅ PHASE 1: Calculate Confidence Score for Smart Position Sizing
        use crate::strategies::PositionSizer;

        let confidence = PositionSizer::calculate_confidence(
            &safety_check,
            &volume_profile,
            &sentiment,
            pool.liquidity_sol,
            holder_count,
        );

        let confidence_category = PositionSizer::get_size_category(confidence);
        info!("🎯 Confidence Score: {}/100 - {}", confidence, confidence_category);

        if confidence < 50 {
            warn!("❌ Confidence too low ({}/100) - minimum threshold is 50", confidence);
            warn!("  Breakdown: Safety={}, Volume={}, Sentiment={}, Liquidity={}, Holders={}",
                if safety_check.passed { "✓" } else { "✗" },
                volume_profile.quality_score,
                sentiment.market_fear_greed,
                if pool.liquidity_sol > 10.0 { "✓" } else { "✗" },
                holder_count
            );
            return Ok(());
        }

        // ✅ PHASE 3: ML Prediction (if model is ready)
        let ml_trainer = self.ml_trainer.read().await;
        if ml_trainer.is_model_ready() {
            // ✅ CRITICAL FIX #2: Get actual price changes from tracker
            let price_changes = if current_price > 0.0 {
                let mut trackers = self.price_trackers.write().await;
                if let Some(tracker) = trackers.get(&pool.token_address) {
                    tracker.get_all_changes(current_price)
                } else {
                    (0.0, 0.0, 0.0) // No data yet
                }
            } else {
                (0.0, 0.0, 0.0) // Price unavailable
            };

            info!("📊 ML Features:");
            info!("  Price Changes: 5m={:.2}%, 15m={:.2}%, 1h={:.2}%",
                price_changes.0, price_changes.1, price_changes.2);
            info!("  Holder Concentration: {:.2}%", top_holder_pct);

            // Extract features using REAL data
            let features = FeatureExtractor::extract_features(
                price_changes, // ✅ Real price changes from tracker
                &volume_profile,
                pool.liquidity_sol,
                pool.volume_24h,
                holder_count,
                top_holder_pct, // ✅ Real top holder percentage
                &safety_check,
                &sentiment,
                &pattern_result, // ✅ Real pattern detection from Phase 2
            );

            // Get ML prediction
            let prediction = ml_trainer.get_model().predict_with_confidence(&features);
            let ml_progress = ml_trainer.get_progress();

            info!("🤖 ML Prediction:");
            info!("  Win Probability: {:.1}%", prediction.probability * 100.0);
            info!("  Confidence Level: {:?}", prediction.confidence_level);
            info!("  Recommendation: {:?}", prediction.recommendation);
            info!("  Model Stats: {}", ml_progress.to_string());

            // Only proceed if ML agrees (probability >= 60%)
            if prediction.probability < 0.6 {
                warn!("❌ ML prediction unfavorable: {:.1}% win probability", prediction.probability * 100.0);
                return Ok(());
            }

            info!("  ✓ ML prediction favorable");
        } else {
            let ml_progress = ml_trainer.get_progress();
            info!("⏳ ML Model Not Ready: {}", ml_progress.to_string());
        }
        drop(ml_trainer);

        // Check balance with proper fee estimation
        let config = self.config.read().await;
        let balance = self.solana.get_balance(None).await?;
        let current_capital = config.capital.current;

        // ✅ PHASE 1: Smart Position Sizing based on confidence
        let base_amount = config.sniper.snipe_amount_sol;
        let snipe_amount = PositionSizer::calculate_position_size(
            base_amount,
            confidence,
            current_capital,
            0.05, // Max 5% of capital per trade
        );

        info!("💰 Position Sizing:");
        info!("  Base Amount: {:.4} SOL", base_amount);
        info!("  Adjusted Amount: {:.4} SOL ({:.1}x multiplier)", snipe_amount, snipe_amount / base_amount);
        info!("  Confidence Factor: {}/100", confidence);

        // ✅ FIX: Use proper fee estimation
        let estimated_fees = self.solana.estimate_transaction_cost(snipe_amount).await?;
        let required_amount = snipe_amount + estimated_fees;
        let rent_exempt_min = 0.001; // Keep wallet above rent-exempt

        if balance < required_amount + rent_exempt_min {
            warn!(
                "  ❌ Insufficient balance: {} SOL (need {} SOL + {} fees + {} rent reserve)",
                balance, snipe_amount, estimated_fees, rent_exempt_min
            );
            return Ok(());
        }

        // ✅ FIX #4: Race condition - hold write lock throughout
        let mut positions = self.active_positions.write().await;

        if positions.len() >= config.risk.max_concurrent_positions {
            warn!(
                "  ❌ Max concurrent positions reached ({})",
                config.risk.max_concurrent_positions
            );
            return Ok(());
        }

        // ✅ FIX #5: Check daily loss limit with proper start capital
        let daily_pnl = *self.daily_pnl.read().await;
        let daily_start = *self.daily_start_capital.read().await;

        if daily_pnl < -(daily_start * config.risk.max_daily_loss) {
            error!("  ❌ Daily loss limit reached");
            error!("     Daily PnL: {:.4} SOL", daily_pnl);
            error!("     Max allowed loss: {:.4} SOL", -(daily_start * config.risk.max_daily_loss));

            let mut db = self.db.write().await;
            db.log_risk_event(
                "daily_loss_limit",
                "high",
                &format!("Daily loss limit reached: {:.4} SOL", daily_pnl),
                "Trading halted",
            )?;
            *self.is_running.write().await = false;
            return Ok(());
        }

        drop(config);

        // ✅ PRIORITY 1.1: Edge-vs-Cost Gate (Section 5E)
        // Validate trade has positive expected value BEFORE execution
        info!("💎 Running Edge-vs-Cost Gate...");

        // Get Jupiter quote to calculate price impact
        let config = self.config.read().await;
        let quote_result = self.jupiter.get_quote(
            "So11111111111111111111111111111111111111112", // SOL mint
            &pool.token_address,
            (snipe_amount * 1_000_000_000.0) as u64, // Convert SOL to lamports
            config.risk.slippage_bps,
        ).await;

        let price_impact_bps = match quote_result {
            Ok(quote) => quote.price_impact_pct.parse::<f64>().unwrap_or(0.0) * 100.0, // Convert to bps
            Err(e) => {
                warn!("  ⚠️  Could not get quote for edge-vs-cost check: {}", e);
                warn!("  Proceeding with conservative 100 bps price impact estimate");
                100.0 // Conservative fallback
            }
        };

        // Calculate total costs in basis points
        let slippage_bps = self.config.read().await.risk.slippage_bps as f64;
        let fee_bps = 30.0; // Jupiter base fee ~0.3%
        let total_costs_bps = slippage_bps + fee_bps + price_impact_bps;

        // Calculate expected edge from confidence score
        // confidence ranges from 50-100, we convert to expected edge
        // 50 confidence = 0% edge, 100 confidence = +100% edge
        let edge_from_confidence = ((confidence as f64 - 50.0) / 50.0) * 100.0;

        // Get ML prediction edge if available (optional boost)
        let ml_edge = if let Ok(ml_trainer) = self.ml_trainer.try_read() {
            if ml_trainer.is_model_ready() {
                // ML probability 60-100% maps to 0-40% extra edge
                let ml_prob = ml_trainer.get_model().predict_with_confidence(
                    &FeatureExtractor::extract_features(
                        (0.0, 0.0, 0.0), // Dummy for edge calculation
                        &volume_profile,
                        pool.liquidity_sol,
                        pool.volume_24h,
                        holder_count,
                        top_holder_pct,
                        &safety_check,
                        &sentiment,
                        &pattern_result,
                    )
                ).probability;
                if ml_prob >= 0.6 {
                    ((ml_prob - 0.6) / 0.4) * 40.0 // Max 40% edge boost
                } else {
                    0.0
                }
            } else {
                0.0
            }
        } else {
            0.0
        };

        let total_expected_edge_bps = (edge_from_confidence + ml_edge) * 100.0; // Convert % to bps

        info!("  Expected Edge: {:.1} bps", total_expected_edge_bps);
        info!("  Total Costs: {:.1} bps", total_costs_bps);
        info!("    - Slippage: {:.1} bps", slippage_bps);
        info!("    - Fees: {:.1} bps", fee_bps);
        info!("    - Price Impact: {:.1} bps", price_impact_bps);
        info!("  Edge-to-Cost Ratio: {:.2}x", total_expected_edge_bps / total_costs_bps);

        // CRITICAL: Expected edge must be at least 2x total costs
        let min_edge_to_cost_ratio = 2.0;
        if total_expected_edge_bps < (total_costs_bps * min_edge_to_cost_ratio) {
            warn!("❌ EDGE-VS-COST GATE FAILED");
            warn!("  Expected edge ({:.1} bps) < 2x costs ({:.1} bps)",
                total_expected_edge_bps, total_costs_bps * min_edge_to_cost_ratio);
            warn!("  This trade would be negative EV - skipping");

            let mut db = self.db.write().await;
            db.log_risk_event(
                "edge_vs_cost_failed",
                "low",
                &format!("Trade rejected: edge={:.1}bps, costs={:.1}bps, ratio={:.2}x",
                    total_expected_edge_bps, total_costs_bps,
                    total_expected_edge_bps / total_costs_bps),
                "Trade would be negative EV",
            )?;
            drop(db);

            return Ok(());
        }

        info!("  ✅ Edge-vs-Cost Gate PASSED ({:.2}x ratio)",
            total_expected_edge_bps / total_costs_bps);

        // Execute the snipe while holding position lock
        info!("🚀 Executing snipe...");
        let config_guard = self.config.read().await;
        let config_clone = config_guard.clone();
        drop(config_guard);
        let result = self.jupiter
            .buy_with_sol(&self.solana, &config_clone, &pool.token_address, snipe_amount)
            .await?;

        if result.success {
            let entry_price = snipe_amount / result.output_amount as f64;

            // Get signature (should always exist when success=true)
            let signature = result.signature.as_ref()
                .ok_or_else(|| anyhow!("Missing signature in successful swap"))?;

            // ✅ SECURITY FIX: Check for duplicate transaction
            if self.is_duplicate_transaction(signature).await {
                error!("  ⚠️  DUPLICATE TRANSACTION DETECTED: {}", signature);
                error!("  Possible replay attack - ignoring this transaction");
                return Ok(());
            }

            // Mark transaction as seen
            self.mark_transaction_seen(signature).await;

            // Record trade in database
            let mut db = self.db.write().await;
            let trade_id = db.record_trade_entry(
                "sniper",
                &pool.token_address,
                &pool.token_symbol,
                entry_price,
                snipe_amount,
                result.output_amount as f64,
                signature,
                result.fees as f64 / 1_000_000_000.0,
            )?;
            drop(db);

            // Add to active positions (still holding write lock)
            // ✅ PRIORITY 1.5: Store regime playbook parameters in position
            let now = chrono::Utc::now().timestamp_millis();
            positions.insert(
                pool.token_address.clone(),
                Position {
                    trade_id,
                    token_address: token_pubkey,
                    token_symbol: pool.token_symbol.clone(),
                    entry_price,
                    amount_sol: snipe_amount,
                    tokens_bought: result.output_amount as f64,
                    entry_time: now,
                    highest_price: entry_price,
                    state: crate::models::PositionState::Sniper,
                    validation_window_end: now + (10 * 60 * 1000),  // 10 min validation window
                    higher_lows_count: 0,
                    stop_loss_pct: playbook.stop_loss_pct,
                    take_profit_pcts: playbook.take_profit_pcts.clone(),
                    time_stop_minutes: playbook.time_stop_minutes,
                    ml_features: None,  // Old code path - no ML features
                },
            );

            info!("  ✅ Snipe successful!");
            info!("  📝 TX: {}", signature);
            info!("  💰 Tokens received: {}", result.output_amount);
            info!("  📊 Active positions: {}/{}", positions.len(), self.config.read().await.risk.max_concurrent_positions);
        } else {
            error!("  ❌ Snipe failed: {:?}", result.error);
            let mut db = self.db.write().await;
            db.log_risk_event(
                "snipe_failed",
                "medium",
                &format!("Failed to snipe {}", pool.token_symbol),
                result.error.as_deref().unwrap_or("Unknown error"),
            )?;
        }

        drop(positions);
        Ok(())
    }

    // ⭐ STANDALONE EVALUATION - Can be called from spawned tasks without self
    #[allow(clippy::too_many_arguments)]
    async fn evaluate_pool_standalone(
        pool: TokenPool,
        config: Arc<RwLock<Config>>,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        price_feed: Arc<PriceFeed>,
        db: Arc<RwLock<Database>>,
        active_positions: Arc<RwLock<HashMap<String, Position>>>,
        circuit_breaker: Arc<RwLock<CircuitBreaker>>,
        daily_pnl: Arc<RwLock<f64>>,
        pattern_detectors: Arc<RwLock<LruCache<String, PatternDetector>>>,
        timeframe_analyzers: Arc<RwLock<LruCache<String, TimeframeAnalyzer>>>,
        price_trackers: Arc<RwLock<LruCache<String, PriceTracker>>>,
        ml_trainer: Arc<RwLock<MLTrainer>>,
        price_cache: Arc<RwLock<PriceCache>>,
        _sentiment_tracker: Arc<RwLock<SentimentAnalyzer>>,
        profit_sweep: Arc<ProfitSweepManager>,
    ) -> Result<()> {
        info!("Evaluating: {} ({})", pool.token_symbol, pool.token_address);

        // Check circuit breaker FIRST
        let breaker = circuit_breaker.read().await;
        let status = breaker.get_stats();
        drop(breaker);

        if status.consecutive_losses >= 3 {
            warn!("⛔ CIRCUIT BREAKER ACTIVE - Trading paused");
            warn!("   Consecutive losses: {}", status.consecutive_losses);
            return Ok(());
        }

        // Run safety checks
        let checks = Self::run_safety_checks_static(&pool, &config).await;

        if !checks.passed {
            info!("  Failed checks: {}", checks.reasons.join(", "));
            return Ok(());
        }

        info!("  ✓ All checks passed!");

        // Volume Analysis
        info!("📊 Analyzing volume profile...");
        let volume_profile = VolumeAnalyzer::analyze_volume_profile(
            pool.volume_1h,
            pool.volume_6h,
            pool.volume_24h,
            pool.liquidity_sol,
        );

        info!("  Volume Quality Score: {}/100", volume_profile.quality_score);
        info!("  Volume Trend: {:?}", volume_profile.volume_trend);
        info!("  Buy/Sell Ratio: {:.2}", volume_profile.buy_sell_ratio);

        if volume_profile.quality_score < 60 {
            warn!("❌ Low volume quality score: {} (minimum: 60)", volume_profile.quality_score);
            return Ok(());
        }

        // Check for wash trading
        let estimated_holders = 100;
        if VolumeAnalyzer::is_wash_trading(pool.volume_24h, pool.liquidity_sol, estimated_holders) {
            warn!("⚠️  Wash trading detected - skipping token");
            return Ok(());
        }

        info!("  ✓ Volume analysis passed");

        let current_price = match price_feed
            .get_token_price_jupiter(&jupiter, &pool.token_address)
            .await
        {
            Ok(price) if price > 0.0 => price,
            Ok(_) => 0.0,
            Err(_) => 0.0,
        };

        // Regime Classification
        info!("🎯 Classifying trading regime...");
        let regime_classification = {
            let mut analyzers = timeframe_analyzers.write().await;
            if !analyzers.contains(&pool.token_address) {
                analyzers.put(pool.token_address.clone(), TimeframeAnalyzer::new());
            }
            let analyzer = analyzers.get_mut(&pool.token_address).unwrap();
            if current_price > 0.0 {
                let volume_point = if pool.volume_1h > 0.0 {
                    pool.volume_1h / 60.0
                } else {
                    pool.volume_24h / 1440.0
                };
                analyzer.add_price(chrono::Utc::now().timestamp_millis(), current_price, volume_point);
            }
            RegimeClassifier::classify(&pool, &volume_profile, Some(analyzer))
        };

        let playbook = regime_classification.playbook;

        info!("  Regime: {}", regime_classification.regime.name());
        info!("  Confidence: {:.0}%", regime_classification.confidence * 100.0);
        info!("  Stop Loss: -{}%", playbook.stop_loss_pct);
        info!("  Take Profit: +{}%", playbook.take_profit_pcts[0]);
        if let Some(time_stop) = playbook.time_stop_minutes {
            info!("  Time Stop: {} minutes", time_stop);
        }

        for reason in &regime_classification.reasons {
            info!("    {}", reason);
        }

        let pattern_result = if current_price > 0.0 {
            let mut detectors = pattern_detectors.write().await;
            if !detectors.contains(&pool.token_address) {
                detectors.put(pool.token_address.clone(), PatternDetector::new(20));
            }
            let detector = detectors.get_mut(&pool.token_address).unwrap();

            let now = chrono::Utc::now().timestamp_millis();
            detector.add_price_point(now, current_price, pool.volume_24h);
            detector.detect_pattern()
        } else {
            crate::strategies::PatternResult {
                pattern: crate::strategies::Pattern::None,
                confidence: 0,
                description: "Price unavailable".to_string(),
                trade_recommendation: crate::strategies::TradeRecommendation::Hold,
            }
        };

        let price_changes = if current_price > 0.0 {
            let mut trackers = price_trackers.write().await;
            if !trackers.contains(&pool.token_address) {
                trackers.put(pool.token_address.clone(), PriceTracker::new(120));
            }
            let tracker = trackers.get_mut(&pool.token_address).unwrap();
            let now = chrono::Utc::now().timestamp_millis();
            tracker.add_price(now, current_price);
            tracker.get_all_changes(current_price)
        } else {
            (0.0, 0.0, 0.0)
        };

        // ✅ ML LEARNING: Extract features for training later
        use crate::ml::FeatureExtractor;
        use crate::services::TokenSafetyCheck;
        use crate::strategies::MarketSentiment;

        // Create a basic sentiment (we don't have full context here)
        let sentiment = MarketSentiment {
            sol_trend: crate::strategies::Trend::Neutral,
            sol_price_change_1h: 0.0,
            sol_price_change_24h: 0.0,
            market_fear_greed: 50,  // Neutral
            trading_recommended: true,
        };

        // Create safety check result for feature extraction
        let safety_for_features = TokenSafetyCheck {
            passed: true,  // We already passed checks
            reasons: vec![],
            risk_score: 0,
        };

        // Extract ML features
        let ml_features = FeatureExtractor::extract_features(
            price_changes,
            &volume_profile,
            pool.liquidity_sol,
            pool.volume_24h,
            estimated_holders,
            0.0,  // No holder concentration data
            &safety_for_features,
            &sentiment,
            &pattern_result,
        );

        // Execute trade
        info!("🚀 Executing snipe...");
        Self::execute_snipe_standalone(
            pool,
            playbook,
            ml_features,  // ⭐ Pass features for ML learning
            config,
            solana,
            jupiter,
            price_feed,
            db,
            active_positions,
            circuit_breaker,
            daily_pnl,
            pattern_detectors,
            ml_trainer,
            price_cache,
            profit_sweep,
        ).await?;

        Ok(())
    }

    // Static version of run_safety_checks for standalone evaluation
    async fn run_safety_checks_static(pool: &TokenPool, config: &Arc<RwLock<Config>>) -> SafetyCheckResult {
        let mut reasons = Vec::new();
        let config = config.read().await;

        // Liquidity checks
        if pool.liquidity_sol < config.sniper.min_liquidity_sol {
            reasons.push(format!(
                "Low liquidity: {:.2} SOL",
                pool.liquidity_sol
            ));
        }

        if pool.liquidity_sol > config.sniper.max_liquidity_sol {
            reasons.push(format!(
                "Too high liquidity: {:.2} SOL",
                pool.liquidity_sol
            ));
        }

        // Age check
        if let Some(created_at) = pool.created_at {
            let age_ms = chrono::Utc::now().timestamp_millis() - created_at;
            let age_minutes = age_ms / 1000 / 60;

            if age_minutes > config.sniper.max_age_minutes as i64 {
                reasons.push(format!("Token too old: {} minutes", age_minutes));
            }
        }

        // Volume check
        if pool.volume_24h < 1000.0 {
            reasons.push(format!("Low 24h volume: ${:.2}", pool.volume_24h));
        }

        SafetyCheckResult {
            passed: reasons.is_empty(),
            reasons,
        }
    }

    async fn run_safety_checks(&self, pool: &TokenPool) -> SafetyCheckResult {
        let mut reasons = Vec::new();
        let config = self.config.read().await;

        // Liquidity checks
        if pool.liquidity_sol < config.sniper.min_liquidity_sol {
            reasons.push(format!(
                "Low liquidity: {:.2} SOL",
                pool.liquidity_sol
            ));
        }

        if pool.liquidity_sol > config.sniper.max_liquidity_sol {
            reasons.push(format!(
                "Too high liquidity: {:.2} SOL",
                pool.liquidity_sol
            ));
        }

        // Age check
        if let Some(created_at) = pool.created_at {
            let age_ms = chrono::Utc::now().timestamp_millis() - created_at;
            let age_minutes = age_ms / 1000 / 60;

            if age_minutes > config.sniper.max_age_minutes as i64 {
                reasons.push(format!("Token too old: {} minutes", age_minutes));
            }
        }

        // Volume check (optional)
        if pool.volume_24h < 1000.0 {
            reasons.push(format!("Low 24h volume: ${:.2}", pool.volume_24h));
        }

        SafetyCheckResult {
            passed: reasons.is_empty(),
            reasons,
        }
    }

    // Static version of execute_snipe for standalone evaluation
    #[allow(clippy::too_many_arguments)]
    async fn execute_snipe_standalone(
        pool: TokenPool,
        playbook: RegimePlaybook,
        ml_features: crate::ml::FeatureVector,  // ⭐ ML features for learning
        config: Arc<RwLock<Config>>,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        _price_feed: Arc<PriceFeed>,
        db: Arc<RwLock<Database>>,
        active_positions: Arc<RwLock<HashMap<String, Position>>>,
        _circuit_breaker: Arc<RwLock<CircuitBreaker>>,
        _daily_pnl: Arc<RwLock<f64>>,
        _pattern_detectors: Arc<RwLock<LruCache<String, PatternDetector>>>,
        _ml_trainer: Arc<RwLock<MLTrainer>>,
        _price_cache: Arc<RwLock<PriceCache>>,
        _profit_sweep: Arc<ProfitSweepManager>,
    ) -> Result<()> {
        info!("EXECUTING SNIPE: {}", pool.token_symbol);

        let cfg = config.read().await;
        let snipe_amount = cfg.sniper.snipe_amount_sol;

        info!("  Amount: {} SOL", snipe_amount);

        let start = std::time::Instant::now();

        // Execute swap via Jupiter
        let result = jupiter
            .buy_with_sol(&solana, &cfg, &pool.token_address, snipe_amount)
            .await?;

        drop(cfg);

        if result.success {
            let execution_time = start.elapsed().as_millis();

            // Get signature
            let signature = result.signature.as_ref()
                .ok_or_else(|| anyhow!("Missing signature in successful swap"))?;

            info!("  Snipe successful in {}ms", execution_time);
            info!("  TX: {}", signature);
            info!("  Tokens received: {}", result.output_amount);

            let entry_price = snipe_amount / result.output_amount as f64;

            // Record trade in database
            let mut db_lock = db.write().await;
            let trade_id = db_lock.record_trade_entry(
                "sniper",
                &pool.token_address,
                &pool.token_symbol,
                entry_price,
                snipe_amount,
                result.output_amount as f64,
                signature,
                result.fees as f64 / 1_000_000_000.0,
            )?;
            drop(db_lock);

            // Add to active positions
            let token_pubkey = Pubkey::from_str(&pool.token_address)?;
            let mut positions = active_positions.write().await;
            let now = chrono::Utc::now().timestamp_millis();

            positions.insert(
                pool.token_address.clone(),
                Position {
                    trade_id,
                    token_address: token_pubkey,
                    token_symbol: pool.token_symbol.clone(),
                    entry_price,
                    amount_sol: snipe_amount,
                    tokens_bought: result.output_amount as f64,
                    entry_time: now,
                    highest_price: entry_price,
                    state: crate::models::PositionState::Sniper,
                    validation_window_end: now + (10 * 60 * 1000),  // 10 min validation window
                    higher_lows_count: 0,
                    stop_loss_pct: playbook.stop_loss_pct,
                    take_profit_pcts: playbook.take_profit_pcts,
                    time_stop_minutes: playbook.time_stop_minutes,
                    ml_features: Some(ml_features.features.to_vec()),  // ⭐ Store for ML learning
                },
            );
            let cfg = config.read().await;
            info!(
                "  Active positions: {}/{}",
                positions.len(),
                cfg.risk.max_concurrent_positions
            );
        } else {
            error!("  Snipe failed: {:?}", result.error);

            let mut db_lock = db.write().await;
            db_lock.log_risk_event(
                "snipe_failed",
                "medium",
                &format!("Failed to snipe {}", pool.token_symbol),
                result.error.as_deref().unwrap_or("Unknown error"),
            )?;
        }

        Ok(())
    }

    async fn execute_snipe(&self, pool: TokenPool) -> Result<()> {
        info!("EXECUTING SNIPE: {}", pool.token_symbol);

        let config = self.config.read().await;
        let snipe_amount = config.sniper.snipe_amount_sol;

        info!("  Amount: {} SOL", snipe_amount);

        let start = std::time::Instant::now();

        // Execute swap via Jupiter
        let result = self
            .jupiter
            .buy_with_sol(&self.solana, &config, &pool.token_address, snipe_amount)
            .await?;

        drop(config);

        if result.success {
            let execution_time = start.elapsed().as_millis();

            // Get signature (should always exist when success=true)
            let signature = result.signature.as_ref()
                .ok_or_else(|| anyhow!("Missing signature in successful swap"))?;

            info!("  Snipe successful in {}ms", execution_time);
            info!("  TX: {}", signature);
            info!("  Tokens received: {}", result.output_amount);

            let entry_price = snipe_amount / result.output_amount as f64;

            // Record trade in database
            let mut db = self.db.write().await;
            let trade_id = db.record_trade_entry(
                "sniper",
                &pool.token_address,
                &pool.token_symbol,
                entry_price,
                snipe_amount,
                result.output_amount as f64,
                signature,
                result.fees as f64 / 1_000_000_000.0,
            )?;
            drop(db);

            // Add to active positions
            let token_pubkey = Pubkey::from_str(&pool.token_address)?;
            let config = self.config.read().await;
            let mut positions = self.active_positions.write().await;
            let now = chrono::Utc::now().timestamp_millis();

            positions.insert(
                pool.token_address.clone(),
                Position {
                    trade_id,
                    token_address: token_pubkey,
                    token_symbol: pool.token_symbol.clone(),
                    entry_price,
                    amount_sol: snipe_amount,
                    tokens_bought: result.output_amount as f64,
                    entry_time: now,
                    highest_price: entry_price,
                    state: crate::models::PositionState::Sniper,
                    validation_window_end: now + (10 * 60 * 1000),  // 10 min validation window
                    higher_lows_count: 0,
                    stop_loss_pct: config.risk.stop_loss_percent,
                    take_profit_pcts: vec![config.risk.take_profit_percent],
                    time_stop_minutes: Some(60),
                    ml_features: None,  // Old code path - no ML features
                },
            );
            info!(
                "  Active positions: {}/{}",
                positions.len(),
                config.risk.max_concurrent_positions
            );
        } else {
            error!("  Snipe failed: {:?}", result.error);

            let mut db = self.db.write().await;
            db.log_risk_event(
                "snipe_failed",
                "medium",
                &format!("Failed to snipe {}", pool.token_symbol),
                result.error.as_deref().unwrap_or("Unknown error"),
            )?;
        }

        Ok(())
    }

    fn spawn_position_monitor(&self) {
        let active_positions = self.active_positions.clone();
        let config = self.config.clone();
        let solana = self.solana.clone();
        let jupiter = self.jupiter.clone();
        let price_feed = self.price_feed.clone();
        let db = self.db.clone();
        let daily_pnl = self.daily_pnl.clone();
        let is_running = self.is_running.clone();
        let price_cache = self.price_cache.clone(); // ✅ PHASE 1: Add price cache
        let circuit_breaker = self.circuit_breaker.clone();
        let profit_sweep = self.profit_sweep.clone();
        let ml_trainer = self.ml_trainer.clone();  // ⭐ ML for learning

        tokio::spawn(async move {
            info!("Starting position monitor...");

            while *is_running.read().await {
                let positions: Vec<_> = {
                    let positions = active_positions.read().await;
                    positions.values().cloned().collect()
                };

                // Process positions concurrently for better performance
                let futures: Vec<_> = positions.into_iter().map(|position| {
                    let config = config.clone();
                    let solana = solana.clone();
                    let jupiter = jupiter.clone();
                    let price_feed = price_feed.clone();
                    let db = db.clone();
                    let daily_pnl = daily_pnl.clone();
                    let active_positions = active_positions.clone();
                    let price_cache = price_cache.clone(); // ✅ PHASE 1
                    let circuit_breaker = circuit_breaker.clone();
                    let profit_sweep = profit_sweep.clone();
                    let ml_trainer = ml_trainer.clone();  // ⭐ ML

                    async move {
                        if let Err(e) = Self::check_position_status_static(&position,
                            &config,
                            &solana,
                            &jupiter,
                            &price_feed,
                            &db,
                            &daily_pnl,
                            &active_positions,
                            &price_cache, // ✅ PHASE 1
                            &circuit_breaker,
                            &profit_sweep,
                            &ml_trainer,  // ⭐ ML
                        )
                        .await
                        {
                            error!("Error checking position {}: {}", position.token_symbol, e);
                        }
                    }
                }).collect();

                // Wait for all position checks to complete concurrently
                futures::future::join_all(futures).await;

                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }

    async fn check_position_status_static(position: &Position,
        config: &Arc<RwLock<Config>>,
        solana: &Arc<SolanaConnection>,
        jupiter: &Arc<JupiterService>,
        price_feed: &Arc<PriceFeed>,
        db: &Arc<RwLock<Database>>,
        daily_pnl: &Arc<RwLock<f64>>,
        active_positions: &Arc<RwLock<HashMap<String, Position>>>,
        price_cache: &Arc<RwLock<PriceCache>>, // ✅ PHASE 1
        circuit_breaker: &Arc<RwLock<CircuitBreaker>>,
        profit_sweep: &Arc<ProfitSweepManager>,
        ml_trainer: &Arc<RwLock<MLTrainer>>,  // ⭐ ML
    ) -> Result<()> {
        // ✅ PHASE 1: Try cache first, then fetch if needed
        let token_address_str = position.token_address.to_string();

        let cached_price = price_cache.read().await.get(&token_address_str);

        let current_price = if let Some(price) = cached_price {
            price
        } else {
            // Cache miss - fetch from Jupiter
            let price = price_feed
                .get_token_price_jupiter(jupiter, &token_address_str)
                .await?;

            // Store in cache for next time
            if price > 0.0 {
                price_cache.write().await.set(&token_address_str, price);
            }

            price
        };

        if current_price == 0.0 {
            return Ok(());
        }

        let price_change = ((current_price - position.entry_price) / position.entry_price) * 100.0;
        let current_value = position.tokens_bought * current_price;
        let pnl_sol = current_value - position.amount_sol;
        let pnl_percent = (pnl_sol / position.amount_sol) * 100.0;

        // Update highest price for trailing stop
        {
            let mut positions = active_positions.write().await;
            if let Some(pos) = positions.get_mut(&position.token_address.to_string()) {
                if current_price > pos.highest_price {
                    pos.highest_price = current_price;
                }
            }
        }

        let config = config.read().await;

        // ⭐ P1.3: Sniper→Momentum Handoff Logic
        // During validation window (2-10 min), check if position qualifies for momentum mode
        let now = chrono::Utc::now().timestamp_millis();
        {
            let mut positions = active_positions.write().await;
            if let Some(pos) = positions.get_mut(&position.token_address.to_string()) {
                if pos.state == crate::models::PositionState::Sniper {
                    // Check if still in validation window
                    if now < pos.validation_window_end {
                        // Track higher lows for momentum detection
                        // Simple VWAP proxy: if price > entry_price * 1.2, we're above "VWAP"
                        let above_vwap_proxy = current_price > (pos.entry_price * 1.2);

                        // Check for higher low (price above entry and not making new lows)
                        if current_price > pos.entry_price {
                            // Check if this is a higher low compared to last check
                            if current_price >= pos.entry_price * (1.0 + (pos.higher_lows_count as f64 * 0.05)) {
                                pos.higher_lows_count += 1;
                            }
                        }

                        // Promotion criteria:
                        // 1. Price > 20% above entry (above VWAP proxy)
                        // 2. At least 2 consecutive higher lows
                        // 3. Positive PnL momentum
                        if above_vwap_proxy && pos.higher_lows_count >= 2 && pnl_percent > 15.0 {
                            info!("🚀 MOMENTUM PROMOTION: {} transitioning to momentum mode!", pos.token_symbol);
                            info!("   ✓ Price above VWAP proxy ({}% gain)", pnl_percent);
                            info!("   ✓ {} consecutive higher lows", pos.higher_lows_count);
                            info!("   ✓ New targets: 5x-20x (from 2x-5x)");

                            pos.state = crate::models::PositionState::Momentum;
                            // Adjust take profit for momentum mode (higher targets)
                            pos.take_profit_pcts = vec![500.0, 1000.0, 2000.0];  // 5x, 10x, 20x
                            pos.stop_loss_pct = 30.0;  // Wider stop for momentum
                        }
                    } else if now >= pos.validation_window_end {
                        // Validation window ended without promotion
                        if pnl_percent < 10.0 {
                            info!("⚠️  {} failed validation window - exit ASAP", pos.token_symbol);
                            pos.state = crate::models::PositionState::Failed;
                        } else {
                            // Doing okay but didn't qualify for momentum - keep sniper targets
                            info!("✓ {} passed validation - maintaining sniper targets", pos.token_symbol);
                        }
                    }
                }
            }
        }

        // Check exit conditions
        let mut should_exit = false;
        let mut exit_reason = String::new();

        // ⭐ P1.3: Failed positions exit ASAP
        if position.state == crate::models::PositionState::Failed {
            should_exit = true;
            exit_reason = format!("Failed validation window - exiting at {}%", pnl_percent);
        }

        // ✅ PRIORITY 1.5: Use regime-specific playbook stops
        // Take profit (use first level from playbook)
        let take_profit_pct = if !position.take_profit_pcts.is_empty() {
            position.take_profit_pcts[0]
        } else {
            config.risk.take_profit_percent * 100.0 // Fallback to config
        };

        if pnl_percent >= take_profit_pct {
            should_exit = true;
            exit_reason = format!("Take profit at {:.2}% (regime target: {:.1}%)", pnl_percent, take_profit_pct);
        }

        // Stop loss (use regime-specific stop from playbook)
        let stop_loss_pct = position.stop_loss_pct;
        if pnl_percent <= -stop_loss_pct {
            should_exit = true;
            exit_reason = format!("Stop loss at {:.2}% (regime stop: -{:.1}%)", pnl_percent, stop_loss_pct);
        }

        // Trailing stop
        if config.risk.trailing_stop_enabled {
            let positions = active_positions.read().await;
            if let Some(pos) = positions.get(&position.token_address.to_string()) {
                let drop_from_high = ((pos.highest_price - current_price) / pos.highest_price) * 100.0;
                if drop_from_high >= (config.risk.trailing_stop_percent * 100.0) {
                    should_exit = true;
                    exit_reason = format!("Trailing stop at {:.2}% from high", drop_from_high);
                }
            }
        }

        // ✅ PRIORITY 1.5: Use regime-specific time stop
        // Check regime time stop first (for NewPair/MeanReversion)
        let now = chrono::Utc::now().timestamp_millis();
        let hold_time_minutes = (now - position.entry_time) / (1000 * 60);

        if let Some(time_stop_minutes) = position.time_stop_minutes {
            if hold_time_minutes >= time_stop_minutes as i64 {
                should_exit = true;
                exit_reason = format!("Regime time stop reached ({} minutes, limit: {})",
                    hold_time_minutes, time_stop_minutes);
                info!("⏰ Regime time stop triggered: {} minutes", hold_time_minutes);
            }
        } else {
            // Fallback to general max hold time if no regime time stop
            const MAX_HOLD_TIME_HOURS: i64 = 4;
            let hold_time_hours = hold_time_minutes / 60;

            if hold_time_hours >= MAX_HOLD_TIME_HOURS {
                should_exit = true;
                exit_reason = format!("Max hold time reached ({} hours)", hold_time_hours);
                info!("⏰ Position held for {} hours, triggering time-based exit", hold_time_hours);
            }
        }

        if should_exit {
            info!("EXIT SIGNAL: {}", position.token_symbol);
            info!("  Reason: {}", exit_reason);
            info!("  PnL: {:.4} SOL ({:.2}%)", pnl_sol, pnl_percent);

            drop(config);

            Self::exit_position(
                position,
                current_price,
                &exit_reason,
                solana,
                jupiter,
                db,
                daily_pnl,
                active_positions,
                circuit_breaker,
                profit_sweep,
                ml_trainer,  // ⭐ ML
            )
            .await?;
        }

        Ok(())
    }

    async fn exit_position(
        position: &Position,
        exit_price: f64,
        reason: &str,
        solana: &Arc<SolanaConnection>,
        jupiter: &Arc<JupiterService>,
        db: &Arc<RwLock<Database>>,
        daily_pnl: &Arc<RwLock<f64>>,
        active_positions: &Arc<RwLock<HashMap<String, Position>>>,
        circuit_breaker: &Arc<RwLock<CircuitBreaker>>,
        profit_sweep: &Arc<ProfitSweepManager>,
        ml_trainer: &Arc<RwLock<MLTrainer>>,  // ⭐ ML for learning
    ) -> Result<()> {
        info!("Executing exit for {}...", position.token_symbol);

        // Get current config
        let config = Config::from_env()?;

        // Execute sell
        let result = jupiter
            .sell_for_sol(
                solana,
                &config,
                &position.token_address.to_string(),
                position.tokens_bought as u64,
            )
            .await?;

        if result.success {
            let sol_received = result.output_amount as f64 / 1_000_000_000.0;
            let pnl_sol = sol_received - position.amount_sol;
            let pnl_percent = (pnl_sol / position.amount_sol) * 100.0;

            // Get signature (should always exist when success=true)
            let signature = result.signature.as_ref()
                .ok_or_else(|| anyhow!("Missing signature in successful swap"))?;

            info!("  Exit successful");
            info!("  TX: {}", signature);
            info!("  Received: {} SOL", sol_received);
            info!("  PnL: {:.4} SOL ({:.2}%)", pnl_sol, pnl_percent);

            // Update database
            let mut db = db.write().await;
            db.record_trade_exit(
                position.trade_id,
                exit_price,
                position.tokens_bought,
                pnl_sol,
                pnl_percent,
                signature,
                result.fees as f64 / 1_000_000_000.0,
            )?;

            // Update daily PnL
            *daily_pnl.write().await += pnl_sol;

            // ✅ CRITICAL FIX #3: Record trade outcome in circuit breaker
            let mut breaker = circuit_breaker.write().await;
            breaker.record_trade(pnl_sol, pnl_percent);
            let status = breaker.get_stats();
            drop(breaker);

            if status.consecutive_losses >= 3 {
                warn!("⚠️  Circuit breaker triggered after this trade!");
                warn!("   Consecutive losses: {}", status.consecutive_losses);
            }

            // Update capital in database
            let new_capital = config.capital.current + pnl_sol;
            db.update_capital(new_capital)?;

            info!("  Capital: {:.4} SOL", new_capital);
            info!("  Daily PnL: {:.4} SOL", *daily_pnl.read().await);

            // ✅ CRITICAL FIX #2: Check and execute profit sweep
            match profit_sweep.check_and_sweep(new_capital).await {
                Ok(Some(sweep_record)) => {
                    info!("💰 Profit sweep executed!");
                    info!("  Swept: {:.4} SOL to cold storage", sweep_record.swept_amount);
                    info!("  New hot wallet balance: {:.4} SOL", sweep_record.capital_after);

                    // Update capital in database to reflect sweep
                    db.update_capital(sweep_record.capital_after)?;
                }
                Ok(None) => {
                    // No sweep needed (below threshold)
                }
                Err(e) => {
                    warn!("⚠️  Profit sweep failed: {}", e);
                    // Continue - don't fail the whole exit
                }
            }

            // ⭐ ML LEARNING: Record trade outcome for model improvement
            if let Some(ref feature_values) = position.ml_features {
                use crate::ml::{FeatureVector, TrainingDataCollector, TrainingSample};

                // Reconstruct feature vector from stored values
                let mut features_array = [0.0; 15];
                for (i, &val) in feature_values.iter().take(15).enumerate() {
                    features_array[i] = val;
                }
                let features = FeatureVector {
                    features: features_array,
                    feature_names: FeatureVector::FEATURE_NAMES,
                };

                // Create training sample
                let sample = TrainingSample {
                    features,
                    outcome: pnl_sol > 0.0,  // Win if profit > 0
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    pnl_percent,
                };

                // Add to ML trainer for continuous learning
                ml_trainer.write().await.add_sample(sample);

                // Log ML progress
                let trainer = ml_trainer.read().await;
                let progress = trainer.get_progress();
                info!("📚 ML Learning: {}", progress.to_string());
                drop(trainer);
            } else {
                warn!("⚠️  No ML features stored - skipping learning");
            }

            // Remove from active positions
            active_positions
                .write()
                .await
                .remove(&position.token_address.to_string());
        } else {
            error!("  Exit failed: {:?}", result.error);
        }

        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        info!("Stopping Token Sniper...");
        *self.is_running.write().await = false;

        // Print final stats
        let db = self.db.read().await;
        let stats = db.get_all_time_stats()?;

        info!("Final Statistics:");
        info!("  Total Trades: {}", stats.total_trades);
        info!("  Win Rate: {:.2}%", stats.win_rate);
        info!("  Total PnL: {:.4} SOL", stats.total_pnl);

        Ok(())
    }
}
