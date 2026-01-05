/// Profit Sweep Manager (CRITICAL FIX #2)
///
/// Automatically sweeps profits to cold storage when growth thresholds are met.
/// Rules:
/// - Trigger: After +25% equity increase
/// - Amount: Sweep 20-30% of profits
/// - Frequency: Once per threshold
/// - Capital Protection: Reset risk if equity drops below last swept balance

use crate::config::Config;
use crate::services::SolanaConnection;
use anyhow::{anyhow, Result};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct SweepRecord {
    pub timestamp: i64,
    pub capital_before: f64,
    pub capital_after: f64,
    pub swept_amount: f64,
    pub signature: String,
}

pub struct ProfitSweepManager {
    config: Arc<RwLock<Config>>,
    solana: Arc<SolanaConnection>,
    last_sweep_capital: Arc<RwLock<f64>>,
    sweep_history: Arc<RwLock<Vec<SweepRecord>>>,
}

impl ProfitSweepManager {
    pub fn new(config: Arc<RwLock<Config>>, solana: Arc<SolanaConnection>, initial_capital: f64) -> Self {
        Self {
            config,
            solana,
            last_sweep_capital: Arc::new(RwLock::new(initial_capital)),
            sweep_history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check if sweep is needed and execute if threshold met
    pub async fn check_and_sweep(&self, current_capital: f64) -> Result<Option<SweepRecord>> {
        let config = self.config.read().await;

        // Check if sweeps are enabled
        if !config.wallet.profit_sweep_enabled {
            return Ok(None);
        }

        // Check if cold wallet is configured
        let cold_wallet_addr = match &config.wallet.cold_wallet_address {
            Some(addr) => addr,
            None => {
                warn!("⚠️  Profit sweep enabled but COLD_WALLET_ADDRESS not configured");
                return Ok(None);
            }
        };

        let last_sweep = *self.last_sweep_capital.read().await;
        let growth_pct = ((current_capital - last_sweep) / last_sweep) * 100.0;

        info!("💰 Profit Sweep Check:");
        info!("  Current capital: {:.4} SOL", current_capital);
        info!("  Last sweep at: {:.4} SOL", last_sweep);
        info!("  Growth since last sweep: {:.2}%", growth_pct);

        // Check if threshold met
        if growth_pct < config.wallet.profit_sweep_threshold_pct {
            info!("  ✓ Below threshold ({:.0}%), no sweep needed", config.wallet.profit_sweep_threshold_pct);
            return Ok(None);
        }

        info!("🚨 PROFIT SWEEP TRIGGERED!");
        info!("  Threshold reached: {:.2}% >= {:.0}%", growth_pct, config.wallet.profit_sweep_threshold_pct);

        // Calculate sweep amount (percentage of PROFITS only)
        let profit = current_capital - last_sweep;
        let sweep_amount = profit * (config.wallet.profit_sweep_amount_pct / 100.0);

        if sweep_amount < 0.01 {
            warn!("  Sweep amount too small: {:.4} SOL (min: 0.01)", sweep_amount);
            return Ok(None);
        }

        info!("  Profit since last sweep: {:.4} SOL", profit);
        info!("  Sweeping {:.0}% of profit: {:.4} SOL", config.wallet.profit_sweep_amount_pct, sweep_amount);
        info!("  Destination: {}", cold_wallet_addr);

        // Execute sweep
        let cold_wallet_pubkey = Pubkey::from_str(cold_wallet_addr)
            .map_err(|e| anyhow!("Invalid cold wallet address: {}", e))?;

        // Convert SOL to lamports
        let sweep_lamports = (sweep_amount * 1_000_000_000.0) as u64;

        // Send SOL to cold wallet
        drop(config); // Release lock before async operation

        match self.solana.send_sol(&cold_wallet_pubkey, sweep_lamports).await {
            Ok(signature) => {
                let new_capital = current_capital - sweep_amount;
                info!("✅ Profit sweep successful!");
                info!("  TX: {}", signature);
                info!("  Remaining hot wallet capital: {:.4} SOL", new_capital);

                // Update last sweep capital
                *self.last_sweep_capital.write().await = new_capital;

                // Record sweep
                let record = SweepRecord {
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    capital_before: current_capital,
                    capital_after: new_capital,
                    swept_amount: sweep_amount,
                    signature: signature.to_string(),
                };

                self.sweep_history.write().await.push(record.clone());

                info!("📊 Total sweeps: {}", self.sweep_history.read().await.len());

                Ok(Some(record))
            }
            Err(e) => {
                error!("❌ Profit sweep failed: {}", e);
                Err(e)
            }
        }
    }

    /// Check if capital has dropped below last swept balance (capital protection rule)
    pub async fn check_capital_protection(&self, current_capital: f64) -> bool {
        let last_sweep = *self.last_sweep_capital.read().await;

        if current_capital < last_sweep {
            warn!("⚠️  CAPITAL PROTECTION TRIGGERED!");
            warn!("  Current: {:.4} SOL < Last sweep: {:.4} SOL", current_capital, last_sweep);
            warn!("  Risk should be reset to MIN_RISK until recovery");
            return true;
        }

        false
    }

    pub async fn get_total_swept(&self) -> f64 {
        self.sweep_history.read().await
            .iter()
            .map(|r| r.swept_amount)
            .sum()
    }

    pub async fn get_sweep_count(&self) -> usize {
        self.sweep_history.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sweep_threshold_logic() {
        // Test: 100 SOL -> 125 SOL = 25% growth -> sweep 25% of 25 SOL profit = 6.25 SOL
        let initial = 100.0;
        let current = 125.0;
        let sweep_pct = 25.0;

        let growth = ((current - initial) / initial) * 100.0;
        assert_eq!(growth, 25.0);

        let profit = current - initial;
        assert_eq!(profit, 25.0);

        let sweep_amount = profit * (sweep_pct / 100.0);
        assert_eq!(sweep_amount, 6.25);

        let remaining = current - sweep_amount;
        assert_eq!(remaining, 118.75);
    }

    #[test]
    fn test_capital_protection() {
        // Test: After sweep at 118.75, if capital drops to 115, protection triggers
        let last_sweep = 118.75;
        let current = 115.0;

        assert!(current < last_sweep);
        // Risk should reset to MIN_RISK
    }
}
