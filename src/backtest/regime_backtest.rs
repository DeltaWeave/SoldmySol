// ✅ STRATEGY VALIDATION: Regime Classification & Edge-vs-Cost Gate Backtest
// This module validates that our advanced strategies actually improve performance

use crate::backtest::{BacktestResult, HistoricalTrade};
use crate::strategies::{RegimeClassifier, VolumeAnalyzer, VolumeProfile, Regime};
use crate::models::TokenPool;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeBacktestResult {
    pub baseline: BacktestMetrics,
    pub with_regime: BacktestMetrics,
    pub with_edge_gate: BacktestMetrics,
    pub with_both: BacktestMetrics,
    pub regime_breakdown: RegimeBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestMetrics {
    pub total_trades: usize,
    pub winning_trades: usize,
    pub win_rate: f64,
    pub total_pnl_percent: f64,
    pub avg_trade_pnl: f64,
    pub max_drawdown: f64,
    pub sharpe_ratio: f64,
    pub trades_filtered: usize, // How many trades were rejected
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeBreakdown {
    pub new_pair_trades: usize,
    pub new_pair_win_rate: f64,
    pub momentum_trades: usize,
    pub momentum_win_rate: f64,
    pub mean_reversion_trades: usize,
    pub mean_reversion_win_rate: f64,
    pub low_conviction_trades: usize,
    pub low_conviction_win_rate: f64,
}

pub struct RegimeBacktester {
    initial_capital: f64,
}

impl RegimeBacktester {
    pub fn new(initial_capital: f64) -> Self {
        Self { initial_capital }
    }

    /// Run comprehensive backtest comparing strategies
    pub fn run(&self, historical_data: Vec<HistoricalTrade>) -> Result<RegimeBacktestResult> {
        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║          REGIME CLASSIFICATION BACKTEST                        ║");
        println!("╚════════════════════════════════════════════════════════════════╝\n");
        println!("Historical trades to analyze: {}\n", historical_data.len());

        // Run 4 different strategies
        let baseline = self.run_baseline(&historical_data)?;
        let with_regime = self.run_with_regime(&historical_data)?;
        let with_edge_gate = self.run_with_edge_gate(&historical_data)?;
        let with_both = self.run_with_both_filters(&historical_data)?;
        let regime_breakdown = self.analyze_regime_performance(&historical_data)?;

        Ok(RegimeBacktestResult {
            baseline,
            with_regime,
            with_edge_gate,
            with_both,
            regime_breakdown,
        })
    }

    /// Baseline: Take every trade that passes basic safety checks
    fn run_baseline(&self, data: &[HistoricalTrade]) -> Result<BacktestMetrics> {
        let mut capital = self.initial_capital;
        let mut trades = 0;
        let mut wins = 0;
        let mut total_pnl = 0.0;
        let mut pnls = Vec::new();

        for trade in data {
            // Basic safety filter only
            if trade.liquidity_sol < 5.0 || trade.liquidity_sol > 50.0 {
                continue;
            }

            trades += 1;
            let pnl_sol = (trade.pnl_percent / 100.0) * 0.1; // 0.1 SOL per trade
            capital += pnl_sol;
            total_pnl += pnl_sol;
            pnls.push(trade.pnl_percent);

            if trade.pnl_percent > 0.0 {
                wins += 1;
            }
        }

        Ok(BacktestMetrics {
            total_trades: trades,
            winning_trades: wins,
            win_rate: if trades > 0 { (wins as f64 / trades as f64) * 100.0 } else { 0.0 },
            total_pnl_percent: ((capital - self.initial_capital) / self.initial_capital) * 100.0,
            avg_trade_pnl: if trades > 0 { total_pnl / trades as f64 } else { 0.0 },
            max_drawdown: Self::calculate_max_drawdown(&pnls),
            sharpe_ratio: Self::calculate_sharpe(&pnls),
            trades_filtered: 0,
        })
    }

    /// With regime classification: Use dynamic stops per regime
    fn run_with_regime(&self, data: &[HistoricalTrade]) -> Result<BacktestMetrics> {
        let mut capital = self.initial_capital;
        let mut trades = 0;
        let mut wins = 0;
        let mut total_pnl = 0.0;
        let mut pnls = Vec::new();
        let mut filtered = 0;

        for trade in data {
            if trade.liquidity_sol < 5.0 || trade.liquidity_sol > 50.0 {
                continue;
            }

            // Build token pool
            let pool = TokenPool {
                token_address: trade.token_symbol.clone(),
                token_symbol: trade.token_symbol.clone(),
                pair_address: String::new(),
                liquidity_sol: trade.liquidity_sol,
                liquidity_usd: trade.liquidity_sol * 100.0,
                price_usd: trade.entry_price,
                volume_24h: trade.volume_24h,
                price_change_24h: 0.0,
                created_at: Some(trade.timestamp),
            };

            // Classify regime
            let volume_profile = VolumeAnalyzer::analyze_volume_profile(
                pool.volume_24h,
                pool.liquidity_sol,
            );

            let regime = RegimeClassifier::classify(&pool, &volume_profile, None);

            // Skip LowConviction trades
            if matches!(regime.regime, Regime::LowConviction) {
                filtered += 1;
                continue;
            }

            let playbook = regime.playbook;

            // Apply regime-specific stops
            let mut actual_pnl = trade.pnl_percent;

            // Check if would have hit stop loss
            if actual_pnl < -(playbook.stop_loss_pct as f64) {
                actual_pnl = -(playbook.stop_loss_pct as f64);
            }

            // Check if would have hit take profit
            if actual_pnl > playbook.take_profit_pcts[0] as f64 {
                actual_pnl = playbook.take_profit_pcts[0] as f64;
            }

            trades += 1;
            let pnl_sol = (actual_pnl / 100.0) * 0.1;
            capital += pnl_sol;
            total_pnl += pnl_sol;
            pnls.push(actual_pnl);

            if actual_pnl > 0.0 {
                wins += 1;
            }
        }

        Ok(BacktestMetrics {
            total_trades: trades,
            winning_trades: wins,
            win_rate: if trades > 0 { (wins as f64 / trades as f64) * 100.0 } else { 0.0 },
            total_pnl_percent: ((capital - self.initial_capital) / self.initial_capital) * 100.0,
            avg_trade_pnl: if trades > 0 { total_pnl / trades as f64 } else { 0.0 },
            max_drawdown: Self::calculate_max_drawdown(&pnls),
            sharpe_ratio: Self::calculate_sharpe(&pnls),
            trades_filtered: filtered,
        })
    }

    /// With edge-vs-cost gate: Only take trades with 2x+ edge
    fn run_with_edge_gate(&self, data: &[HistoricalTrade]) -> Result<BacktestMetrics> {
        let mut capital = self.initial_capital;
        let mut trades = 0;
        let mut wins = 0;
        let mut total_pnl = 0.0;
        let mut pnls = Vec::new();
        let mut filtered = 0;

        for trade in data {
            if trade.liquidity_sol < 5.0 || trade.liquidity_sol > 50.0 {
                continue;
            }

            // Simulate edge calculation
            // Higher volume quality = higher expected edge
            let volume_profile = VolumeAnalyzer::analyze_volume_profile(
                trade.volume_24h,
                trade.liquidity_sol,
            );

            let expected_edge_bps = volume_profile.quality_score as f64 * 10.0; // 0-1000 bps
            let total_costs_bps = 150.0; // Slippage + fees + price impact

            // Edge-vs-cost gate: must be 2x+
            if expected_edge_bps < (total_costs_bps * 2.0) {
                filtered += 1;
                continue;
            }

            trades += 1;
            let pnl_sol = (trade.pnl_percent / 100.0) * 0.1;
            capital += pnl_sol;
            total_pnl += pnl_sol;
            pnls.push(trade.pnl_percent);

            if trade.pnl_percent > 0.0 {
                wins += 1;
            }
        }

        Ok(BacktestMetrics {
            total_trades: trades,
            winning_trades: wins,
            win_rate: if trades > 0 { (wins as f64 / trades as f64) * 100.0 } else { 0.0 },
            total_pnl_percent: ((capital - self.initial_capital) / self.initial_capital) * 100.0,
            avg_trade_pnl: if trades > 0 { total_pnl / trades as f64 } else { 0.0 },
            max_drawdown: Self::calculate_max_drawdown(&pnls),
            sharpe_ratio: Self::calculate_sharpe(&pnls),
            trades_filtered: filtered,
        })
    }

    /// With both filters combined (current production strategy)
    fn run_with_both_filters(&self, data: &[HistoricalTrade]) -> Result<BacktestMetrics> {
        let mut capital = self.initial_capital;
        let mut trades = 0;
        let mut wins = 0;
        let mut total_pnl = 0.0;
        let mut pnls = Vec::new();
        let mut filtered = 0;

        for trade in data {
            if trade.liquidity_sol < 5.0 || trade.liquidity_sol > 50.0 {
                continue;
            }

            let pool = TokenPool {
                token_address: trade.token_symbol.clone(),
                token_symbol: trade.token_symbol.clone(),
                pair_address: String::new(),
                liquidity_sol: trade.liquidity_sol,
                liquidity_usd: trade.liquidity_sol * 100.0,
                price_usd: trade.entry_price,
                volume_24h: trade.volume_24h,
                price_change_24h: 0.0,
                created_at: Some(trade.timestamp),
            };

            let volume_profile = VolumeAnalyzer::analyze_volume_profile(
                pool.volume_24h,
                pool.liquidity_sol,
            );

            // Filter 1: Regime classification
            let regime = RegimeClassifier::classify(&pool, &volume_profile, None);
            if matches!(regime.regime, Regime::LowConviction) {
                filtered += 1;
                continue;
            }

            // Filter 2: Edge-vs-cost gate
            let expected_edge_bps = volume_profile.quality_score as f64 * 10.0;
            let total_costs_bps = 150.0;

            if expected_edge_bps < (total_costs_bps * 2.0) {
                filtered += 1;
                continue;
            }

            let playbook = regime.playbook;

            // Apply regime-specific stops
            let mut actual_pnl = trade.pnl_percent;

            if actual_pnl < -(playbook.stop_loss_pct as f64) {
                actual_pnl = -(playbook.stop_loss_pct as f64);
            }

            if actual_pnl > playbook.take_profit_pcts[0] as f64 {
                actual_pnl = playbook.take_profit_pcts[0] as f64;
            }

            trades += 1;
            let pnl_sol = (actual_pnl / 100.0) * 0.1;
            capital += pnl_sol;
            total_pnl += pnl_sol;
            pnls.push(actual_pnl);

            if actual_pnl > 0.0 {
                wins += 1;
            }
        }

        Ok(BacktestMetrics {
            total_trades: trades,
            winning_trades: wins,
            win_rate: if trades > 0 { (wins as f64 / trades as f64) * 100.0 } else { 0.0 },
            total_pnl_percent: ((capital - self.initial_capital) / self.initial_capital) * 100.0,
            avg_trade_pnl: if trades > 0 { total_pnl / trades as f64 } else { 0.0 },
            max_drawdown: Self::calculate_max_drawdown(&pnls),
            sharpe_ratio: Self::calculate_sharpe(&pnls),
            trades_filtered: filtered,
        })
    }

    /// Analyze performance by regime type
    fn analyze_regime_performance(&self, data: &[HistoricalTrade]) -> Result<RegimeBreakdown> {
        let mut new_pair = (0, 0);
        let mut momentum = (0, 0);
        let mut mean_reversion = (0, 0);
        let mut low_conviction = (0, 0);

        for trade in data {
            let pool = TokenPool {
                token_address: trade.token_symbol.clone(),
                token_symbol: trade.token_symbol.clone(),
                pair_address: String::new(),
                liquidity_sol: trade.liquidity_sol,
                liquidity_usd: trade.liquidity_sol * 100.0,
                price_usd: trade.entry_price,
                volume_24h: trade.volume_24h,
                price_change_24h: 0.0,
                created_at: Some(trade.timestamp),
            };

            let volume_profile = VolumeAnalyzer::analyze_volume_profile(
                pool.volume_24h,
                pool.liquidity_sol,
            );

            let regime = RegimeClassifier::classify(&pool, &volume_profile, None);

            let (trades, wins) = match regime.regime {
                Regime::NewPair => &mut new_pair,
                Regime::Momentum => &mut momentum,
                Regime::MeanReversion => &mut mean_reversion,
                Regime::LowConviction => &mut low_conviction,
            };

            trades.0 += 1;
            if trade.pnl_percent > 0.0 {
                trades.1 += 1;
            }
        }

        Ok(RegimeBreakdown {
            new_pair_trades: new_pair.0,
            new_pair_win_rate: if new_pair.0 > 0 { (new_pair.1 as f64 / new_pair.0 as f64) * 100.0 } else { 0.0 },
            momentum_trades: momentum.0,
            momentum_win_rate: if momentum.0 > 0 { (momentum.1 as f64 / momentum.0 as f64) * 100.0 } else { 0.0 },
            mean_reversion_trades: mean_reversion.0,
            mean_reversion_win_rate: if mean_reversion.0 > 0 { (mean_reversion.1 as f64 / mean_reversion.0 as f64) * 100.0 } else { 0.0 },
            low_conviction_trades: low_conviction.0,
            low_conviction_win_rate: if low_conviction.0 > 0 { (low_conviction.1 as f64 / low_conviction.0 as f64) * 100.0 } else { 0.0 },
        })
    }

    fn calculate_max_drawdown(pnls: &[f64]) -> f64 {
        if pnls.is_empty() {
            return 0.0;
        }

        let mut peak = 0.0;
        let mut max_dd = 0.0;
        let mut cumulative = 0.0;

        for pnl in pnls {
            cumulative += pnl;
            if cumulative > peak {
                peak = cumulative;
            }
            let dd = peak - cumulative;
            if dd > max_dd {
                max_dd = dd;
            }
        }

        max_dd
    }

    fn calculate_sharpe(returns: &[f64]) -> f64 {
        if returns.is_empty() {
            return 0.0;
        }

        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;

        let std_dev = variance.sqrt();

        if std_dev == 0.0 {
            0.0
        } else {
            mean / std_dev
        }
    }

    pub fn print_results(&self, results: &RegimeBacktestResult) {
        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║            REGIME BACKTEST RESULTS                             ║");
        println!("╚════════════════════════════════════════════════════════════════╝\n");

        // Comparison table
        println!("📊 STRATEGY COMPARISON:\n");
        println!("┌────────────────────┬───────┬──────────┬─────────┬──────────┬──────────┐");
        println!("│ Strategy           │ Trades│ Win Rate │ Avg PnL │ Total % │ Filtered │");
        println!("├────────────────────┼───────┼──────────┼─────────┼──────────┼──────────┤");

        println!("│ Baseline           │ {:5} │ {:6.1}%  │ {:7.4} │ {:+7.2}% │    {:5} │",
            results.baseline.total_trades,
            results.baseline.win_rate,
            results.baseline.avg_trade_pnl,
            results.baseline.total_pnl_percent,
            results.baseline.trades_filtered);

        println!("│ + Regime Class.    │ {:5} │ {:6.1}%  │ {:7.4} │ {:+7.2}% │    {:5} │",
            results.with_regime.total_trades,
            results.with_regime.win_rate,
            results.with_regime.avg_trade_pnl,
            results.with_regime.total_pnl_percent,
            results.with_regime.trades_filtered);

        println!("│ + Edge-vs-Cost     │ {:5} │ {:6.1}%  │ {:7.4} │ {:+7.2}% │    {:5} │",
            results.with_edge_gate.total_trades,
            results.with_edge_gate.win_rate,
            results.with_edge_gate.avg_trade_pnl,
            results.with_edge_gate.total_pnl_percent,
            results.with_edge_gate.trades_filtered);

        println!("│ + BOTH (Current)   │ {:5} │ {:6.1}%  │ {:7.4} │ {:+7.2}% │    {:5} │",
            results.with_both.total_trades,
            results.with_both.win_rate,
            results.with_both.avg_trade_pnl,
            results.with_both.total_pnl_percent,
            results.with_both.trades_filtered);

        println!("└────────────────────┴───────┴──────────┴─────────┴──────────┴──────────┘\n");

        // Regime breakdown
        println!("🎯 REGIME PERFORMANCE BREAKDOWN:\n");
        println!("┌────────────────────┬───────┬──────────┐");
        println!("│ Regime Type        │ Trades│ Win Rate │");
        println!("├────────────────────┼───────┼──────────┤");
        println!("│ New Pair           │ {:5} │ {:6.1}%  │",
            results.regime_breakdown.new_pair_trades,
            results.regime_breakdown.new_pair_win_rate);
        println!("│ Momentum           │ {:5} │ {:6.1}%  │",
            results.regime_breakdown.momentum_trades,
            results.regime_breakdown.momentum_win_rate);
        println!("│ Mean Reversion     │ {:5} │ {:6.1}%  │",
            results.regime_breakdown.mean_reversion_trades,
            results.regime_breakdown.mean_reversion_win_rate);
        println!("│ Low Conviction     │ {:5} │ {:6.1}%  │",
            results.regime_breakdown.low_conviction_trades,
            results.regime_breakdown.low_conviction_win_rate);
        println!("└────────────────────┴───────┴──────────┘\n");

        // Analysis
        println!("📈 ANALYSIS:\n");

        let regime_improvement = results.with_regime.win_rate - results.baseline.win_rate;
        let edge_improvement = results.with_edge_gate.win_rate - results.baseline.win_rate;
        let total_improvement = results.with_both.win_rate - results.baseline.win_rate;

        println!("Regime Classification Impact: {:+.1}% win rate", regime_improvement);
        println!("Edge-vs-Cost Gate Impact:     {:+.1}% win rate", edge_improvement);
        println!("Combined Strategy Impact:     {:+.1}% win rate\n", total_improvement);

        if total_improvement > 5.0 {
            println!("✅ VALIDATION SUCCESSFUL:");
            println!("   Advanced filters significantly improve performance!");
            println!("   Safe to use in production.\n");
        } else if total_improvement > 0.0 {
            println!("⚠️  VALIDATION PARTIAL:");
            println!("   Filters improve performance slightly.");
            println!("   Consider further tuning before scaling.\n");
        } else {
            println!("❌ VALIDATION FAILED:");
            println!("   Filters reduce performance!");
            println!("   DO NOT use these strategies in production.\n");
        }
    }
}
