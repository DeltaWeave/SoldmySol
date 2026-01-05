/// Hybrid Trading Strategy Coordinator
///
/// Runs multiple strategies in parallel:
/// 1. Pump.fun Token Launch Sniping (pumpfun_token_sniper) - Snipe new tokens on bonding curve
/// 2. Token Creation + First Liquidity (token_creation_monitor) - Snipe fresh tokens with new liquidity
/// 3. Momentum Trading (momentum_trader) - Trade established tokens with bullish patterns
/// 4. Pool Sniping (token_sniper) - Background monitoring for pool graduations
///
/// This allows us to:
/// - Catch tokens at creation (Option 1 & 2) - highest profit potential
/// - Trade established trending tokens (Option 3) - lower risk
/// - Keep pool detection running (Option 4) - future opportunities

use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::config::Config;
use crate::services::{Database, JupiterService, PriceFeed, PumpFunSwap, SolanaConnection};
use crate::strategies::{MomentumTrader, PumpfunTokenSniper, TokenCreationMonitor, TokenSniper};

pub struct HybridStrategy {
    config: Config,
    solana: Arc<SolanaConnection>,
    jupiter: Arc<JupiterService>,
    price_feed: Arc<PriceFeed>,
    pumpfun: Arc<PumpFunSwap>,
    db: Arc<Database>,

    // Strategy handles
    pumpfun_sniper_handle: Option<JoinHandle<()>>,
    token_creation_handle: Option<JoinHandle<()>>,
    momentum_handle: Option<JoinHandle<()>>,
    pool_sniper_handle: Option<JoinHandle<()>>,
}

impl HybridStrategy {
    pub fn new(
        config: Config,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        price_feed: Arc<PriceFeed>,
        pumpfun: Arc<PumpFunSwap>,
        db: Arc<Database>,
    ) -> Self {
        Self {
            config,
            solana,
            jupiter,
            price_feed,
            pumpfun,
            db,
            pumpfun_sniper_handle: None,
            token_creation_handle: None,
            momentum_handle: None,
            pool_sniper_handle: None,
        }
    }

    /// Start all strategies in parallel
    pub async fn start(mut self) -> Result<()> {
        info!("\n{}", "=".repeat(80));
        info!("🚀 HYBRID MULTI-STRATEGY TRADING BOT");
        info!("{}", "=".repeat(80));
        info!("");
        info!("🎯 Strategy 1: PUMP.FUN TOKEN LAUNCH SNIPING (High Priority)");
        info!("   - Monitors Pump.fun for NEW token creation events");
        info!("   - Buys tokens on bonding curve BEFORE graduation");
        info!("   - Position: 0.1 SOL | Target: 3x profit");
        info!("   - Focus: Catch tokens at birth, highest upside");
        info!("");
        info!("🔍 Strategy 2: TOKEN CREATION + FIRST LIQUIDITY (High Priority)");
        info!("   - Monitors SPL Token Program for new mint creation");
        info!("   - Detects first liquidity addition to DEX pools");
        info!("   - Snipes immediately when liquidity appears");
        info!("   - Position: 0.2 SOL | Target: 5x profit");
        info!("   - Focus: Fresh tokens, before bots swarm");
        info!("");
        info!("📊 Strategy 3: MOMENTUM TRADING (Medium Priority)");
        info!("   - Scans trending tokens from DexScreener");
        info!("   - Detects bullish patterns (accumulation, breakouts)");
        info!("   - Trades established tokens with confirmed liquidity");
        info!("   - Focus: Lower risk, established tokens");
        info!("");
        info!("🌊 Strategy 4: POOL SNIPING (Background)");
        info!("   - Monitors new pool creation/graduation events");
        info!("   - Validates liquidity and Jupiter routing");
        info!("   - Focus: Future opportunities, kept running");
        info!("");
        info!("{}", "=".repeat(80));
        info!("");

        // Clone config for each strategy
        let pumpfun_config = self.config.clone();
        let token_creation_config = self.config.clone();
        let momentum_config = self.config.clone();
        let pool_sniper_config = self.config.clone();

        // Create separate Database instances where needed
        let pumpfun_db = Database::new(&self.config.database.path)?;
        let token_creation_db = Database::new(&self.config.database.path)?;
        let pool_sniper_db = Database::new(&self.config.database.path)?;

        // Strategy 1: Pump.fun Token Launch Sniper
        let pumpfun_sniper = Arc::new(PumpfunTokenSniper::new(
            pumpfun_config,
            self.solana.clone(),
            self.jupiter.clone(),
            self.pumpfun.clone(),
            Arc::new(pumpfun_db),
        ));

        // Strategy 2: Token Creation Monitor
        let token_creation_monitor = Arc::new(TokenCreationMonitor::new(
            token_creation_config,
            self.solana.clone(),
            self.jupiter.clone(),
            Arc::new(token_creation_db),
        ));

        // Strategy 3: Momentum Trader
        let momentum_trader = Arc::new(MomentumTrader::new(
            momentum_config,
            self.solana.clone(),
            self.jupiter.clone(),
            self.price_feed.clone(),
            self.db.clone(),
        ));

        // Strategy 4: Pool Sniper (background)
        let pool_sniper = Arc::new(TokenSniper::new(
            pool_sniper_config,
            self.solana.clone(),
            self.jupiter.clone(),
            self.price_feed.clone(),
            pool_sniper_db,
        ));

        // Spawn Strategy 1: Pump.fun Token Sniper
        let pumpfun_clone = pumpfun_sniper.clone();
        self.pumpfun_sniper_handle = Some(tokio::spawn(async move {
            info!("🎯 Starting Pump.fun Token Launch Sniper...");
            if let Err(e) = pumpfun_clone.start().await {
                error!("Pump.fun Token Sniper error: {}", e);
            }
        }));

        // Spawn Strategy 2: Token Creation Monitor
        let token_creation_clone = token_creation_monitor.clone();
        self.token_creation_handle = Some(tokio::spawn(async move {
            info!("🔍 Starting Token Creation Monitor...");
            if let Err(e) = token_creation_clone.start().await {
                error!("Token Creation Monitor error: {}", e);
            }
        }));

        // Spawn Strategy 3: Momentum Trader
        let momentum_clone = momentum_trader.clone();
        self.momentum_handle = Some(tokio::spawn(async move {
            info!("📊 Starting Momentum Trader...");
            if let Err(e) = momentum_clone.start().await {
                error!("Momentum Trader error: {}", e);
            }
        }));

        // Spawn Strategy 4: Pool Sniper (background)
        let pool_sniper_clone = pool_sniper.clone();
        self.pool_sniper_handle = Some(tokio::spawn(async move {
            info!("🌊 Starting Pool Sniper (background)...");
            if let Err(e) = pool_sniper_clone.start().await {
                error!("Pool Sniper error: {}", e);
            }
        }));

        info!("✅ All 4 strategies started successfully");
        info!("");

        // Wait for any task to complete (they run forever unless error)
        tokio::select! {
            result = self.pumpfun_sniper_handle.unwrap() => {
                if let Err(e) = result {
                    error!("Pump.fun Token Sniper task panicked: {}", e);
                }
            }
            result = self.token_creation_handle.unwrap() => {
                if let Err(e) = result {
                    error!("Token Creation Monitor task panicked: {}", e);
                }
            }
            result = self.momentum_handle.unwrap() => {
                if let Err(e) = result {
                    error!("Momentum Trader task panicked: {}", e);
                }
            }
            result = self.pool_sniper_handle.unwrap() => {
                if let Err(e) = result {
                    error!("Pool Sniper task panicked: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Stop both strategies gracefully
    pub async fn stop(&self) -> Result<()> {
        info!("⏸️  Stopping Hybrid Strategy...");

        // Strategies are running in background tasks
        // This is a simplified stop - in production would send shutdown signals

        info!("✅ Hybrid Strategy stopped");
        Ok(())
    }
}
