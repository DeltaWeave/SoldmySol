// ✅ STRATEGY VALIDATION: Historical Data Collector
// Collects token launch data from Raydium/Pump.fun for backtesting

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::str::FromStr;
use tokio::time::sleep;
use tracing::{error, info, warn};

use solana_trading_bot::config::Config;
use solana_trading_bot::services::{JupiterService, PriceFeed, SolanaConnection, TokenSafetyChecker};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLaunchData {
    pub timestamp: i64,
    pub token_address: String,
    pub token_symbol: String,
    pub token_name: String,
    pub pair_address: String,

    // Launch metrics
    pub initial_liquidity_sol: f64,
    pub initial_price_usd: f64,
    pub initial_market_cap: f64,

    // Price history (24h)
    pub price_5m: f64,
    pub price_15m: f64,
    pub price_1h: f64,
    pub price_4h: f64,
    pub price_24h: f64,

    // Peak metrics
    pub peak_price: f64,
    pub peak_market_cap: f64,
    pub time_to_peak_minutes: i64,

    // Volume metrics
    pub volume_5m: f64,
    pub volume_1h: f64,
    pub volume_24h: f64,

    // Holder metrics
    pub initial_holders: i32,
    pub holders_24h: i32,
    pub top_holder_percent: f64,

    // Outcome
    pub final_price_24h: f64,
    pub price_change_24h_percent: f64,
    pub outcome: TokenOutcome,
    pub rug_pulled: bool,
    pub honeypot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenOutcome {
    MoonShot,      // 10x+ gain
    Success,       // 2x-10x gain
    Moderate,      // 0.5x-2x
    Failed,        // <0.5x
    RugPull,       // Liquidity removed
    Honeypot,      // Can't sell
}

impl std::fmt::Display for TokenOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenOutcome::MoonShot => write!(f, "MoonShot"),
            TokenOutcome::Success => write!(f, "Success"),
            TokenOutcome::Moderate => write!(f, "Moderate"),
            TokenOutcome::Failed => write!(f, "Failed"),
            TokenOutcome::RugPull => write!(f, "RugPull"),
            TokenOutcome::Honeypot => write!(f, "Honeypot"),
        }
    }
}

pub struct DataCollector {
    db_path: String,
    collection_interval_seconds: u64,
    max_tokens_per_day: usize,
    price_feed: PriceFeed,
    jupiter: JupiterService,
    solana: Arc<SolanaConnection>,
    safety_checker: TokenSafetyChecker,
}

impl DataCollector {
    pub fn new(
        db_path: String,
        price_feed: PriceFeed,
        jupiter: JupiterService,
        solana: Arc<SolanaConnection>,
    ) -> Self {
        let safety_checker = TokenSafetyChecker::new(solana.clone());
        Self {
            db_path,
            collection_interval_seconds: 300, // 5 minutes
            max_tokens_per_day: 100, // Focus on quality over quantity
            price_feed,
            jupiter,
            solana,
            safety_checker,
        }
    }

    /// Initialize database schema
    pub fn init_database(&self) -> Result<()> {
        let conn = Connection::open(&self.db_path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS historical_tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                token_address TEXT NOT NULL UNIQUE,
                token_symbol TEXT NOT NULL,
                token_name TEXT NOT NULL,
                pair_address TEXT NOT NULL,

                initial_liquidity_sol REAL NOT NULL,
                initial_price_usd REAL NOT NULL,
                initial_market_cap REAL NOT NULL,

                price_5m REAL NOT NULL,
                price_15m REAL NOT NULL,
                price_1h REAL NOT NULL,
                price_4h REAL NOT NULL,
                price_24h REAL NOT NULL,

                peak_price REAL NOT NULL,
                peak_market_cap REAL NOT NULL,
                time_to_peak_minutes INTEGER NOT NULL,

                volume_5m REAL NOT NULL,
                volume_1h REAL NOT NULL,
                volume_24h REAL NOT NULL,

                initial_holders INTEGER NOT NULL,
                holders_24h INTEGER NOT NULL,
                top_holder_percent REAL NOT NULL,

                final_price_24h REAL NOT NULL,
                price_change_24h_percent REAL NOT NULL,
                outcome TEXT NOT NULL,
                rug_pulled INTEGER NOT NULL,
                honeypot INTEGER NOT NULL,

                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON historical_tokens(timestamp)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_outcome ON historical_tokens(outcome)",
            [],
        )?;

        info!("✅ Historical data database initialized: {}", self.db_path);
        Ok(())
    }

    /// Start collecting historical data
    pub async fn start_collection(&self) -> Result<()> {
        info!("🎯 Starting historical data collection");
        info!("   Interval: {}s", self.collection_interval_seconds);
        info!("   Max per day: {}", self.max_tokens_per_day);

        loop {
            match self.collect_batch().await {
                Ok(count) => {
                    info!("✅ Collected {} tokens this batch", count);
                }
                Err(e) => {
                    error!("❌ Collection failed: {}", e);
                }
            }

            sleep(Duration::from_secs(self.collection_interval_seconds)).await;
        }
    }

    /// Collect one batch of token data
    async fn collect_batch(&self) -> Result<usize> {
        info!("📊 Collecting token data batch...");

        // Get new tokens from Raydium/Pump.fun
        let new_tokens = self.fetch_new_tokens().await?;

        info!("   Found {} new tokens", new_tokens.len());

        let mut saved = 0;

        for token_address in new_tokens {
            match self.track_token(&token_address).await {
                Ok(data) => {
                    self.save_token_data(&data)?;
                    saved += 1;
                    info!("   ✓ Saved: {} ({})", data.token_symbol, data.outcome);
                }
                Err(e) => {
                    warn!("   ✗ Failed to track {}: {}", token_address, e);
                }
            }

            // Rate limiting
            sleep(Duration::from_millis(500)).await;
        }

        Ok(saved)
    }

    /// Fetch newly launched tokens from DEX
    async fn fetch_new_tokens(&self) -> Result<Vec<String>> {
        info!("🔍 Querying DEX aggregators for new token launches...");

        let pools = self.price_feed.get_new_pools().await?;
        let token_addresses = pools
            .into_iter()
            .map(|pool| pool.token_address)
            .collect::<Vec<_>>();

        Ok(token_addresses)
    }

    /// Track a token for 24 hours and collect all metrics
    async fn track_token(&self, token_address: &str) -> Result<TokenLaunchData> {
        info!("📈 Tracking token: {}", token_address);

        // Get initial metrics
        let initial_data = self.get_token_snapshot(token_address).await?;

        // Track price over 24 hours
        let mut price_5m = initial_data.price_usd;
        let mut price_15m = initial_data.price_usd;
        let mut price_1h = initial_data.price_usd;
        let mut price_4h = initial_data.price_usd;

        let mut peak_price = initial_data.price_usd;
        let mut peak_time = 0i64;

        let start_time = Utc::now().timestamp();

        // Track for 24 hours
        for minutes_elapsed in (5..=1440).step_by(5) {
            sleep(Duration::from_secs(300)).await; // 5 minutes

            let snapshot = self.get_token_snapshot(token_address).await?;

            // Update price points
            if minutes_elapsed == 5 {
                price_5m = snapshot.price_usd;
            }
            if minutes_elapsed == 15 {
                price_15m = snapshot.price_usd;
            }
            if minutes_elapsed == 60 {
                price_1h = snapshot.price_usd;
            }
            if minutes_elapsed == 240 {
                price_4h = snapshot.price_usd;
            }

            // Track peak
            if snapshot.price_usd > peak_price {
                peak_price = snapshot.price_usd;
                peak_time = minutes_elapsed;
            }
        }

        // Get final snapshot
        let final_data = self.get_token_snapshot(token_address).await?;

        // Determine outcome
        let price_change_24h = ((final_data.price_usd - initial_data.price_usd) / initial_data.price_usd) * 100.0;

        let outcome = if final_data.rug_pulled {
            TokenOutcome::RugPull
        } else if final_data.honeypot {
            TokenOutcome::Honeypot
        } else if price_change_24h >= 900.0 {
            TokenOutcome::MoonShot
        } else if price_change_24h >= 100.0 {
            TokenOutcome::Success
        } else if price_change_24h >= -50.0 {
            TokenOutcome::Moderate
        } else {
            TokenOutcome::Failed
        };

        Ok(TokenLaunchData {
            timestamp: start_time,
            token_address: token_address.to_string(),
            token_symbol: initial_data.symbol,
            token_name: initial_data.name,
            pair_address: initial_data.pair_address,

            initial_liquidity_sol: initial_data.liquidity_sol,
            initial_price_usd: initial_data.price_usd,
            initial_market_cap: initial_data.market_cap,

            price_5m,
            price_15m,
            price_1h,
            price_4h,
            price_24h: final_data.price_usd,

            peak_price,
            peak_market_cap: peak_price * initial_data.market_cap / initial_data.price_usd,
            time_to_peak_minutes: peak_time,

            volume_5m: final_data.volume_5m,
            volume_1h: final_data.volume_1h,
            volume_24h: final_data.volume_24h,

            initial_holders: initial_data.holders,
            holders_24h: final_data.holders,
            top_holder_percent: final_data.top_holder_percent,

            final_price_24h: final_data.price_usd,
            price_change_24h_percent: price_change_24h,
            outcome,
            rug_pulled: final_data.rug_pulled,
            honeypot: final_data.honeypot,
        })
    }

    /// Get current snapshot of token metrics
    async fn get_token_snapshot(&self, token_address: &str) -> Result<TokenSnapshot> {
        let pool_info = self.price_feed.get_token_info(token_address).await?;

        let mut symbol = "UNKNOWN".to_string();
        let mut name = "UNKNOWN".to_string();
        let mut pair_address = "UNKNOWN".to_string();
        let mut liquidity_sol = 0.0;
        let mut market_cap = 0.0;
        let mut volume_24h = 0.0;
        let mut volume_1h = 0.0;
        let mut volume_5m = 0.0;

        let price_from_dex = if let Some(pool) = &pool_info {
            symbol = pool.token_symbol.clone();
            name = pool.token_symbol.clone();
            pair_address = pool.pair_address.clone();
            liquidity_sol = pool.liquidity_sol;
            volume_24h = pool.volume_24h;
            volume_1h = if pool.volume_1h > 0.0 { pool.volume_1h } else { pool.volume_24h / 24.0 };
            volume_5m = volume_1h / 12.0;
            pool.price_usd
        } else {
            0.0
        };

        let price_usd = match self.jupiter.get_token_price(token_address).await {
            Ok(price) if price > 0.0 => price,
            _ => price_from_dex,
        };

        let holders_data = if let Ok(mint) = solana_sdk::pubkey::Pubkey::from_str(token_address) {
            self.solana.get_token_holder_stats(&mint).await.ok()
        } else {
            None
        };

        let (holders, top_holder_percent) = holders_data.unwrap_or((0, 0.0));
        market_cap = if price_usd > 0.0 {
            let supply = if let Ok(mint) = solana_sdk::pubkey::Pubkey::from_str(token_address) {
                self.solana
                    .get_client()
                    .get_token_supply(&mint)
                    .await
                    .ok()
                    .and_then(|supply| supply.amount.parse::<u64>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            (supply as f64) * price_usd
        } else {
            0.0
        };

        let honeypot = self
            .safety_checker
            .check_honeypot(&self.jupiter, token_address, 1_000_000)
            .await
            .unwrap_or(true);

        let rug_pulled = liquidity_sol < 0.1 && volume_24h < 100.0;

        Ok(TokenSnapshot {
            symbol,
            name,
            pair_address,
            price_usd,
            liquidity_sol,
            market_cap,
            volume_5m,
            volume_1h,
            volume_24h,
            holders: holders as i32,
            top_holder_percent,
            rug_pulled,
            honeypot,
        })
    }

    /// Save token data to database
    fn save_token_data(&self, data: &TokenLaunchData) -> Result<()> {
        let conn = Connection::open(&self.db_path)?;

        conn.execute(
            "INSERT INTO historical_tokens (
                timestamp, token_address, token_symbol, token_name, pair_address,
                initial_liquidity_sol, initial_price_usd, initial_market_cap,
                price_5m, price_15m, price_1h, price_4h, price_24h,
                peak_price, peak_market_cap, time_to_peak_minutes,
                volume_5m, volume_1h, volume_24h,
                initial_holders, holders_24h, top_holder_percent,
                final_price_24h, price_change_24h_percent, outcome,
                rug_pulled, honeypot, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                      ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                      ?25, ?26, ?27, ?28)",
            params![
                data.timestamp,
                data.token_address,
                data.token_symbol,
                data.token_name,
                data.pair_address,
                data.initial_liquidity_sol,
                data.initial_price_usd,
                data.initial_market_cap,
                data.price_5m,
                data.price_15m,
                data.price_1h,
                data.price_4h,
                data.price_24h,
                data.peak_price,
                data.peak_market_cap,
                data.time_to_peak_minutes,
                data.volume_5m,
                data.volume_1h,
                data.volume_24h,
                data.initial_holders,
                data.holders_24h,
                data.top_holder_percent,
                data.final_price_24h,
                data.price_change_24h_percent,
                format!("{:?}", data.outcome),
                if data.rug_pulled { 1 } else { 0 },
                if data.honeypot { 1 } else { 0 },
                Utc::now().timestamp(),
            ],
        )?;

        Ok(())
    }

    /// Export collected data for backtesting
    pub fn export_for_backtest(&self, output_path: &str) -> Result<()> {
        let conn = Connection::open(&self.db_path)?;

        let mut stmt = conn.prepare(
            "SELECT * FROM historical_tokens ORDER BY timestamp DESC"
        )?;

        let tokens: Vec<TokenLaunchData> = stmt.query_map([], |row| {
            Ok(TokenLaunchData {
                timestamp: row.get(1)?,
                token_address: row.get(2)?,
                token_symbol: row.get(3)?,
                token_name: row.get(4)?,
                pair_address: row.get(5)?,
                initial_liquidity_sol: row.get(6)?,
                initial_price_usd: row.get(7)?,
                initial_market_cap: row.get(8)?,
                price_5m: row.get(9)?,
                price_15m: row.get(10)?,
                price_1h: row.get(11)?,
                price_4h: row.get(12)?,
                price_24h: row.get(13)?,
                peak_price: row.get(14)?,
                peak_market_cap: row.get(15)?,
                time_to_peak_minutes: row.get(16)?,
                volume_5m: row.get(17)?,
                volume_1h: row.get(18)?,
                volume_24h: row.get(19)?,
                initial_holders: row.get(20)?,
                holders_24h: row.get(21)?,
                top_holder_percent: row.get(22)?,
                final_price_24h: row.get(23)?,
                price_change_24h_percent: row.get(24)?,
                outcome: match row.get::<_, String>(25)?.as_str() {
                    "MoonShot" => TokenOutcome::MoonShot,
                    "Success" => TokenOutcome::Success,
                    "Moderate" => TokenOutcome::Moderate,
                    "Failed" => TokenOutcome::Failed,
                    "RugPull" => TokenOutcome::RugPull,
                    "Honeypot" => TokenOutcome::Honeypot,
                    _ => TokenOutcome::Failed,
                },
                rug_pulled: row.get::<_, i32>(26)? == 1,
                honeypot: row.get::<_, i32>(27)? == 1,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

        let json = serde_json::to_string_pretty(&tokens)?;
        std::fs::write(output_path, json)?;

        info!("✅ Exported {} tokens to {}", tokens.len(), output_path);
        Ok(())
    }
}

#[derive(Debug)]
struct TokenSnapshot {
    symbol: String,
    name: String,
    pair_address: String,
    price_usd: f64,
    liquidity_sol: f64,
    market_cap: f64,
    volume_5m: f64,
    volume_1h: f64,
    volume_24h: f64,
    holders: i32,
    top_holder_percent: f64,
    rug_pulled: bool,
    honeypot: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    info!("🚀 Historical Data Collector Starting");

    let config = Config::from_env()?;
    let price_feed = PriceFeed::new()?;
    let jupiter = JupiterService::new()?;
    let solana = Arc::new(SolanaConnection::new(&config.rpc.url, &config.wallet.private_key)?);

    let collector = DataCollector::new(
        "historical_tokens.db".to_string(),
        price_feed,
        jupiter,
        solana,
    );

    // Initialize database
    collector.init_database()?;

    // Start collection
    collector.start_collection().await?;

    Ok(())
}

// Usage:
// cargo run --bin data_collector
//
// Export for backtesting:
// let collector = DataCollector::new("historical_tokens.db".to_string());
// collector.export_for_backtest("backtest_data.json")?;
