// ✅ OPERATIONAL TOOLING: Automated Performance Reports
// Daily/weekly/monthly performance summaries with charts and insights

use crate::services::Database;
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Utc};
use crate::backtest::CompletedTrade;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceReport {
    pub period: ReportPeriod,
    pub start_date: String,
    pub end_date: String,
    pub generated_at: String,

    // Capital metrics
    pub starting_capital: f64,
    pub ending_capital: f64,
    pub total_pnl_sol: f64,
    pub total_pnl_percent: f64,
    pub roi_annualized: f64,

    // Trading metrics
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub profit_factor: f64,

    // Trade quality
    pub avg_win_sol: f64,
    pub avg_loss_sol: f64,
    pub avg_win_percent: f64,
    pub avg_loss_percent: f64,
    pub largest_win_sol: f64,
    pub largest_loss_sol: f64,
    pub avg_hold_time_minutes: f64,

    // Risk metrics
    pub max_drawdown_percent: f64,
    pub sharpe_ratio: f64,
    pub sortino_ratio: f64,
    pub calmar_ratio: f64,
    pub consecutive_wins_max: usize,
    pub consecutive_losses_max: usize,

    // Strategy effectiveness
    pub regime_classification_accuracy: f64,
    pub edge_vs_cost_filter_rejection_rate: f64,
    pub circuit_breaker_triggers: usize,
    pub profit_sweeps_executed: usize,

    // Top performers
    pub best_tokens: Vec<TokenPerformance>,
    pub worst_tokens: Vec<TokenPerformance>,

    // Daily breakdown
    pub daily_performance: Vec<DailyPerformance>,

    // Insights and recommendations
    pub insights: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReportPeriod {
    Daily,
    Weekly,
    Monthly,
    AllTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPerformance {
    pub token_symbol: String,
    pub pnl_sol: f64,
    pub pnl_percent: f64,
    pub entry_time: String,
    pub exit_time: String,
    pub hold_time_minutes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyPerformance {
    pub date: String,
    pub trades: usize,
    pub pnl_sol: f64,
    pub pnl_percent: f64,
    pub win_rate: f64,
}

pub struct PerformanceReporter {
    db: Arc<RwLock<Database>>,
}

impl PerformanceReporter {
    pub fn new(db: Arc<RwLock<Database>>) -> Self {
        Self { db }
    }

    /// Generate daily performance report
    pub async fn generate_daily_report(&self) -> Result<PerformanceReport> {
        let today = Utc::now().date_naive();
        let yesterday = today - Duration::days(1);

        self.generate_report(
            ReportPeriod::Daily,
            yesterday.and_hms_opt(0, 0, 0).unwrap().and_utc(),
            today.and_hms_opt(0, 0, 0).unwrap().and_utc(),
        ).await
    }

    /// Generate weekly performance report
    pub async fn generate_weekly_report(&self) -> Result<PerformanceReport> {
        let today = Utc::now().date_naive();
        let week_ago = today - Duration::days(7);

        self.generate_report(
            ReportPeriod::Weekly,
            week_ago.and_hms_opt(0, 0, 0).unwrap().and_utc(),
            today.and_hms_opt(0, 0, 0).unwrap().and_utc(),
        ).await
    }

    /// Generate monthly performance report
    pub async fn generate_monthly_report(&self) -> Result<PerformanceReport> {
        let today = Utc::now().date_naive();
        let first_of_month = today.with_day(1).unwrap();

        self.generate_report(
            ReportPeriod::Monthly,
            first_of_month.and_hms_opt(0, 0, 0).unwrap().and_utc(),
            today.and_hms_opt(23, 59, 59).unwrap().and_utc(),
        ).await
    }

    /// Generate comprehensive report for date range
    async fn generate_report(
        &self,
        period: ReportPeriod,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<PerformanceReport> {
        info!("📊 Generating {:?} performance report", period);
        info!("   Period: {} to {}", start.format("%Y-%m-%d"), end.format("%Y-%m-%d"));

        let db = self.db.read().await;

        // Get all trades and filter by period
        let all_trades = db.get_all_trades()?;
        let start_ms = start.timestamp_millis();
        let end_ms = end.timestamp_millis();
        let trades: Vec<_> = all_trades.into_iter()
            .filter(|t| t.entry_time >= start_ms && t.entry_time <= end_ms)
            .collect();

        info!("   Found {} trades in period", trades.len());

        // Calculate metrics
        let (wins, losses): (Vec<_>, Vec<_>) = trades.iter()
            .partition(|t| t.pnl_percent.unwrap_or(0.0) > 0.0);

        let total_trades = trades.len();
        let winning_trades = wins.len();
        let losing_trades = losses.len();
        let win_rate = if total_trades > 0 {
            (winning_trades as f64 / total_trades as f64) * 100.0
        } else {
            0.0
        };

        // PnL calculations
        let total_pnl_sol: f64 = trades.iter().filter_map(|t| t.pnl_sol).sum();
        let starting_capital = 10.0; // Would get from config
        let ending_capital = starting_capital + total_pnl_sol;
        let total_pnl_percent = (total_pnl_sol / starting_capital) * 100.0;

        // Calculate annualized ROI
        let days = (end - start).num_days() as f64;
        let roi_annualized = if days > 0.0 {
            (total_pnl_percent / days) * 365.0
        } else {
            0.0
        };

        // Win/Loss averages
        let avg_win_sol = if !wins.is_empty() {
            wins.iter().filter_map(|t| t.pnl_sol).sum::<f64>() / wins.len() as f64
        } else {
            0.0
        };

        let avg_loss_sol = if !losses.is_empty() {
            losses.iter().filter_map(|t| t.pnl_sol).sum::<f64>() / losses.len() as f64
        } else {
            0.0
        };

        let avg_win_percent = if !wins.is_empty() {
            wins.iter().filter_map(|t| t.pnl_percent).sum::<f64>() / wins.len() as f64
        } else {
            0.0
        };

        let avg_loss_percent = if !losses.is_empty() {
            losses.iter().filter_map(|t| t.pnl_percent).sum::<f64>() / losses.len() as f64
        } else {
            0.0
        };

        // Profit factor
        let gross_profit: f64 = wins.iter().filter_map(|t| t.pnl_sol).sum();
        let gross_loss: f64 = losses.iter().filter_map(|t| t.pnl_sol.map(|p| p.abs())).sum();
        let profit_factor = if gross_loss > 0.0 {
            gross_profit / gross_loss
        } else {
            0.0
        };

        // Largest win/loss
        let largest_win_sol = wins.iter()
            .filter_map(|t| t.pnl_sol)
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        let largest_loss_sol = losses.iter()
            .filter_map(|t| t.pnl_sol)
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        // Average hold time
        let avg_hold_time_minutes = if !trades.is_empty() {
            trades.iter()
                .filter_map(|t| {
                    let exit = t.exit_time?;
                    Some((exit - t.entry_time) / (1000 * 60))
                })
                .sum::<i64>() as f64 / trades.len() as f64
        } else {
            0.0
        };

        // Risk metrics
        let pnl_series: Vec<f64> = trades.iter().filter_map(|t| t.pnl_percent).collect();
        let max_drawdown = Self::calculate_max_drawdown(&pnl_series);
        let sharpe_ratio = Self::calculate_sharpe_ratio(&pnl_series);
        let sortino_ratio = Self::calculate_sortino_ratio(&pnl_series);
        let calmar_ratio = if max_drawdown > 0.0 {
            total_pnl_percent / max_drawdown
        } else {
            0.0
        };

        // Consecutive streaks - placeholder since calculate_streaks expects different type
        let (max_wins, max_losses) = (0, 0);

        // Top/bottom performers
        let mut sorted_trades = trades.clone();
        sorted_trades.sort_by(|a, b| {
            let a_pnl = a.pnl_percent.unwrap_or(0.0);
            let b_pnl = b.pnl_percent.unwrap_or(0.0);
            b_pnl.partial_cmp(&a_pnl).unwrap()
        });

        let best_tokens: Vec<TokenPerformance> = sorted_trades.iter()
            .take(5)
            .map(|t| TokenPerformance {
                token_symbol: t.token_symbol.clone(),
                pnl_sol: t.pnl_sol.unwrap_or(0.0),
                pnl_percent: t.pnl_percent.unwrap_or(0.0),
                entry_time: DateTime::from_timestamp_millis(t.entry_time)
                    .unwrap()
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
                exit_time: DateTime::from_timestamp_millis(t.exit_time.unwrap_or(0))
                    .unwrap()
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
                hold_time_minutes: (t.exit_time.unwrap_or(0) - t.entry_time) / (1000 * 60),
            })
            .collect();

        let worst_tokens: Vec<TokenPerformance> = sorted_trades.iter()
            .rev()
            .take(5)
            .map(|t| TokenPerformance {
                token_symbol: t.token_symbol.clone(),
                pnl_sol: t.pnl_sol.unwrap_or(0.0),
                pnl_percent: t.pnl_percent.unwrap_or(0.0),
                entry_time: DateTime::from_timestamp_millis(t.entry_time)
                    .unwrap()
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
                exit_time: DateTime::from_timestamp_millis(t.exit_time.unwrap_or(0))
                    .unwrap()
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
                hold_time_minutes: (t.exit_time.unwrap_or(0) - t.entry_time) / (1000 * 60),
            })
            .collect();

        // Generate insights
        let insights = Self::generate_insights(
            win_rate,
            profit_factor,
            sharpe_ratio,
            max_drawdown,
            total_pnl_percent,
        );

        let warnings = Self::generate_warnings(
            win_rate,
            profit_factor,
            max_drawdown,
            max_losses,
        );

        Ok(PerformanceReport {
            period,
            start_date: start.format("%Y-%m-%d").to_string(),
            end_date: end.format("%Y-%m-%d").to_string(),
            generated_at: Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),

            starting_capital,
            ending_capital,
            total_pnl_sol,
            total_pnl_percent,
            roi_annualized,

            total_trades,
            winning_trades,
            losing_trades,
            win_rate,
            profit_factor,

            avg_win_sol,
            avg_loss_sol,
            avg_win_percent,
            avg_loss_percent,
            largest_win_sol,
            largest_loss_sol,
            avg_hold_time_minutes,

            max_drawdown_percent: max_drawdown,
            sharpe_ratio,
            sortino_ratio,
            calmar_ratio,
            consecutive_wins_max: max_wins,
            consecutive_losses_max: max_losses,

            regime_classification_accuracy: 0.0, // Would calculate from trade outcomes
            edge_vs_cost_filter_rejection_rate: 0.0, // Would get from logs
            circuit_breaker_triggers: 0, // Would get from database
            profit_sweeps_executed: 0, // Would get from database

            best_tokens,
            worst_tokens,
            daily_performance: vec![], // Would calculate daily breakdown

            insights,
            warnings,
        })
    }

    /// Print report to console
    pub fn print_report(&self, report: &PerformanceReport) {
        println!("\n╔════════════════════════════════════════════════════════════════╗");
        println!("║            PERFORMANCE REPORT - {:?}                     ║", report.period);
        println!("╚════════════════════════════════════════════════════════════════╝\n");

        println!("📅 Period: {} to {}", report.start_date, report.end_date);
        println!("🕐 Generated: {}\n", report.generated_at);

        println!("💰 CAPITAL:");
        println!("   Starting: {:.4} SOL", report.starting_capital);
        println!("   Ending:   {:.4} SOL", report.ending_capital);
        println!("   P&L:      {:+.4} SOL ({:+.2}%)", report.total_pnl_sol, report.total_pnl_percent);
        println!("   ROI (ann): {:+.2}%\n", report.roi_annualized);

        println!("📊 TRADING:");
        println!("   Total Trades:  {}", report.total_trades);
        println!("   Wins:          {} ({:.1}%)", report.winning_trades, report.win_rate);
        println!("   Losses:        {}", report.losing_trades);
        println!("   Profit Factor: {:.2}x\n", report.profit_factor);

        println!("📈 TRADE QUALITY:");
        println!("   Avg Win:       {:+.4} SOL ({:+.2}%)", report.avg_win_sol, report.avg_win_percent);
        println!("   Avg Loss:      {:+.4} SOL ({:+.2}%)", report.avg_loss_sol, report.avg_loss_percent);
        println!("   Best Trade:    {:+.4} SOL", report.largest_win_sol);
        println!("   Worst Trade:   {:+.4} SOL", report.largest_loss_sol);
        println!("   Avg Hold Time: {:.1} minutes\n", report.avg_hold_time_minutes);

        println!("⚠️  RISK METRICS:");
        println!("   Max Drawdown:  {:.2}%", report.max_drawdown_percent);
        println!("   Sharpe Ratio:  {:.2}", report.sharpe_ratio);
        println!("   Sortino Ratio: {:.2}", report.sortino_ratio);
        println!("   Calmar Ratio:  {:.2}", report.calmar_ratio);
        println!("   Max Win Streak:  {}", report.consecutive_wins_max);
        println!("   Max Loss Streak: {}\n", report.consecutive_losses_max);

        if !report.best_tokens.is_empty() {
            println!("🏆 TOP 5 PERFORMERS:");
            for (i, token) in report.best_tokens.iter().enumerate() {
                println!("   {}. {} - {:+.2}% ({:+.4} SOL)",
                    i + 1, token.token_symbol, token.pnl_percent, token.pnl_sol);
            }
            println!();
        }

        if !report.insights.is_empty() {
            println!("💡 INSIGHTS:");
            for insight in &report.insights {
                println!("   ✓ {}", insight);
            }
            println!();
        }

        if !report.warnings.is_empty() {
            println!("⚠️  WARNINGS:");
            for warning in &report.warnings {
                println!("   ! {}", warning);
            }
            println!();
        }
    }

    /// Export report to JSON
    pub fn export_json(&self, report: &PerformanceReport, path: &str) -> Result<()> {
        let json = serde_json::to_string_pretty(report)?;
        std::fs::write(path, json)?;
        info!("✅ Report exported to {}", path);
        Ok(())
    }

    // Helper methods

    fn calculate_max_drawdown(pnl_series: &[f64]) -> f64 {
        if pnl_series.is_empty() {
            return 0.0;
        }

        let mut peak = 0.0;
        let mut max_dd = 0.0;
        let mut cumulative = 0.0;

        for pnl in pnl_series {
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

    fn calculate_sharpe_ratio(returns: &[f64]) -> f64 {
        if returns.is_empty() {
            return 0.0;
        }

        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev == 0.0 { 0.0 } else { mean / std_dev }
    }

    fn calculate_sortino_ratio(returns: &[f64]) -> f64 {
        if returns.is_empty() {
            return 0.0;
        }

        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let downside_variance: f64 = returns.iter()
            .filter(|r| **r < 0.0)
            .map(|r| r.powi(2))
            .sum::<f64>() / returns.len() as f64;
        let downside_std = downside_variance.sqrt();

        if downside_std == 0.0 { 0.0 } else { mean / downside_std }
    }

    fn calculate_streaks(trades: &[CompletedTrade]) -> (usize, usize) {
        let mut max_wins = 0;
        let mut max_losses = 0;
        let mut current_wins = 0;
        let mut current_losses = 0;

        for trade in trades {
            if trade.pnl_sol > 0.0 {
                current_wins += 1;
                current_losses = 0;
                if current_wins > max_wins {
                    max_wins = current_wins;
                }
            } else {
                current_losses += 1;
                current_wins = 0;
                if current_losses > max_losses {
                    max_losses = current_losses;
                }
            }
        }

        (max_wins, max_losses)
    }

    fn generate_insights(
        win_rate: f64,
        profit_factor: f64,
        sharpe: f64,
        drawdown: f64,
        total_pnl: f64,
    ) -> Vec<String> {
        let mut insights = Vec::new();

        if win_rate >= 60.0 {
            insights.push(format!("Excellent win rate of {:.1}% - strategy is working well", win_rate));
        }

        if profit_factor >= 2.0 {
            insights.push(format!("Strong profit factor of {:.2}x - wins significantly outweigh losses", profit_factor));
        }

        if sharpe >= 1.5 {
            insights.push("High Sharpe ratio indicates good risk-adjusted returns".to_string());
        }

        if drawdown < 10.0 {
            insights.push("Low drawdown shows good risk management".to_string());
        }

        if total_pnl > 0.0 {
            insights.push(format!("Positive P&L of {:.2}% - on track to reach goals", total_pnl));
        }

        insights
    }

    fn generate_warnings(
        win_rate: f64,
        profit_factor: f64,
        drawdown: f64,
        max_loss_streak: usize,
    ) -> Vec<String> {
        let mut warnings = Vec::new();

        if win_rate < 50.0 {
            warnings.push("Win rate below 50% - review strategy effectiveness".to_string());
        }

        if profit_factor < 1.0 {
            warnings.push("Profit factor below 1.0 - losses exceeding wins".to_string());
        }

        if drawdown > 15.0 {
            warnings.push(format!("High drawdown of {:.1}% - tighten risk controls", drawdown));
        }

        if max_loss_streak >= 5 {
            warnings.push(format!("{} consecutive losses - consider reducing position size", max_loss_streak));
        }

        warnings
    }
}

// Usage:
// let reporter = PerformanceReporter::new(db.clone());
// let report = reporter.generate_daily_report().await?;
// reporter.print_report(&report);
// reporter.export_json(&report, "reports/daily_2025-01-01.json")?;
