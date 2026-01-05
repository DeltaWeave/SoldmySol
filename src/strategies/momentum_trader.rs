/// Momentum Trading Strategy
///
/// Trades established tokens showing bullish patterns:
/// - Fetches trending tokens from Jupiter/DexScreener
/// - Analyzes price momentum and volume
/// - Uses PatternDetector to identify accumulation/breakout patterns
/// - Executes trades on tokens with confirmed liquidity

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn, error};

use crate::config::Config;
use crate::services::{Database, JupiterService, PriceFeed, SolanaConnection};
use crate::services::jupiter::SOL_MINT;
use crate::strategies::route_validation::validate_entry_exit_routes;
use crate::strategies::{PatternDetector, Pattern, TradeRecommendation, PricePoint};

/// Token candidate for momentum trading
#[derive(Debug, Clone)]
pub struct MomentumCandidate {
    pub token_mint: String,
    pub symbol: String,
    pub price_usd: f64,
    pub volume_24h: f64,
    pub price_change_24h: f64,
    pub liquidity_usd: f64,
    pub market_cap: Option<f64>,
}

/// DexScreener API response for trending pairs
#[derive(Debug, Deserialize)]
struct DexScreenerPair {
    #[serde(rename = "chainId")]
    chain_id: String,
    #[serde(rename = "dexId")]
    dex_id: String,
    #[serde(rename = "baseToken")]
    base_token: DexScreenerToken,
    #[serde(rename = "priceUsd")]
    price_usd: Option<String>,
    volume: Option<DexScreenerVolume>,
    #[serde(rename = "priceChange")]
    price_change: Option<DexScreenerPriceChange>,
    liquidity: Option<DexScreenerLiquidity>,
    #[serde(rename = "marketCap")]
    market_cap: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DexScreenerToken {
    address: String,
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct DexScreenerVolume {
    #[serde(rename = "h24")]
    h24: f64,
}

#[derive(Debug, Deserialize)]
struct DexScreenerPriceChange {
    #[serde(rename = "h24")]
    h24: f64,
}

#[derive(Debug, Deserialize)]
struct DexScreenerLiquidity {
    usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DexScreenerResponse {
    pairs: Option<Vec<DexScreenerPair>>,
}

pub struct MomentumTrader {
    config: Config,
    solana: Arc<SolanaConnection>,
    jupiter: Arc<JupiterService>,
    price_feed: Arc<PriceFeed>,
    db: Arc<Database>,

    /// Pattern detectors per token
    pattern_detectors: Arc<tokio::sync::RwLock<HashMap<String, PatternDetector>>>,

    /// Active positions
    active_positions: Arc<tokio::sync::RwLock<HashMap<String, Position>>>,

    /// Scan interval
    scan_interval_secs: u64,
}

#[derive(Debug, Clone)]
struct Position {
    token_mint: String,
    entry_price: f64,
    amount: f64,
    stop_loss: f64,
    take_profit: f64,
    entry_time: std::time::Instant,
}

impl MomentumTrader {
    pub fn new(
        config: Config,
        solana: Arc<SolanaConnection>,
        jupiter: Arc<JupiterService>,
        price_feed: Arc<PriceFeed>,
        db: Arc<Database>,
    ) -> Self {
        let scan_interval_secs = config.momentum.scan_interval_secs;
        Self {
            config,
            solana,
            jupiter,
            price_feed,
            db,
            pattern_detectors: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_positions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            scan_interval_secs,
        }
    }

    /// Start the momentum trading strategy
    pub async fn start(&self) -> Result<()> {
        info!("🚀 Starting Momentum Trading Strategy...");
        info!("   Scan Interval: {}s", self.scan_interval_secs);
        info!("   Min Liquidity: {} SOL", self.config.sniper.min_liquidity_sol);
        info!("   Position Size: {} SOL", self.config.sniper.snipe_amount_sol);
        info!(
            "   Momentum Filters: min ${:.0} volume | min ${:.0} liquidity | min {:.1}% change",
            self.config.momentum.min_volume_24h_usd,
            self.config.momentum.min_liquidity_usd,
            self.config.momentum.min_price_change_24h_pct
        );

        loop {
            // 1. Scan for trending tokens
            match self.scan_trending_tokens().await {
                Ok(candidates) => {
                    info!("📊 Found {} trending tokens", candidates.len());

                    // 2. Analyze each candidate
                    for candidate in candidates {
                        if let Err(e) = self.analyze_and_trade(candidate).await {
                            warn!("Failed to analyze candidate: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to scan trending tokens: {}", e);
                }
            }

            // 3. Monitor active positions
            if let Err(e) = self.monitor_positions().await {
                error!("Failed to monitor positions: {}", e);
            }

            // Wait before next scan
            sleep(Duration::from_secs(self.scan_interval_secs)).await;
        }
    }

    /// Scan DexScreener for trending Solana tokens
    async fn scan_trending_tokens(&self) -> Result<Vec<MomentumCandidate>> {
        info!("🔍 Scanning DexScreener for trending tokens...");

        let client = reqwest::Client::new();

        // Fetch trending pairs from DexScreener
        let url = "https://api.dexscreener.com/latest/dex/pairs/solana";
        let response = client.get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .context("Failed to fetch trending tokens")?;

        let data: DexScreenerResponse = response.json().await
            .context("Failed to parse DexScreener response")?;

        let mut candidates = Vec::new();

        if let Some(pairs) = data.pairs {
            let mut ranked_pairs: Vec<&DexScreenerPair> = pairs.iter().collect();
            ranked_pairs.sort_by(|a, b| {
                let a_volume = a.volume.as_ref().map(|v| v.h24).unwrap_or(0.0);
                let b_volume = b.volume.as_ref().map(|v| v.h24).unwrap_or(0.0);
                b_volume
                    .partial_cmp(&a_volume)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            for pair in ranked_pairs.iter().take(self.config.momentum.max_candidates) {
                // Filter criteria
                let price_usd = pair.price_usd
                    .as_ref()
                    .and_then(|p| p.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let volume_24h = pair.volume
                    .as_ref()
                    .map(|v| v.h24)
                    .unwrap_or(0.0);

                let price_change_24h = pair.price_change
                    .as_ref()
                    .map(|pc| pc.h24)
                    .unwrap_or(0.0);

                let liquidity_usd = pair.liquidity
                    .as_ref()
                    .and_then(|l| l.usd)
                    .unwrap_or(0.0);

                if pair.chain_id != "solana" {
                    continue;
                }

                // Filter: Must have minimum liquidity and positive price change
                // Convert USD liquidity to SOL (rough estimate: 1 SOL = $200)
                let liquidity_sol = liquidity_usd / 200.0;
                if liquidity_sol >= self.config.sniper.min_liquidity_sol
                    && price_change_24h >= self.config.momentum.min_price_change_24h_pct
                    && volume_24h >= self.config.momentum.min_volume_24h_usd
                    && liquidity_usd >= self.config.momentum.min_liquidity_usd
                {

                    candidates.push(MomentumCandidate {
                        token_mint: pair.base_token.address.clone(),
                        symbol: pair.base_token.symbol.clone(),
                        price_usd,
                        volume_24h,
                        price_change_24h,
                        liquidity_usd,
                        market_cap: pair.market_cap,
                    });

                    debug!("✅ Candidate: {} | Price: ${:.6} | 24h: {:.1}% | Vol: ${:.0} | Liq: ${:.0}",
                        pair.base_token.symbol,
                        price_usd,
                        price_change_24h,
                        volume_24h,
                        liquidity_usd
                    );
                }
            }
        }

        Ok(candidates)
    }

    /// Analyze candidate and execute trade if bullish pattern detected
    async fn analyze_and_trade(&self, candidate: MomentumCandidate) -> Result<()> {
        // Skip if already in position
        {
            let positions = self.active_positions.read().await;
            if positions.contains_key(&candidate.token_mint) {
                return Ok(());
            }
        }

        // Get or create pattern detector for this token
        let mut detectors = self.pattern_detectors.write().await;
        let detector = detectors.entry(candidate.token_mint.clone())
            .or_insert_with(|| PatternDetector::new(50));

        // Add current price point
        detector.add_price_point(
            chrono::Utc::now().timestamp(),
            candidate.price_usd,
            candidate.volume_24h,
        );

        // Detect pattern
        let pattern_result = detector.detect_pattern();

        info!("📈 {} | Pattern: {:?} | Confidence: {}% | Rec: {:?}",
            candidate.symbol,
            pattern_result.pattern,
            pattern_result.confidence,
            pattern_result.trade_recommendation
        );

        // Execute trade if strong bullish signal
        if matches!(pattern_result.trade_recommendation, TradeRecommendation::StrongBuy | TradeRecommendation::Buy)
            && pattern_result.confidence >= 60 {

            info!("🎯 BULLISH SIGNAL: {} | {}", candidate.symbol, pattern_result.description);

            // Verify Jupiter route exists
            match self.verify_jupiter_route(&candidate).await {
                Ok(true) => {
                    // Execute entry trade
                    if let Err(e) = self.execute_entry(&candidate, &pattern_result).await {
                        error!("Failed to execute entry for {}: {}", candidate.symbol, e);
                    }
                }
                Ok(false) => {
                    info!("⏭️  Skipping {} - no Jupiter route", candidate.symbol);
                }
                Err(e) => {
                    warn!("Failed to verify route for {}: {}", candidate.symbol, e);
                }
            }
        }

        Ok(())
    }

    /// Verify that Jupiter can route this token
    async fn verify_jupiter_route(&self, candidate: &MomentumCandidate) -> Result<bool> {
        let amount_lamports = (self.config.sniper.snipe_amount_sol * 1e9) as u64;

        match validate_entry_exit_routes(
            &self.jupiter,
            &self.config,
            SOL_MINT,
            &candidate.token_mint,
            amount_lamports,
            self.config.risk.slippage_bps,
        )
        .await
        {
            Ok(validation) => {
                info!(
                    "✅ Route validated for {} (impact {}bps)",
                    candidate.symbol,
                    validation.impact_bps
                );
                Ok(true)
            }
            Err(e) => {
                warn!("Route validation failed for {}: {}", candidate.symbol, e);
                Ok(false)
            }
        }
    }

    /// Execute entry trade
    async fn execute_entry(
        &self,
        candidate: &MomentumCandidate,
        pattern: &crate::strategies::PatternResult,
    ) -> Result<()> {
        info!("🚀 EXECUTING ENTRY: {} at ${:.6}", candidate.symbol, candidate.price_usd);

        // Calculate stop loss and take profit
        let stop_loss = candidate.price_usd * (1.0 - self.config.risk.stop_loss_percent);
        let take_profit = candidate.price_usd * (1.0 + self.config.risk.take_profit_percent);

        info!("   Entry: ${:.6} | SL: ${:.6} (-{:.0}%) | TP: ${:.6} (+{:.0}%)",
            candidate.price_usd,
            stop_loss,
            self.config.risk.stop_loss_percent * 100.0,
            take_profit,
            self.config.risk.take_profit_percent * 100.0
        );

        // Execute swap via Jupiter
        match self.jupiter.buy_with_sol(
            &self.solana,
            &self.config,
            &candidate.token_mint,
            self.config.sniper.snipe_amount_sol,
        ).await {
            Ok(result) => {
                info!("✅ ENTRY EXECUTED: {} | Sig: {:?}", candidate.symbol, result.signature);

                // Record position
                let position = Position {
                    token_mint: candidate.token_mint.clone(),
                    entry_price: candidate.price_usd,
                    amount: self.config.sniper.snipe_amount_sol,
                    stop_loss,
                    take_profit,
                    entry_time: std::time::Instant::now(),
                };

                let mut positions = self.active_positions.write().await;
                positions.insert(candidate.token_mint.clone(), position);

                Ok(())
            }
            Err(e) => {
                error!("❌ ENTRY FAILED: {} | Error: {}", candidate.symbol, e);
                Err(e)
            }
        }
    }

    /// Monitor active positions for stop loss / take profit
    async fn monitor_positions(&self) -> Result<()> {
        let positions: Vec<Position> = {
            let pos_guard = self.active_positions.read().await;
            pos_guard.values().cloned().collect()
        };

        if positions.is_empty() {
            return Ok(());
        }

        info!("📊 Monitoring {} active positions...", positions.len());

        for position in positions {
            // Get current price
            match self.get_current_price(&position.token_mint).await {
                Ok(current_price) => {
                    let pnl_percent = ((current_price - position.entry_price) / position.entry_price) * 100.0;

                    debug!("   {} | Entry: ${:.6} | Current: ${:.6} | PnL: {:.1}%",
                        &position.token_mint[..8],
                        position.entry_price,
                        current_price,
                        pnl_percent
                    );

                    // Check stop loss
                    if current_price <= position.stop_loss {
                        warn!("🛑 STOP LOSS HIT: {} at ${:.6} (-{:.1}%)",
                            &position.token_mint[..8],
                            current_price,
                            pnl_percent.abs()
                        );

                        if let Err(e) = self.execute_exit(&position, current_price, "Stop Loss").await {
                            error!("Failed to execute stop loss: {}", e);
                        }
                    }
                    // Check take profit
                    else if current_price >= position.take_profit {
                        info!("🎯 TAKE PROFIT HIT: {} at ${:.6} (+{:.1}%)",
                            &position.token_mint[..8],
                            current_price,
                            pnl_percent
                        );

                        if let Err(e) = self.execute_exit(&position, current_price, "Take Profit").await {
                            error!("Failed to execute take profit: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to get price for {}: {}", &position.token_mint[..8], e);
                }
            }
        }

        Ok(())
    }

    /// Get current price for a token
    async fn get_current_price(&self, token_mint: &str) -> Result<f64> {
        // Use Jupiter's built-in get_token_price
        self.jupiter.get_token_price(token_mint).await
    }

    /// Execute exit trade
    async fn execute_exit(&self, position: &Position, exit_price: f64, _reason: &str) -> Result<()> {
        info!("🚪 EXECUTING EXIT: {} | Reason: {}", &position.token_mint[..8], _reason);

        // Get token balance
        use solana_sdk::pubkey::Pubkey;
        use std::str::FromStr;

        let token_pubkey = Pubkey::from_str(&position.token_mint)?;
        let token_balance = self.solana.get_token_balance(&token_pubkey).await?;
        let token_amount_lamports = (token_balance * 1e9) as u64;

        match self.jupiter.sell_for_sol(
            &self.solana,
            &self.config,
            &position.token_mint,
            token_amount_lamports,
        ).await {
            Ok(result) => {
                info!("✅ EXIT EXECUTED: {} | Sig: {:?}", &position.token_mint[..8], result.signature);

                // Remove from active positions
                let mut positions = self.active_positions.write().await;
                positions.remove(&position.token_mint);

                // Calculate final PnL
                let pnl_percent = ((exit_price - position.entry_price) / position.entry_price) * 100.0;
                info!("💰 Final PnL: {:.2}%", pnl_percent);

                Ok(())
            }
            Err(e) => {
                error!("❌ EXIT FAILED: {} | Error: {}", &position.token_mint[..8], e);
                Err(e)
            }
        }
    }

    /// Stop the trader gracefully
    pub async fn stop(&self) -> Result<()> {
        info!("⏸️  Stopping Momentum Trader...");

        // Close all positions
        let positions: Vec<Position> = {
            let pos_guard = self.active_positions.read().await;
            pos_guard.values().cloned().collect()
        };

        for position in positions {
            if let Ok(price) = self.get_current_price(&position.token_mint).await {
                let _ = self.execute_exit(&position, price, "Shutdown").await;
            }
        }

        info!("✅ Momentum Trader stopped");
        Ok(())
    }
}
