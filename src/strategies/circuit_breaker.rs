/// Circuit Breaker for Rapid Loss Protection
///
/// Tracks recent trade outcomes and pauses trading if too many consecutive
/// losses occur. Prevents catastrophic losses from market downturns.
///
/// Rules:
/// - 3 consecutive losses → 1 hour pause
/// - 5 losses in last 10 trades → 2 hour pause
/// - 10% capital loss in 1 hour → 4 hour pause

use chrono::{DateTime, Utc, Duration};
use tracing::warn;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct TradeOutcome {
    pub timestamp: DateTime<Utc>,
    pub profit_loss: f64,
    pub profit_loss_pct: f64,
}

pub struct CircuitBreaker {
    recent_trades: VecDeque<TradeOutcome>,
    paused_until: Option<DateTime<Utc>>,
    max_history: usize,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(max_history: usize) -> Self {
        Self {
            recent_trades: VecDeque::with_capacity(max_history),
            paused_until: None,
            max_history,
        }
    }

    /// Record a trade outcome
    pub fn record_trade(&mut self, profit_loss: f64, profit_loss_pct: f64) {
        let outcome = TradeOutcome {
            timestamp: Utc::now(),
            profit_loss,
            profit_loss_pct,
        };

        self.recent_trades.push_back(outcome);

        // Keep only max_history trades
        if self.recent_trades.len() > self.max_history {
            self.recent_trades.pop_front();
        }

        // Check if we should pause
        self.check_and_set_pause();
    }

    /// Check if trading is currently paused
    pub fn is_paused(&self) -> bool {
        if let Some(paused_until) = self.paused_until {
            Utc::now() < paused_until
        } else {
            false
        }
    }

    /// Get time remaining in pause (if paused)
    pub fn pause_remaining(&self) -> Option<Duration> {
        if let Some(paused_until) = self.paused_until {
            let now = Utc::now();
            if now < paused_until {
                Some(paused_until - now)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Manually unpause trading
    pub fn unpause(&mut self) {
        self.paused_until = None;
    }

    /// Check conditions and set pause if needed
    fn check_and_set_pause(&mut self) {
        // Rule 1: 3 consecutive losses → 1 hour pause
        if self.has_consecutive_losses(3) {
            self.set_pause(Duration::hours(1), "3 consecutive losses");
            return;
        }

        // Rule 2: 5 losses in last 10 trades → 2 hour pause
        if self.has_losses_in_window(5, 10) {
            self.set_pause(Duration::hours(2), "5 losses in last 10 trades");
            return;
        }

        // Rule 3: 10% capital loss in 1 hour → 4 hour pause
        if self.has_hourly_loss_exceeding(10.0) {
            self.set_pause(Duration::hours(4), "10% capital loss in 1 hour");
            return;
        }
    }

    /// Set pause duration
    fn set_pause(&mut self, duration: Duration, reason: &str) {
        let until = Utc::now() + duration;
        self.paused_until = Some(until);

        warn!("🔴 CIRCUIT BREAKER TRIGGERED: {}", reason);
        warn!("   Trading PAUSED until {}", until.format("%Y-%m-%d %H:%M:%S UTC"));
        warn!("   Duration: {} minutes", duration.num_minutes());
    }

    /// Check for N consecutive losses
    fn has_consecutive_losses(&self, count: usize) -> bool {
        if self.recent_trades.len() < count {
            return false;
        }

        self.recent_trades
            .iter()
            .rev()
            .take(count)
            .all(|t| t.profit_loss < 0.0)
    }

    /// Check for N losses in last M trades
    fn has_losses_in_window(&self, losses: usize, window: usize) -> bool {
        if self.recent_trades.len() < window {
            return false;
        }

        let loss_count = self.recent_trades
            .iter()
            .rev()
            .take(window)
            .filter(|t| t.profit_loss < 0.0)
            .count();

        loss_count >= losses
    }

    /// Check if capital loss exceeds threshold in last hour
    fn has_hourly_loss_exceeding(&self, threshold_pct: f64) -> bool {
        let one_hour_ago = Utc::now() - Duration::hours(1);

        let total_loss_pct: f64 = self.recent_trades
            .iter()
            .filter(|t| t.timestamp > one_hour_ago && t.profit_loss < 0.0)
            .map(|t| t.profit_loss_pct.abs())
            .sum();

        total_loss_pct >= threshold_pct
    }

    /// Get statistics for debugging
    pub fn get_stats(&self) -> CircuitBreakerStats {
        let total_trades = self.recent_trades.len();
        let wins = self.recent_trades.iter().filter(|t| t.profit_loss > 0.0).count();
        let losses = self.recent_trades.iter().filter(|t| t.profit_loss < 0.0).count();

        let total_pnl: f64 = self.recent_trades.iter().map(|t| t.profit_loss).sum();

        let recent_10 = self.recent_trades.iter().rev().take(10).collect::<Vec<_>>();
        let consecutive_losses = recent_10
            .iter()
            .take_while(|t| t.profit_loss < 0.0)
            .count();

        CircuitBreakerStats {
            total_trades,
            wins,
            losses,
            total_pnl,
            consecutive_losses,
            is_paused: self.is_paused(),
            pause_remaining_minutes: self.pause_remaining().map(|d| d.num_minutes()),
        }
    }

    /// Clear all trade history (use with caution)
    pub fn clear(&mut self) {
        self.recent_trades.clear();
        self.paused_until = None;
    }
}

#[derive(Debug)]
pub struct CircuitBreakerStats {
    pub total_trades: usize,
    pub wins: usize,
    pub losses: usize,
    pub total_pnl: f64,
    pub consecutive_losses: usize,
    pub is_paused: bool,
    pub pause_remaining_minutes: Option<i64>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(100) // Keep last 100 trades
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_creation() {
        let cb = CircuitBreaker::new(10);
        assert!(!cb.is_paused());
        assert_eq!(cb.recent_trades.len(), 0);
    }

    #[test]
    fn test_record_trade() {
        let mut cb = CircuitBreaker::new(10);
        cb.record_trade(100.0, 5.0);
        assert_eq!(cb.recent_trades.len(), 1);
    }

    #[test]
    fn test_consecutive_losses_trigger() {
        let mut cb = CircuitBreaker::new(10);

        // Record 3 consecutive losses
        cb.record_trade(-50.0, -2.5);
        cb.record_trade(-40.0, -2.0);
        cb.record_trade(-30.0, -1.5);

        // Should be paused
        assert!(cb.is_paused());
        assert!(cb.pause_remaining().is_some());
    }

    #[test]
    fn test_losses_in_window_trigger() {
        let mut cb = CircuitBreaker::new(20);

        // Record 5 losses and 5 wins (10 trades total)
        for _ in 0..5 {
            cb.record_trade(-50.0, -2.0);
        }
        for _ in 0..5 {
            cb.record_trade(100.0, 5.0);
        }

        // Should be paused (5 losses in last 10)
        assert!(cb.is_paused());
    }

    #[test]
    fn test_no_trigger_on_wins() {
        let mut cb = CircuitBreaker::new(10);

        // Record only wins
        for _ in 0..10 {
            cb.record_trade(100.0, 5.0);
        }

        // Should NOT be paused
        assert!(!cb.is_paused());
    }

    #[test]
    fn test_unpause() {
        let mut cb = CircuitBreaker::new(10);

        // Trigger pause
        cb.record_trade(-50.0, -2.5);
        cb.record_trade(-40.0, -2.0);
        cb.record_trade(-30.0, -1.5);

        assert!(cb.is_paused());

        // Manual unpause
        cb.unpause();
        assert!(!cb.is_paused());
    }

    #[test]
    fn test_max_history_limit() {
        let mut cb = CircuitBreaker::new(5);

        // Add more trades than max_history
        for i in 0..10 {
            cb.record_trade(100.0, 5.0);
        }

        // Should only keep 5
        assert_eq!(cb.recent_trades.len(), 5);
    }

    #[test]
    fn test_stats() {
        let mut cb = CircuitBreaker::new(10);

        cb.record_trade(100.0, 5.0);  // Win
        cb.record_trade(-50.0, -2.5); // Loss
        cb.record_trade(150.0, 7.5);  // Win

        let stats = cb.get_stats();
        assert_eq!(stats.total_trades, 3);
        assert_eq!(stats.wins, 2);
        assert_eq!(stats.losses, 1);
        assert_eq!(stats.total_pnl, 200.0);
    }
}
