use crate::config::Config;
use crate::models::{SafetyCheckResult, TokenPool};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTrade {
    pub timestamp: i64,
    pub token_symbol: String,
    pub entry_price: f64,
    pub exit_price: f64,
    pub duration_minutes: i64,
    pub pnl_percent: f64,
    pub liquidity_sol: f64,
    pub volume_24h: f64,
}

#[derive(Debug, Clone)]
pub struct BacktestPosition {
    pub token_symbol: String,
    pub entry_price: f64,
    pub entry_time: i64,
    pub amount_sol: f64,
    pub tokens_bought: f64,
    pub highest_price: f64,
}

#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub total_pnl: f64,
    pub total_pnl_percent: f64,
    pub best_trade: f64,
    pub worst_trade: f64,
    pub avg_trade_pnl: f64,
    pub avg_hold_time_minutes: f64,
    pub max_drawdown: f64,
    pub sharpe_ratio: f64,
    pub final_capital: f64,
}

pub struct Backtester {
    config: Config,
    initial_capital: f64,
    current_capital: f64,
    daily_pnl: f64,
    daily_start_capital: f64,
    positions: HashMap<String, BacktestPosition>,
    completed_trades: Vec<CompletedTrade>,
    equity_curve: Vec<(i64, f64)>,
}

#[derive(Debug, Clone)]
pub struct CompletedTrade {
    pub token_symbol: String,
    pub entry_time: i64,
    pub exit_time: i64,
    pub pnl_sol: f64,
    pub pnl_percent: f64,
    pub hold_time_minutes: i64,
}

impl Backtester {
    pub fn new(config: Config) -> Self {
        let initial_capital = config.capital.initial;

        Self {
            config,
            initial_capital,
            current_capital: initial_capital,
            daily_pnl: 0.0,
            daily_start_capital: initial_capital,
            positions: HashMap::new(),
            completed_trades: Vec::new(),
            equity_curve: Vec::new(),
        }
    }

    /// Run backtest on historical data
    pub fn run(&mut self, historical_data: Vec<HistoricalTrade>) -> Result<BacktestResult> {
        println!("\n🔄 Starting Backtest...\n");
        println!("Initial Capital: {} SOL", self.initial_capital);
        println!("Total Historical Trades: {}\n", historical_data.len());

        for trade_data in historical_data {
            // Simulate token pool
            let pool = TokenPool {
                token_address: format!("backtest_{}", trade_data.token_symbol),
                token_symbol: trade_data.token_symbol.clone(),
                pair_address: "backtest_pair".to_string(),
                liquidity_sol: trade_data.liquidity_sol,
                liquidity_usd: trade_data.liquidity_sol * 100.0, // Approximate
                price_usd: trade_data.entry_price,
                volume_24h: trade_data.volume_24h,
                price_change_24h: 0.0,
                created_at: Some(trade_data.timestamp),
            };

            // Evaluate if we would have taken this trade
            if self.should_take_trade(&pool) {
                self.enter_trade(&pool, trade_data.timestamp)?;

                // Simulate price movement and exit
                self.simulate_price_action(&pool, &trade_data)?;
            }

            // Record equity
            self.equity_curve.push((trade_data.timestamp, self.current_capital));
        }

        // Calculate final results
        self.calculate_results()
    }

    fn should_take_trade(&self, pool: &TokenPool) -> bool {
        // Apply same safety checks as live bot
        let checks = self.run_safety_checks(pool);
        if !checks.passed {
            return false;
        }

        // Check balance
        let required_amount = self.config.sniper.snipe_amount_sol + 0.01;
        if self.current_capital < required_amount {
            return false;
        }

        // Check concurrent positions
        if self.positions.len() >= self.config.risk.max_concurrent_positions {
            return false;
        }

        // Check daily loss limit
        if self.daily_pnl < -(self.daily_start_capital * self.config.risk.max_daily_loss) {
            return false;
        }

        true
    }

    fn run_safety_checks(&self, pool: &TokenPool) -> SafetyCheckResult {
        let mut reasons = Vec::new();

        // Liquidity checks
        if pool.liquidity_sol < self.config.sniper.min_liquidity_sol {
            reasons.push(format!("Low liquidity: {:.2} SOL", pool.liquidity_sol));
        }

        if pool.liquidity_sol > self.config.sniper.max_liquidity_sol {
            reasons.push(format!("High liquidity: {:.2} SOL", pool.liquidity_sol));
        }

        // Age check
        if let Some(created_at) = pool.created_at {
            let age_ms = chrono::Utc::now().timestamp_millis() - created_at;
            let age_minutes = age_ms / 1000 / 60;

            if age_minutes > self.config.sniper.max_age_minutes as i64 {
                reasons.push(format!("Token too old: {} minutes", age_minutes));
            }
        }

        // Volume check
        if pool.volume_24h < 1000.0 {
            reasons.push(format!("Low volume: ${:.2}", pool.volume_24h));
        }

        SafetyCheckResult {
            passed: reasons.is_empty(),
            reasons,
        }
    }

    fn enter_trade(&mut self, pool: &TokenPool, timestamp: i64) -> Result<()> {
        let entry_price = pool.price_usd;
        let amount_sol = self.config.sniper.snipe_amount_sol;
        let tokens_bought = amount_sol / entry_price;

        let position = BacktestPosition {
            token_symbol: pool.token_symbol.clone(),
            entry_price,
            entry_time: timestamp,
            amount_sol,
            tokens_bought,
            highest_price: entry_price,
        };

        self.positions.insert(pool.token_symbol.clone(), position);

        Ok(())
    }

    fn simulate_price_action(&mut self, pool: &TokenPool, trade_data: &HistoricalTrade) -> Result<()> {
        if let Some(position) = self.positions.get_mut(&pool.token_symbol) {
            // Simulate exit based on historical outcome
            let exit_price = trade_data.exit_price;
            let current_value = position.tokens_bought * exit_price;
            let pnl_sol = current_value - position.amount_sol;
            let pnl_percent = (pnl_sol / position.amount_sol) * 100.0;

            // Check if our strategy would have exited
            let should_exit = {
                let pos = position.clone();
                self.check_exit_conditions(&pos, exit_price, pnl_percent)
            };

            if should_exit {
                self.exit_position(&pool.token_symbol, exit_price, trade_data.timestamp + (trade_data.duration_minutes * 60 * 1000))?;
            }
        }

        Ok(())
    }

    fn check_exit_conditions(&self, position: &BacktestPosition, current_price: f64, pnl_percent: f64) -> bool {
        // Take profit
        if pnl_percent >= (self.config.risk.take_profit_percent * 100.0) {
            return true;
        }

        // Stop loss
        if pnl_percent <= -(self.config.risk.stop_loss_percent * 100.0) {
            return true;
        }

        // Trailing stop
        if self.config.risk.trailing_stop_enabled {
            let drop_from_high = ((position.highest_price - current_price) / position.highest_price) * 100.0;
            if drop_from_high >= (self.config.risk.trailing_stop_percent * 100.0) {
                return true;
            }
        }

        false
    }

    fn exit_position(&mut self, token_symbol: &str, exit_price: f64, exit_time: i64) -> Result<()> {
        if let Some(position) = self.positions.remove(token_symbol) {
            let sol_received = position.tokens_bought * exit_price;
            let pnl_sol = sol_received - position.amount_sol;
            let pnl_percent = (pnl_sol / position.amount_sol) * 100.0;
            let hold_time = (exit_time - position.entry_time) / 1000 / 60; // minutes

            // Update capital
            self.current_capital += pnl_sol;
            self.daily_pnl += pnl_sol;

            // Record trade
            self.completed_trades.push(CompletedTrade {
                token_symbol: token_symbol.to_string(),
                entry_time: position.entry_time,
                exit_time,
                pnl_sol,
                pnl_percent,
                hold_time_minutes: hold_time,
            });

            // Adjust risk parameters
            self.config.adjust_risk_parameters(self.current_capital);
        }

        Ok(())
    }

    fn calculate_results(&self) -> Result<BacktestResult> {
        let total_trades = self.completed_trades.len();

        if total_trades == 0 {
            return Ok(BacktestResult {
                total_trades: 0,
                winning_trades: 0,
                losing_trades: 0,
                win_rate: 0.0,
                total_pnl: 0.0,
                total_pnl_percent: 0.0,
                best_trade: 0.0,
                worst_trade: 0.0,
                avg_trade_pnl: 0.0,
                avg_hold_time_minutes: 0.0,
                max_drawdown: 0.0,
                sharpe_ratio: 0.0,
                final_capital: self.current_capital,
            });
        }

        let winning_trades = self.completed_trades.iter().filter(|t| t.pnl_sol > 0.0).count();
        let losing_trades = total_trades - winning_trades;
        let win_rate = (winning_trades as f64 / total_trades as f64) * 100.0;

        let total_pnl: f64 = self.completed_trades.iter().map(|t| t.pnl_sol).sum();
        let total_pnl_percent = ((self.current_capital - self.initial_capital) / self.initial_capital) * 100.0;

        let best_trade = self.completed_trades.iter().map(|t| t.pnl_percent).fold(f64::NEG_INFINITY, f64::max);
        let worst_trade = self.completed_trades.iter().map(|t| t.pnl_percent).fold(f64::INFINITY, f64::min);

        let avg_trade_pnl = total_pnl / total_trades as f64;
        let avg_hold_time: f64 = self.completed_trades.iter().map(|t| t.hold_time_minutes as f64).sum::<f64>() / total_trades as f64;

        let max_drawdown = self.calculate_max_drawdown();
        let sharpe_ratio = self.calculate_sharpe_ratio();

        Ok(BacktestResult {
            total_trades,
            winning_trades,
            losing_trades,
            win_rate,
            total_pnl,
            total_pnl_percent,
            best_trade,
            worst_trade,
            avg_trade_pnl,
            avg_hold_time_minutes: avg_hold_time,
            max_drawdown,
            sharpe_ratio,
            final_capital: self.current_capital,
        })
    }

    fn calculate_max_drawdown(&self) -> f64 {
        let mut max_drawdown = 0.0;
        let mut peak = self.initial_capital;

        for (_, equity) in &self.equity_curve {
            if *equity > peak {
                peak = *equity;
            }
            let drawdown = ((peak - equity) / peak) * 100.0;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        max_drawdown
    }

    fn calculate_sharpe_ratio(&self) -> f64 {
        if self.completed_trades.is_empty() {
            return 0.0;
        }

        let returns: Vec<f64> = self.completed_trades.iter().map(|t| t.pnl_percent).collect();
        let mean_return: f64 = returns.iter().sum::<f64>() / returns.len() as f64;

        let variance: f64 = returns.iter()
            .map(|r| (r - mean_return).powi(2))
            .sum::<f64>() / returns.len() as f64;

        let std_dev = variance.sqrt();

        if std_dev == 0.0 {
            0.0
        } else {
            mean_return / std_dev
        }
    }

    pub fn print_results(&self, results: &BacktestResult) {
        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║                    BACKTEST RESULTS                            ║");
        println!("╚════════════════════════════════════════════════════════════════╝\n");

        println!("📊 Performance Summary:");
        println!("   Initial Capital:     {:.4} SOL", self.initial_capital);
        println!("   Final Capital:       {:.4} SOL", results.final_capital);
        println!("   Total PnL:           {:.4} SOL ({:+.2}%)", results.total_pnl, results.total_pnl_percent);
        println!();

        println!("📈 Trade Statistics:");
        println!("   Total Trades:        {}", results.total_trades);
        println!("   Winning Trades:      {} ({:.1}%)", results.winning_trades, results.win_rate);
        println!("   Losing Trades:       {}", results.losing_trades);
        println!("   Win Rate:            {:.2}%", results.win_rate);
        println!();

        println!("💰 Trade Metrics:");
        println!("   Best Trade:          {:+.2}%", results.best_trade);
        println!("   Worst Trade:         {:+.2}%", results.worst_trade);
        println!("   Avg Trade PnL:       {:.4} SOL", results.avg_trade_pnl);
        println!("   Avg Hold Time:       {:.1} minutes", results.avg_hold_time_minutes);
        println!();

        println!("📉 Risk Metrics:");
        println!("   Max Drawdown:        {:.2}%", results.max_drawdown);
        println!("   Sharpe Ratio:        {:.3}", results.sharpe_ratio);
        println!();

        println!("🎯 Goal Progress:");
        let progress = ((results.final_capital - self.initial_capital) / (self.config.capital.target - self.initial_capital)) * 100.0;
        println!("   Target:              {} SOL", self.config.capital.target);
        println!("   Progress:            {:.2}%", progress);
        println!();

        // Assessment
        if results.win_rate > 60.0 && results.total_pnl_percent > 50.0 {
            println!("✅ EXCELLENT: Strategy shows strong performance");
        } else if results.win_rate > 50.0 && results.total_pnl_percent > 0.0 {
            println!("✓ GOOD: Strategy is profitable but could be optimized");
        } else if results.total_pnl_percent > 0.0 {
            println!("⚠️  CAUTION: Strategy is barely profitable");
        } else {
            println!("❌ WARNING: Strategy is losing money - DO NOT USE");
        }
        println!();
    }
}
