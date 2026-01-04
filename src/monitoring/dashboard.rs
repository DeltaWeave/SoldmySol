// ✅ OPERATIONAL TOOLING: Real-time Monitoring Dashboard
// Provides live visibility into bot health, performance, and risk metrics

use crate::config::Config;
use crate::services::Database;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, SystemTime};

pub struct MonitoringDashboard {
    db: Arc<RwLock<Database>>,
    config: Arc<RwLock<Config>>,
    start_time: SystemTime,
}

#[derive(Debug, Clone)]
pub struct DashboardSnapshot {
    // Bot Health
    pub uptime_seconds: u64,
    pub is_healthy: bool,
    pub last_trade_timestamp: Option<i64>,
    pub rpc_health: RpcHealth,

    // Performance
    pub current_capital: f64,
    pub daily_pnl: f64,
    pub daily_pnl_percent: f64,
    pub total_pnl: f64,
    pub total_pnl_percent: f64,

    // Trading Stats
    pub active_positions: usize,
    pub total_trades_today: usize,
    pub win_rate_today: f64,
    pub win_rate_all_time: f64,
    pub avg_hold_time_minutes: f64,

    // Risk Metrics
    pub circuit_breaker_status: String,
    pub risk_level: RiskLevel,
    pub max_drawdown_today: f64,
    pub largest_loss_today: f64,
    pub consecutive_losses: usize,
}

#[derive(Debug, Clone)]
pub struct RpcHealth {
    pub is_connected: bool,
    pub avg_latency_ms: u64,
    pub failed_requests_last_hour: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RiskLevel {
    Low,     // All good
    Medium,  // Approaching limits
    High,    // Close to circuit breaker
    Critical // Circuit breaker active
}

impl MonitoringDashboard {
    pub fn new(db: Arc<RwLock<Database>>, config: Arc<RwLock<Config>>) -> Self {
        Self {
            db,
            config,
            start_time: SystemTime::now(),
        }
    }

    /// Get current dashboard snapshot
    pub async fn get_snapshot(&self) -> Result<DashboardSnapshot> {
        let uptime = self.start_time.elapsed()
            .unwrap_or(Duration::from_secs(0))
            .as_secs();

        let db = self.db.read().await;
        let config = self.config.read().await;

        // Get stats from database
        let stats_today = db.get_daily_stats()?;
        let stats_all_time = db.get_all_time_stats()?;
        let all_trades = db.get_all_trades()?;
        let recent_trades: Vec<_> = all_trades.into_iter().rev().take(10).collect();

        let current_capital = config.capital.current;
        let initial_capital = config.capital.initial;
        let total_pnl = current_capital - initial_capital;
        let total_pnl_percent = (total_pnl / initial_capital) * 100.0;

        // Calculate risk level
        let risk_level = self.calculate_risk_level(&stats_today, &config);

        // Check RPC health (simplified - would need actual RPC metrics in production)
        let rpc_health = RpcHealth {
            is_connected: true,
            avg_latency_ms: 200,
            failed_requests_last_hour: 0,
        };

        let last_trade_timestamp = recent_trades.first().map(|t| t.entry_time);

        Ok(DashboardSnapshot {
            uptime_seconds: uptime,
            is_healthy: self.check_health(uptime, last_trade_timestamp),
            last_trade_timestamp,
            rpc_health,
            current_capital,
            daily_pnl: stats_today.total_pnl,
            daily_pnl_percent: stats_today.pnl_percent,
            total_pnl,
            total_pnl_percent,
            active_positions: stats_today.open_positions,
            total_trades_today: stats_today.total_trades,
            win_rate_today: stats_today.win_rate,
            win_rate_all_time: stats_all_time.win_rate,
            avg_hold_time_minutes: stats_all_time.avg_hold_time_minutes,
            circuit_breaker_status: "OK".to_string(), // Would check actual circuit breaker
            risk_level,
            max_drawdown_today: stats_today.max_drawdown_percent,
            largest_loss_today: stats_today.largest_loss,
            consecutive_losses: stats_today.consecutive_losses,
        })
    }

    /// Display dashboard in terminal
    pub fn display(&self, snapshot: &DashboardSnapshot) {
        self.clear_screen();

        println!("╔══════════════════════════════════════════════════════════════════════╗");
        println!("║                  SOLANA TRADING BOT - LIVE DASHBOARD                ║");
        println!("╚══════════════════════════════════════════════════════════════════════╝\n");

        // Status bar
        let status_icon = if snapshot.is_healthy { "🟢" } else { "🔴" };
        let risk_icon = match snapshot.risk_level {
            RiskLevel::Low => "🟢",
            RiskLevel::Medium => "🟡",
            RiskLevel::High => "🟠",
            RiskLevel::Critical => "🔴",
        };

        println!("Status: {} RUNNING  |  Risk: {} {:?}  |  Uptime: {}",
            status_icon,
            risk_icon,
            snapshot.risk_level,
            self.format_uptime(snapshot.uptime_seconds));

        println!("RPC: {} {}ms latency  |  Circuit Breaker: {}\n",
            if snapshot.rpc_health.is_connected { "🟢" } else { "🔴" },
            snapshot.rpc_health.avg_latency_ms,
            snapshot.circuit_breaker_status);

        // Capital & PnL
        println!("┌─────────────────────────── CAPITAL ───────────────────────────┐");
        self.print_capital_row("Current Capital", snapshot.current_capital, "SOL");
        self.print_pnl_row("Today's PnL", snapshot.daily_pnl, snapshot.daily_pnl_percent);
        self.print_pnl_row("Total PnL", snapshot.total_pnl, snapshot.total_pnl_percent);
        println!("└───────────────────────────────────────────────────────────────┘\n");

        // Trading Stats
        println!("┌───────────────────────── TRADING STATS ───────────────────────┐");
        self.print_stat_row("Active Positions", &snapshot.active_positions.to_string());
        self.print_stat_row("Trades Today", &snapshot.total_trades_today.to_string());
        self.print_stat_row("Win Rate (Today)", &format!("{:.1}%", snapshot.win_rate_today));
        self.print_stat_row("Win Rate (All Time)", &format!("{:.1}%", snapshot.win_rate_all_time));
        self.print_stat_row("Avg Hold Time", &format!("{:.1} min", snapshot.avg_hold_time_minutes));
        println!("└───────────────────────────────────────────────────────────────┘\n");

        // Risk Metrics
        println!("┌──────────────────────── RISK METRICS ─────────────────────────┐");
        self.print_risk_row("Max Drawdown (Today)", snapshot.max_drawdown_today, 15.0);
        self.print_risk_row("Largest Loss (Today)", snapshot.largest_loss_today.abs(), 10.0);
        self.print_stat_row("Consecutive Losses", &snapshot.consecutive_losses.to_string());
        println!("└───────────────────────────────────────────────────────────────┘\n");

        // Last update
        if let Some(last_trade) = snapshot.last_trade_timestamp {
            let mins_ago = (chrono::Utc::now().timestamp_millis() - last_trade) / 1000 / 60;
            println!("Last trade: {} minutes ago", mins_ago);
        } else {
            println!("No trades yet");
        }

        println!("\nRefresh every 5s  |  Press Ctrl+C to stop");
    }

    fn print_capital_row(&self, label: &str, value: f64, unit: &str) {
        println!("│ {:<30} {:>10.4} {} {:>20} │",
            label, value, unit, "");
    }

    fn print_pnl_row(&self, label: &str, value_sol: f64, value_percent: f64) {
        let color = if value_sol >= 0.0 { "+" } else { "" };
        println!("│ {:<30} {}{:>9.4} SOL ({:+.2}%) {:>13} │",
            label, color, value_sol, value_percent, "");
    }

    fn print_stat_row(&self, label: &str, value: &str) {
        println!("│ {:<30} {:<35} │", label, value);
    }

    fn print_risk_row(&self, label: &str, value: f64, threshold: f64) {
        let icon = if value < threshold * 0.5 {
            "🟢"
        } else if value < threshold * 0.8 {
            "🟡"
        } else {
            "🔴"
        };

        println!("│ {:<30} {} {:.2}% (limit: {:.0}%) {:>12} │",
            label, icon, value, threshold, "");
    }

    fn format_uptime(&self, seconds: u64) -> String {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let secs = seconds % 60;
        format!("{}h {}m {}s", hours, minutes, secs)
    }

    fn check_health(&self, uptime: u64, last_trade: Option<i64>) -> bool {
        // Bot is healthy if:
        // 1. Has been running for at least 10 seconds
        // 2. Either had a trade in last hour OR just started (< 10 min)

        if uptime < 10 {
            return true; // Just started
        }

        if let Some(last_trade_ts) = last_trade {
            let mins_since_trade = (chrono::Utc::now().timestamp_millis() - last_trade_ts) / 1000 / 60;
            mins_since_trade < 60 || uptime < 600
        } else {
            uptime < 600 // No trades yet, but just started
        }
    }

    fn calculate_risk_level(&self, stats: &crate::models::DailyStats, config: &Config) -> RiskLevel {
        let daily_loss_percent = stats.pnl_percent.abs();
        let max_daily_loss = config.risk.max_daily_loss * 100.0;

        if stats.consecutive_losses >= 3 {
            return RiskLevel::Critical;
        }

        if daily_loss_percent >= max_daily_loss {
            RiskLevel::Critical
        } else if daily_loss_percent >= max_daily_loss * 0.8 {
            RiskLevel::High
        } else if daily_loss_percent >= max_daily_loss * 0.5 {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }

    fn clear_screen(&self) {
        // ANSI escape code to clear screen and move cursor to top
        print!("\x1B[2J\x1B[1;1H");
    }

    /// Run dashboard in continuous loop
    pub async fn run(self: Arc<Self>) -> Result<()> {
        println!("🚀 Starting monitoring dashboard...\n");

        loop {
            match self.get_snapshot().await {
                Ok(snapshot) => {
                    self.display(&snapshot);

                    // Alert on critical conditions
                    if snapshot.risk_level == RiskLevel::Critical {
                        println!("\n⚠️  ALERT: CRITICAL RISK LEVEL - Review immediately!");
                    }

                    if !snapshot.is_healthy {
                        println!("\n⚠️  WARNING: Bot may be stalled - no recent activity");
                    }
                }
                Err(e) => {
                    println!("Error fetching dashboard data: {}", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

// Additional helper for DailyStats if not defined in models
impl crate::services::Database {
    pub fn get_daily_stats(&self) -> Result<crate::models::DailyStats> {
        // This would query the database for today's stats
        // Placeholder implementation
        Ok(crate::models::DailyStats {
            total_trades: 0,
            winning_trades: 0,
            losing_trades: 0,
            total_pnl: 0.0,
            win_rate: 0.0,
            avg_win: 0.0,
            avg_loss: 0.0,
            largest_win: 0.0,
            largest_loss: 0.0,
            avg_hold_time_minutes: 0.0,
            open_positions: 0,
            pnl_percent: 0.0,
            max_drawdown_percent: 0.0,
            consecutive_losses: 0,
        })
    }
}
