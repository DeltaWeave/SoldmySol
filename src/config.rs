use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub rpc: RpcConfig,
    pub wallet: WalletConfig,
    pub capital: CapitalConfig,
    pub risk: RiskConfig,
    pub sniper: SniperConfig,
    pub momentum: MomentumConfig,
    pub performance: PerformanceConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub url: String,
    pub ws_url: String,
    pub commitment: String,
    // MEV Protection
    pub use_private_rpc: bool,
    pub jito_enabled: bool,
    pub jito_tip_lamports: u64,
    // RPC Failover
    pub backup_rpc_urls: Vec<String>,
    pub max_rpc_latency_ms: u64,
}

#[derive(Debug, Clone)]
pub struct WalletConfig {
    pub private_key: String,
    // Profit Sweep (CRITICAL FIX #2)
    pub cold_wallet_address: Option<String>,
    pub profit_sweep_enabled: bool,
    pub profit_sweep_threshold_pct: f64,  // e.g., 25% = sweep at +25% growth
    pub profit_sweep_amount_pct: f64,     // e.g., 25% = sweep 25% of profits
}

#[derive(Debug, Clone)]
pub struct CapitalConfig {
    pub initial: f64,
    pub target: f64,
    pub current: f64,
}

#[derive(Debug, Clone)]
pub struct RiskConfig {
    pub stop_loss_percent: f64,
    pub take_profit_percent: f64,
    pub max_daily_loss: f64,
    pub max_concurrent_positions: usize,
    pub trailing_stop_enabled: bool,
    pub trailing_stop_percent: f64,
    pub slippage_bps: u64,
}

#[derive(Debug, Clone)]
pub struct SniperConfig {
    pub min_liquidity_sol: f64,
    pub max_liquidity_sol: f64,
    pub snipe_amount_sol: f64,
    pub auto_sell_multiplier: f64,
    pub max_buy_tax: f64,
    pub max_sell_tax: f64,
    pub min_holders: u32,
    pub max_holder_percent: f64,
    pub max_age_minutes: u64,
    // P0 CRITICAL: Exit route validation
    pub max_exit_impact_bps: u64,  // Max acceptable round-trip impact (e.g., 1500 = 15%)
    pub require_exit_route: bool,   // Reject tokens without exit route
}

#[derive(Debug, Clone)]
pub struct MomentumConfig {
    pub scan_interval_secs: u64,
    pub min_volume_24h_usd: f64,
    pub min_liquidity_usd: f64,
    pub min_price_change_24h_pct: f64,
    pub max_candidates: usize,
}

#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    pub priority_fee_lamports: u64,
    pub compute_unit_price: u64,
    // Execution Timing SLOs (CRITICAL FIX #3)
    pub entry_quote_max_age_ms: u64,      // Max age for entry quotes (2000ms)
    pub exit_quote_max_age_ms: u64,       // Max age for exit quotes (1000ms)
    pub tx_build_max_ms: u64,             // Max time to build tx (500ms)
    pub max_entry_retries: u32,           // Max retries for entry (2)
    pub max_exit_retries: u32,            // Max retries for exit (6)
    pub exit_priority_fee_escalation: bool, // Escalate fees on exit retries
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub path: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let rpc = RpcConfig {
            url: env::var("SOLANA_RPC_URL")
                .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string()),
            ws_url: env::var("SOLANA_RPC_WS")
                .unwrap_or_else(|_| "wss://api.mainnet-beta.solana.com".to_string()),
            commitment: env::var("RPC_COMMITMENT").unwrap_or_else(|_| "confirmed".to_string()),
            // MEV Protection (CRITICAL FIX #1)
            use_private_rpc: env::var("USE_PRIVATE_RPC")
                .unwrap_or_else(|_| "true".to_string())
                .parse::<bool>()?,
            jito_enabled: env::var("JITO_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                .parse::<bool>()?,
            jito_tip_lamports: env::var("JITO_TIP_LAMPORTS")
                .unwrap_or_else(|_| "10000".to_string())  // 0.00001 SOL default tip
                .parse::<u64>()?,
            backup_rpc_urls: env::var("BACKUP_RPC_URLS")
                .unwrap_or_else(|_| "".to_string())
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
            max_rpc_latency_ms: env::var("MAX_RPC_LATENCY_MS")
                .unwrap_or_else(|_| "600".to_string())
                .parse::<u64>()?,
        };

        let wallet = WalletConfig {
            private_key: env::var("PRIVATE_KEY").context("PRIVATE_KEY not found in .env")?,
            // Profit Sweep (CRITICAL FIX #2)
            cold_wallet_address: env::var("COLD_WALLET_ADDRESS").ok(),
            profit_sweep_enabled: env::var("PROFIT_SWEEP_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                .parse::<bool>()?,
            profit_sweep_threshold_pct: env::var("PROFIT_SWEEP_THRESHOLD_PCT")
                .unwrap_or_else(|_| "25.0".to_string())
                .parse::<f64>()?,
            profit_sweep_amount_pct: env::var("PROFIT_SWEEP_AMOUNT_PCT")
                .unwrap_or_else(|_| "25.0".to_string())
                .parse::<f64>()?,
        };

        let initial_capital = env::var("INITIAL_CAPITAL")
            .unwrap_or_else(|_| "100".to_string())
            .parse::<f64>()?;

        let capital = CapitalConfig {
            initial: initial_capital,
            target: env::var("TARGET_CAPITAL")
                .unwrap_or_else(|_| "10000".to_string())
                .parse::<f64>()?,
            current: initial_capital,
        };

        let risk = RiskConfig {
            stop_loss_percent: env::var("STOP_LOSS_PERCENT")
                .unwrap_or_else(|_| "0.20".to_string())
                .parse::<f64>()?,
            take_profit_percent: env::var("TAKE_PROFIT_PERCENT")
                .unwrap_or_else(|_| "2.0".to_string())
                .parse::<f64>()?,
            max_daily_loss: env::var("MAX_DAILY_LOSS")
                .unwrap_or_else(|_| "0.10".to_string())
                .parse::<f64>()?,
            max_concurrent_positions: env::var("MAX_CONCURRENT_POSITIONS")
                .unwrap_or_else(|_| "3".to_string())
                .parse::<usize>()?,
            trailing_stop_enabled: env::var("TRAILING_STOP_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                .parse::<bool>()?,
            trailing_stop_percent: env::var("TRAILING_STOP_PERCENT")
                .unwrap_or_else(|_| "0.30".to_string())
                .parse::<f64>()?,
            slippage_bps: env::var("SLIPPAGE_BPS")
                .unwrap_or_else(|_| "500".to_string())
                .parse::<u64>()?,
        };

        let sniper = SniperConfig {
            min_liquidity_sol: env::var("MIN_LIQUIDITY_SOL")
                .unwrap_or_else(|_| "5".to_string())
                .parse::<f64>()?,
            max_liquidity_sol: env::var("MAX_LIQUIDITY_SOL")
                .unwrap_or_else(|_| "100".to_string())
                .parse::<f64>()?,
            snipe_amount_sol: env::var("SNIPE_AMOUNT_SOL")
                .unwrap_or_else(|_| "0.5".to_string())
                .parse::<f64>()?,
            auto_sell_multiplier: env::var("AUTO_SELL_MULTIPLIER")
                .unwrap_or_else(|_| "2.0".to_string())
                .parse::<f64>()?,
            max_buy_tax: env::var("MAX_BUY_TAX")
                .unwrap_or_else(|_| "10".to_string())
                .parse::<f64>()?,
            max_sell_tax: env::var("MAX_SELL_TAX")
                .unwrap_or_else(|_| "10".to_string())
                .parse::<f64>()?,
            min_holders: env::var("MIN_HOLDERS")
                .unwrap_or_else(|_| "10".to_string())
                .parse::<u32>()?,
            max_holder_percent: env::var("MAX_HOLDER_PERCENT")
                .unwrap_or_else(|_| "30".to_string())
                .parse::<f64>()?,
            max_age_minutes: env::var("MAX_TOKEN_AGE_MINUTES")
                .unwrap_or_else(|_| "60".to_string())
                .parse::<u64>()?,
            // P0 CRITICAL: Exit route validation
            max_exit_impact_bps: env::var("MAX_EXIT_IMPACT_BPS")
                .unwrap_or_else(|_| "2000".to_string())  // 20% max round-trip loss
                .parse::<u64>()?,
            require_exit_route: env::var("REQUIRE_EXIT_ROUTE")
                .unwrap_or_else(|_| "true".to_string())
                .parse::<bool>()?,
        };

        let performance = PerformanceConfig {
            priority_fee_lamports: env::var("PRIORITY_FEE_LAMPORTS")
                .unwrap_or_else(|_| "50000".to_string())
                .parse::<u64>()?,
            compute_unit_price: env::var("COMPUTE_UNIT_PRICE")
                .unwrap_or_else(|_| "100000".to_string())
                .parse::<u64>()?,
            // Execution Timing SLOs (CRITICAL FIX #3)
            entry_quote_max_age_ms: env::var("ENTRY_QUOTE_MAX_AGE_MS")
                .unwrap_or_else(|_| "2000".to_string())
                .parse::<u64>()?,
            exit_quote_max_age_ms: env::var("EXIT_QUOTE_MAX_AGE_MS")
                .unwrap_or_else(|_| "1000".to_string())
                .parse::<u64>()?,
            tx_build_max_ms: env::var("TX_BUILD_MAX_MS")
                .unwrap_or_else(|_| "500".to_string())
                .parse::<u64>()?,
            max_entry_retries: env::var("MAX_ENTRY_RETRIES")
                .unwrap_or_else(|_| "2".to_string())
                .parse::<u32>()?,
            max_exit_retries: env::var("MAX_EXIT_RETRIES")
                .unwrap_or_else(|_| "6".to_string())
                .parse::<u32>()?,
            exit_priority_fee_escalation: env::var("EXIT_PRIORITY_FEE_ESCALATION")
                .unwrap_or_else(|_| "true".to_string())
                .parse::<bool>()?,
        };

        let momentum = MomentumConfig {
            scan_interval_secs: env::var("MOMENTUM_SCAN_INTERVAL_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse::<u64>()?,
            min_volume_24h_usd: env::var("MOMENTUM_MIN_VOLUME_USD")
                .unwrap_or_else(|_| "10000".to_string())
                .parse::<f64>()?,
            min_liquidity_usd: env::var("MOMENTUM_MIN_LIQUIDITY_USD")
                .unwrap_or_else(|_| "20000".to_string())
                .parse::<f64>()?,
            min_price_change_24h_pct: env::var("MOMENTUM_MIN_PRICE_CHANGE_PCT")
                .unwrap_or_else(|_| "2.5".to_string())
                .parse::<f64>()?,
            max_candidates: env::var("MOMENTUM_MAX_CANDIDATES")
                .unwrap_or_else(|_| "20".to_string())
                .parse::<usize>()?,
        };

        let database = DatabaseConfig {
            path: env::var("DB_PATH").unwrap_or_else(|_| "./data/trades.db".to_string()),
        };

        let mut config = Config {
            rpc,
            wallet,
            capital,
            risk,
            sniper,
            momentum,
            performance,
            database,
        };

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Validate configuration parameters
    pub fn validate(&self) -> Result<()> {
        // Capital validation
        if self.capital.initial <= 0.0 {
            return Err(anyhow!("Initial capital must be positive"));
        }
        if self.capital.target <= self.capital.initial {
            return Err(anyhow!("Target capital must be greater than initial capital"));
        }
        if self.capital.current < 0.0 {
            return Err(anyhow!("Current capital cannot be negative"));
        }
        if self.capital.initial.is_nan() || self.capital.initial.is_infinite() {
            return Err(anyhow!("Initial capital must be a valid number"));
        }

        // Risk validation
        if self.risk.stop_loss_percent <= 0.0 || self.risk.stop_loss_percent > 1.0 {
            return Err(anyhow!("Stop loss must be between 0 and 100%"));
        }
        if self.risk.take_profit_percent <= 0.0 {
            return Err(anyhow!("Take profit must be positive"));
        }
        if self.risk.max_daily_loss <= 0.0 || self.risk.max_daily_loss > 1.0 {
            return Err(anyhow!("Max daily loss must be between 0 and 100%"));
        }
        if self.risk.max_concurrent_positions == 0 || self.risk.max_concurrent_positions > 20 {
            return Err(anyhow!("Max concurrent positions must be between 1 and 20"));
        }
        if self.risk.trailing_stop_percent <= 0.0 || self.risk.trailing_stop_percent > 1.0 {
            return Err(anyhow!("Trailing stop must be between 0 and 100%"));
        }
        if self.risk.slippage_bps > 10000 {
            return Err(anyhow!("Slippage BPS cannot exceed 10000 (100%)"));
        }

        // Sniper validation
        if self.sniper.min_liquidity_sol <= 0.0 {
            return Err(anyhow!("Min liquidity must be positive"));
        }
        if self.sniper.max_liquidity_sol < self.sniper.min_liquidity_sol {
            return Err(anyhow!("Max liquidity must be greater than min liquidity"));
        }
        if self.sniper.snipe_amount_sol <= 0.0 {
            return Err(anyhow!("Snipe amount must be positive"));
        }
        if self.sniper.snipe_amount_sol > self.capital.initial {
            return Err(anyhow!("Snipe amount cannot exceed initial capital"));
        }
        if self.sniper.auto_sell_multiplier <= 1.0 {
            return Err(anyhow!("Auto sell multiplier must be greater than 1"));
        }
        if self.sniper.max_buy_tax < 0.0 || self.sniper.max_buy_tax > 100.0 {
            return Err(anyhow!("Max buy tax must be between 0 and 100%"));
        }
        if self.sniper.max_sell_tax < 0.0 || self.sniper.max_sell_tax > 100.0 {
            return Err(anyhow!("Max sell tax must be between 0 and 100%"));
        }
        if self.sniper.max_holder_percent <= 0.0 || self.sniper.max_holder_percent > 100.0 {
            return Err(anyhow!("Max holder percent must be between 0 and 100%"));
        }

        if self.momentum.scan_interval_secs == 0 {
            return Err(anyhow!("Momentum scan interval must be greater than 0 seconds"));
        }
        if self.momentum.min_volume_24h_usd <= 0.0 {
            return Err(anyhow!("Momentum min volume must be positive"));
        }
        if self.momentum.min_liquidity_usd <= 0.0 {
            return Err(anyhow!("Momentum min liquidity must be positive"));
        }
        if self.momentum.min_price_change_24h_pct <= 0.0 {
            return Err(anyhow!("Momentum min price change must be positive"));
        }
        if self.momentum.max_candidates == 0 || self.momentum.max_candidates > 100 {
            return Err(anyhow!("Momentum max candidates must be between 1 and 100"));
        }

        // ✅ PRIORITY 1.2: Private RPC Enforcement (Section 12)
        // CRITICAL: Refuse to start if USE_PRIVATE_RPC=true but using public RPC
        if self.rpc.use_private_rpc {
            let public_rpc_domains = vec![
                "api.mainnet-beta.solana.com",
                "api.devnet.solana.com",
                "api.testnet.solana.com",
                "rpc.ankr.com/solana",
            ];

            let is_public_rpc = public_rpc_domains.iter().any(|domain| {
                self.rpc.url.contains(domain) || self.rpc.ws_url.contains(domain)
            });

            if is_public_rpc {
                return Err(anyhow!(
                    "\n\n\
                    ╔════════════════════════════════════════════════════════════════╗\n\
                    ║           🚨 CRITICAL: PUBLIC RPC DETECTED 🚨                  ║\n\
                    ╠════════════════════════════════════════════════════════════════╣\n\
                    ║                                                                ║\n\
                    ║  Your configuration requires private RPC (USE_PRIVATE_RPC=true)║\n\
                    ║  but you're using a PUBLIC RPC endpoint.                       ║\n\
                    ║                                                                ║\n\
                    ║  Current RPC: {}                                ║\n\
                    ║                                                                ║\n\
                    ║  ⚠️  Trading with public RPC will result in:                  ║\n\
                    ║     • MEV sandwich attacks (-200% to -500% loss)               ║\n\
                    ║     • Front-running on all trades                              ║\n\
                    ║     • Guaranteed negative EV                                   ║\n\
                    ║                                                                ║\n\
                    ║  📋 TO FIX:                                                    ║\n\
                    ║                                                                ║\n\
                    ║  1. Get a private RPC from:                                    ║\n\
                    ║     • Helius: https://helius.dev                               ║\n\
                    ║     • Triton: https://triton.one                               ║\n\
                    ║     • QuickNode: https://quicknode.com                         ║\n\
                    ║                                                                ║\n\
                    ║  2. Update your .env file:                                     ║\n\
                    ║     SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY║\n\
                    ║     SOLANA_RPC_WS=wss://mainnet.helius-rpc.com/?api-key=YOUR_KEY  ║\n\
                    ║                                                                ║\n\
                    ║  3. OR disable private RPC requirement:                        ║\n\
                    ║     USE_PRIVATE_RPC=false                                      ║\n\
                    ║     (NOT RECOMMENDED - will lose money to MEV)                 ║\n\
                    ║                                                                ║\n\
                    ╚════════════════════════════════════════════════════════════════╝\n\
                    ",
                    self.rpc.url
                ));
            }

            // Validate we actually have a different RPC
            if self.rpc.url.contains("localhost") || self.rpc.url.contains("127.0.0.1") {
                println!("⚠️  WARNING: Using localhost RPC - only safe for development/testing");
            }
        }

        Ok(())
    }

    pub fn adjust_risk_parameters(&mut self, current_capital: f64) {
        let growth_factor = current_capital / self.capital.initial;

        if growth_factor >= 10.0 {
            // Conservative
            self.risk.stop_loss_percent = 0.15;
            self.risk.take_profit_percent = 1.5;
        } else if growth_factor >= 5.0 {
            // Moderate
            self.risk.stop_loss_percent = 0.18;
            self.risk.take_profit_percent = 1.75;
        } else if growth_factor >= 2.0 {
            // Balanced
            self.risk.stop_loss_percent = 0.20;
            self.risk.take_profit_percent = 2.0;
        } else {
            // Aggressive
            self.risk.stop_loss_percent = 0.25;
            self.risk.take_profit_percent = 3.0;
        }

        self.capital.current = current_capital;
    }
}
