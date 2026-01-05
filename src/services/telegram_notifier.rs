// ✅ OPERATIONAL TOOLING: Telegram Notification System
// Real-time alerts for critical events, trades, and bot health

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use teloxide::prelude::*;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct TelegramNotifier {
    bot: Bot,
    chat_id: ChatId,
    enabled: bool,
    alerts_enabled: AlertConfig,
}

#[derive(Debug, Clone)]
pub struct AlertConfig {
    pub trade_entries: bool,
    pub trade_exits: bool,
    pub circuit_breaker: bool,
    pub large_losses: bool,
    pub profit_sweeps: bool,
    pub errors: bool,
    pub daily_summary: bool,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            trade_entries: true,
            trade_exits: true,
            circuit_breaker: true,
            large_losses: true,
            profit_sweeps: true,
            errors: true,
            daily_summary: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeNotification {
    pub action: String, // "ENTRY" or "EXIT"
    pub token_symbol: String,
    pub token_address: String,
    pub amount_sol: f64,
    pub price: f64,
    pub pnl_sol: Option<f64>,
    pub pnl_percent: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlertNotification {
    pub level: AlertLevel,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlertLevel {
    Info,    // 📘
    Success, // ✅
    Warning, // ⚠️
    Error,   // 🚨
    Critical // ⛔
}

impl TelegramNotifier {
    /// Create new Telegram notifier from environment
    pub fn new() -> Result<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| anyhow!("TELEGRAM_BOT_TOKEN not set"))?;

        let chat_id_str = std::env::var("TELEGRAM_CHAT_ID")
            .map_err(|_| anyhow!("TELEGRAM_CHAT_ID not set"))?;

        let chat_id = ChatId(chat_id_str.parse::<i64>()
            .map_err(|_| anyhow!("Invalid TELEGRAM_CHAT_ID format"))?);

        let enabled = std::env::var("TELEGRAM_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .parse::<bool>()
            .unwrap_or(true);

        let bot = Bot::new(token);

        info!("📱 Telegram Notifier initialized");
        info!("   Chat ID: {}", chat_id);
        info!("   Enabled: {}", enabled);

        Ok(Self {
            bot,
            chat_id,
            enabled,
            alerts_enabled: AlertConfig::default(),
        })
    }

    /// Send startup notification
    pub async fn send_startup(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let message = format!(
            "🚀 *SOLANA TRADING BOT STARTED*\n\n\
            📅 Time: {}\n\
            🤖 Version: 2.1\n\
            🔧 Mode: Production\n\n\
            ✅ All systems operational\n\
            📊 Monitoring active\n\
            🛡️ Circuit breaker armed\n\n\
            _Ready to trade_",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send shutdown notification
    pub async fn send_shutdown(&self, stats: ShutdownStats) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let message = format!(
            "🛑 *BOT SHUTDOWN*\n\n\
            📅 Time: {}\n\
            ⏱️ Uptime: {}\n\n\
            📊 *Session Stats:*\n\
            Trades: {}\n\
            Win Rate: {:.1}%\n\
            PnL: {:+.4} SOL ({:+.2}%)\n\n\
            Status: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            Self::format_duration(stats.uptime_seconds),
            stats.total_trades,
            stats.win_rate,
            stats.pnl_sol,
            stats.pnl_percent,
            if stats.clean_shutdown { "✅ Clean shutdown" } else { "⚠️ Emergency stop" }
        );

        self.send_message(&message).await
    }

    /// Send trade entry notification
    pub async fn send_trade_entry(&self, trade: &TradeNotification) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.trade_entries {
            return Ok(());
        }

        let message = format!(
            "🟢 *TRADE ENTRY*\n\n\
            🪙 Token: `{}`\n\
            💰 Amount: {:.4} SOL\n\
            💵 Price: ${:.8}\n\
            📍 Address: `{}`\n\n\
            ⏰ {}",
            trade.token_symbol,
            trade.amount_sol,
            trade.price,
            &trade.token_address[..8],
            chrono::Utc::now().format("%H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send trade exit notification
    pub async fn send_trade_exit(&self, trade: &TradeNotification) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.trade_exits {
            return Ok(());
        }

        let pnl_sol = trade.pnl_sol.unwrap_or(0.0);
        let pnl_percent = trade.pnl_percent.unwrap_or(0.0);

        let icon = if pnl_sol > 0.0 { "🟢" } else { "🔴" };
        let result = if pnl_sol > 0.0 { "PROFIT" } else { "LOSS" };

        let message = format!(
            "{} *TRADE EXIT - {}*\n\n\
            🪙 Token: `{}`\n\
            💰 Amount: {:.4} SOL\n\
            💵 Exit Price: ${:.8}\n\n\
            📊 *P&L:*\n\
            SOL: {:+.4}\n\
            Percent: {:+.2}%\n\n\
            ℹ️ Reason: {}\n\
            ⏰ {}",
            icon,
            result,
            trade.token_symbol,
            trade.amount_sol,
            trade.price,
            pnl_sol,
            pnl_percent,
            trade.reason.as_deref().unwrap_or("N/A"),
            chrono::Utc::now().format("%H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send circuit breaker alert
    pub async fn send_circuit_breaker(&self, reason: &str, resume_time: Option<String>) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.circuit_breaker {
            return Ok(());
        }

        let message = format!(
            "⛔ *CIRCUIT BREAKER TRIGGERED*\n\n\
            🚨 Trading halted\n\n\
            📋 Reason: {}\n\
            ⏰ Resume: {}\n\n\
            ⚠️ Review required",
            reason,
            resume_time.unwrap_or_else(|| "Manual intervention needed".to_string())
        );

        self.send_message(&message).await
    }

    /// Send large loss alert
    pub async fn send_large_loss(&self, token: &str, loss_sol: f64, loss_percent: f64) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.large_losses {
            return Ok(());
        }

        let message = format!(
            "🔴 *LARGE LOSS ALERT*\n\n\
            🪙 Token: `{}`\n\
            💔 Loss: {:.4} SOL ({:.2}%)\n\n\
            ⚠️ Above threshold\n\
            ⏰ {}",
            token,
            loss_sol.abs(),
            loss_percent.abs(),
            chrono::Utc::now().format("%H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send profit sweep notification
    pub async fn send_profit_sweep(&self, amount: f64, cold_wallet: &str) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.profit_sweeps {
            return Ok(());
        }

        let message = format!(
            "💰 *PROFIT SWEEP EXECUTED*\n\n\
            💸 Amount: {:.4} SOL\n\
            🏦 To: `{}`\n\n\
            ✅ Profits secured\n\
            ⏰ {}",
            amount,
            &cold_wallet[..8],
            chrono::Utc::now().format("%H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send daily summary report
    pub async fn send_daily_summary(&self, summary: DailySummary) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.daily_summary {
            return Ok(());
        }

        let pnl_icon = if summary.pnl_sol >= 0.0 { "📈" } else { "📉" };
        let result = if summary.pnl_sol >= 0.0 { "PROFIT" } else { "LOSS" };

        let message = format!(
            "{} *DAILY SUMMARY - {}*\n\n\
            📅 Date: {}\n\n\
            📊 *Performance:*\n\
            Total Trades: {}\n\
            Winning: {} ({:.1}%)\n\
            Losing: {}\n\n\
            💰 *P&L:*\n\
            SOL: {:+.4}\n\
            Percent: {:+.2}%\n\
            Capital: {:.4} SOL\n\n\
            📈 *Best Trade:* {:+.2}%\n\
            📉 *Worst Trade:* {:+.2}%\n\
            ⏱️ *Avg Hold:* {:.1} min\n\n\
            🎯 *Risk:*\n\
            Max Drawdown: {:.2}%\n\
            Consecutive Losses: {}\n\n\
            {}",
            pnl_icon,
            result,
            summary.date,
            summary.total_trades,
            summary.winning_trades,
            summary.win_rate,
            summary.losing_trades,
            summary.pnl_sol,
            summary.pnl_percent,
            summary.current_capital,
            summary.best_trade_percent,
            summary.worst_trade_percent,
            summary.avg_hold_time_minutes,
            summary.max_drawdown,
            summary.consecutive_losses,
            if summary.circuit_breaker_triggered {
                "⚠️ Circuit breaker was triggered today"
            } else {
                "✅ No circuit breaker triggers"
            }
        );

        self.send_message(&message).await
    }

    /// Send generic alert
    pub async fn send_alert(&self, alert: AlertNotification) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.errors {
            return Ok(());
        }

        let icon = match alert.level {
            AlertLevel::Info => "📘",
            AlertLevel::Success => "✅",
            AlertLevel::Warning => "⚠️",
            AlertLevel::Error => "🚨",
            AlertLevel::Critical => "⛔",
        };

        let level_name = match alert.level {
            AlertLevel::Info => "INFO",
            AlertLevel::Success => "SUCCESS",
            AlertLevel::Warning => "WARNING",
            AlertLevel::Error => "ERROR",
            AlertLevel::Critical => "CRITICAL",
        };

        let message = format!(
            "{} *{}*\n\n\
            {}\n\n\
            {}\n\n\
            ⏰ {}",
            icon,
            level_name,
            alert.title,
            alert.message,
            chrono::Utc::now().format("%H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send error notification
    pub async fn send_error(&self, error_type: &str, error_message: &str) -> Result<()> {
        if !self.enabled || !self.alerts_enabled.errors {
            return Ok(());
        }

        let message = format!(
            "🚨 *ERROR DETECTED*\n\n\
            Type: `{}`\n\
            Message: {}\n\n\
            ⚠️ Requires attention\n\
            ⏰ {}",
            error_type,
            error_message,
            chrono::Utc::now().format("%H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Send test notification
    pub async fn send_test(&self) -> Result<()> {
        if !self.enabled {
            return Err(anyhow!("Telegram notifications are disabled"));
        }

        let message = format!(
            "🧪 *TEST NOTIFICATION*\n\n\
            ✅ Telegram integration working\n\
            📱 Bot can send alerts\n\
            🔔 Notifications enabled\n\n\
            ⏰ {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );

        self.send_message(&message).await
    }

    /// Internal: Send message with retry logic
    async fn send_message(&self, text: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        const MAX_RETRIES: u32 = 3;
        let mut retries = 0;

        loop {
            match self.bot
                .send_message(self.chat_id, text)
                .await
            {
                Ok(_) => {
                    info!("📱 Telegram notification sent");
                    return Ok(());
                }
                Err(e) => {
                    retries += 1;
                    if retries >= MAX_RETRIES {
                        error!("Failed to send Telegram notification after {} retries: {}", MAX_RETRIES, e);
                        return Err(anyhow!("Telegram send failed: {}", e));
                    }
                    warn!("Telegram send failed (attempt {}/{}): {}", retries, MAX_RETRIES, e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Format duration in human-readable form
    fn format_duration(seconds: u64) -> String {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        let minutes = (seconds % 3600) / 60;

        if days > 0 {
            format!("{}d {}h {}m", days, hours, minutes)
        } else if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    }

    /// Update alert configuration
    pub fn set_alerts(&mut self, config: AlertConfig) {
        self.alerts_enabled = config;
    }

    /// Enable/disable all notifications
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        info!("📱 Telegram notifications {}", if enabled { "enabled" } else { "disabled" });
    }
}

#[derive(Debug, Clone)]
pub struct ShutdownStats {
    pub uptime_seconds: u64,
    pub total_trades: usize,
    pub win_rate: f64,
    pub pnl_sol: f64,
    pub pnl_percent: f64,
    pub clean_shutdown: bool,
}

#[derive(Debug, Clone)]
pub struct DailySummary {
    pub date: String,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub pnl_sol: f64,
    pub pnl_percent: f64,
    pub current_capital: f64,
    pub best_trade_percent: f64,
    pub worst_trade_percent: f64,
    pub avg_hold_time_minutes: f64,
    pub max_drawdown: f64,
    pub consecutive_losses: usize,
    pub circuit_breaker_triggered: bool,
}

// Example integration in main.rs:
//
// let telegram = Arc::new(TelegramNotifier::new()?);
// telegram.send_startup().await?;
//
// // On trade entry:
// telegram.send_trade_entry(&TradeNotification {
//     action: "ENTRY".to_string(),
//     token_symbol: "TOKEN".to_string(),
//     token_address: "abc123...".to_string(),
//     amount_sol: 0.1,
//     price: 0.0001,
//     pnl_sol: None,
//     pnl_percent: None,
//     reason: None,
// }).await?;
