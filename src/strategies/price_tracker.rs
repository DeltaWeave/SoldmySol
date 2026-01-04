/// Price Tracker for Multi-Timeframe Analysis
///
/// Tracks token prices over time to calculate price changes for:
/// - 5-minute momentum
/// - 15-minute trends
/// - 1-hour movements
///
/// Used for ML feature extraction and timeframe analysis

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct PricePoint {
    pub timestamp: i64,
    pub price: f64,
}

pub struct PriceTracker {
    prices: VecDeque<PricePoint>,
    max_age_ms: i64,
}

impl PriceTracker {
    /// Create a new price tracker
    /// max_age_minutes: How long to keep price history (typically 120 for 2 hours)
    pub fn new(max_age_minutes: i64) -> Self {
        Self {
            prices: VecDeque::with_capacity(120), // Expect ~1 price per minute
            max_age_ms: max_age_minutes * 60 * 1000,
        }
    }

    /// Add a new price point
    pub fn add_price(&mut self, timestamp: i64, price: f64) {
        // Remove old prices beyond max age
        let cutoff = timestamp - self.max_age_ms;
        self.prices.retain(|p| p.timestamp >= cutoff);

        // Add new price
        self.prices.push_back(PricePoint { timestamp, price });
    }

    /// Get price change percentage over the last N minutes
    /// Returns 0.0 if insufficient data
    pub fn get_price_change(&self, minutes_ago: i64, current_price: f64) -> f64 {
        if self.prices.is_empty() || current_price <= 0.0 {
            return 0.0;
        }

        let now = self.prices.back().unwrap().timestamp;
        let target_time = now - (minutes_ago * 60 * 1000);

        // Find the closest price to the target time
        let old_price = self.prices
            .iter()
            .filter(|p| p.timestamp <= target_time)
            .last()
            .map(|p| p.price)
            .unwrap_or(current_price); // Fallback to current if no old data

        if old_price <= 0.0 {
            return 0.0;
        }

        ((current_price - old_price) / old_price) * 100.0
    }

    /// Check if we have data from N minutes ago
    pub fn has_data(&self, minutes_ago: i64) -> bool {
        if self.prices.is_empty() {
            return false;
        }

        let now = self.prices.back().unwrap().timestamp;
        let target_time = now - (minutes_ago * 60 * 1000);

        self.prices.iter().any(|p| p.timestamp <= target_time)
    }

    /// Get the number of price points stored
    pub fn size(&self) -> usize {
        self.prices.len()
    }

    /// Clear all price history
    pub fn clear(&mut self) {
        self.prices.clear();
    }

    /// Get all price changes for ML features
    /// Returns (5m, 15m, 1h) changes
    pub fn get_all_changes(&self, current_price: f64) -> (f64, f64, f64) {
        (
            self.get_price_change(5, current_price),
            self.get_price_change(15, current_price),
            self.get_price_change(60, current_price),
        )
    }
}

impl Default for PriceTracker {
    fn default() -> Self {
        Self::new(120) // Default 2 hours
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_tracker_creation() {
        let tracker = PriceTracker::new(60);
        assert_eq!(tracker.size(), 0);
    }

    #[test]
    fn test_add_price() {
        let mut tracker = PriceTracker::new(60);
        let now = 1000000;

        tracker.add_price(now, 1.0);
        tracker.add_price(now + 60000, 1.05);

        assert_eq!(tracker.size(), 2);
    }

    #[test]
    fn test_price_change_calculation() {
        let mut tracker = PriceTracker::new(120);
        let base_time = 1000000;

        // Simulate price history
        tracker.add_price(base_time, 1.0);                          // t=0
        tracker.add_price(base_time + 5 * 60 * 1000, 1.05);        // t=5m, +5%
        tracker.add_price(base_time + 15 * 60 * 1000, 1.10);       // t=15m, +10%
        tracker.add_price(base_time + 60 * 60 * 1000, 1.20);       // t=60m, +20%

        let current_price = 1.20;

        let change_5m = tracker.get_price_change(5, current_price);
        let change_15m = tracker.get_price_change(15, current_price);
        let change_60m = tracker.get_price_change(60, current_price);

        // 5m ago: 1.05 -> 1.20 = +14.29%
        assert!((change_5m - 14.29).abs() < 0.1);

        // 15m ago: 1.10 -> 1.20 = +9.09%
        assert!((change_15m - 9.09).abs() < 0.1);

        // 60m ago: 1.00 -> 1.20 = +20%
        assert!((change_60m - 20.0).abs() < 0.1);
    }

    #[test]
    fn test_get_all_changes() {
        let mut tracker = PriceTracker::new(120);
        let base_time = 1000000;

        tracker.add_price(base_time, 1.0);
        tracker.add_price(base_time + 5 * 60 * 1000, 1.05);
        tracker.add_price(base_time + 15 * 60 * 1000, 1.10);
        tracker.add_price(base_time + 60 * 60 * 1000, 1.20);

        let (change_5m, change_15m, change_60m) = tracker.get_all_changes(1.20);

        assert!(change_5m > 0.0);
        assert!(change_15m > 0.0);
        assert!(change_60m > 0.0);
    }

    #[test]
    fn test_has_data() {
        let mut tracker = PriceTracker::new(120);
        let base_time = 1000000;

        assert!(!tracker.has_data(5));

        tracker.add_price(base_time, 1.0);
        tracker.add_price(base_time + 10 * 60 * 1000, 1.05);

        assert!(tracker.has_data(5));
        assert!(tracker.has_data(10));
        assert!(!tracker.has_data(20)); // No data from 20 minutes ago
    }

    #[test]
    fn test_old_price_cleanup() {
        let mut tracker = PriceTracker::new(10); // Only keep 10 minutes
        let base_time = 1000000;

        // Add prices over 20 minutes
        for i in 0..20 {
            tracker.add_price(base_time + i * 60 * 1000, 1.0 + i as f64 * 0.01);
        }

        // Should only keep last 10 minutes
        assert!(tracker.size() <= 11); // 10 minutes + current
    }

    #[test]
    fn test_insufficient_data() {
        let tracker = PriceTracker::new(60);

        // No data - should return 0.0
        let change = tracker.get_price_change(5, 1.0);
        assert_eq!(change, 0.0);
    }

    #[test]
    fn test_zero_price_handling() {
        let mut tracker = PriceTracker::new(60);
        tracker.add_price(1000000, 0.0);

        let change = tracker.get_price_change(5, 0.0);
        assert_eq!(change, 0.0);
    }
}
