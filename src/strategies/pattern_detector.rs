/// Pattern Recognition for Token Trading
///
/// Detects common price patterns and market behaviors:
/// - Pump and dump schemes
/// - Breakout patterns
/// - Accumulation phases
/// - Distribution phases
/// - Support/resistance levels

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    PumpAndDump,
    Breakout,
    Accumulation,
    Distribution,
    Consolidation,
    None,
}

#[derive(Debug, Clone)]
pub struct PricePoint {
    pub timestamp: i64,
    pub price: f64,
    pub volume: f64,
}

#[derive(Debug, Clone)]
pub struct PatternResult {
    pub pattern: Pattern,
    pub confidence: u8,  // 0-100
    pub description: String,
    pub trade_recommendation: TradeRecommendation,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TradeRecommendation {
    StrongBuy,
    Buy,
    Hold,
    Sell,
    StrongSell,
    Avoid,
}

pub struct PatternDetector {
    price_history: VecDeque<PricePoint>,
    max_history: usize,
}

impl PatternDetector {
    pub fn new(max_history: usize) -> Self {
        Self {
            price_history: VecDeque::with_capacity(max_history),
            max_history,
        }
    }

    /// Add a new price point to the history
    pub fn add_price_point(&mut self, timestamp: i64, price: f64, volume: f64) {
        if self.price_history.len() >= self.max_history {
            self.price_history.pop_front();
        }

        self.price_history.push_back(PricePoint {
            timestamp,
            price,
            volume,
        });
    }

    /// Detect patterns in the current price history
    pub fn detect_pattern(&self) -> PatternResult {
        if self.price_history.len() < 5 {
            return PatternResult {
                pattern: Pattern::None,
                confidence: 0,
                description: "Insufficient data".to_string(),
                trade_recommendation: TradeRecommendation::Hold,
            };
        }

        // Check for pump and dump (highest priority)
        if let Some(result) = self.detect_pump_and_dump() {
            return result;
        }

        // Check for breakout
        if let Some(result) = self.detect_breakout() {
            return result;
        }

        // Check for accumulation
        if let Some(result) = self.detect_accumulation() {
            return result;
        }

        // Check for distribution
        if let Some(result) = self.detect_distribution() {
            return result;
        }

        // Check for consolidation
        if let Some(result) = self.detect_consolidation() {
            return result;
        }

        // No clear pattern
        PatternResult {
            pattern: Pattern::None,
            confidence: 0,
            description: "No clear pattern detected".to_string(),
            trade_recommendation: TradeRecommendation::Hold,
        }
    }

    /// Detect pump and dump pattern
    /// Characteristics:
    /// - Rapid price increase (>50% in short time)
    /// - Followed by rapid decline
    /// - Volume spike during pump
    /// - Decreasing volume during dump
    fn detect_pump_and_dump(&self) -> Option<PatternResult> {
        if self.price_history.len() < 10 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().map(|p| p.price).collect();
        let volumes: Vec<f64> = self.price_history.iter().map(|p| p.volume).collect();

        // Find the peak
        let max_price = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let max_idx = prices.iter().position(|&p| p == max_price)?;

        if max_idx < 3 || max_idx > prices.len() - 3 {
            return None; // Peak must be in the middle
        }

        // Check for rapid rise before peak
        let rise = (max_price - prices[max_idx - 3]) / prices[max_idx - 3] * 100.0;

        // Check for rapid fall after peak
        let current_price = prices[prices.len() - 1];
        let fall = (max_price - current_price) / max_price * 100.0;

        // Check volume pattern
        let avg_volume_before: f64 = volumes[..max_idx].iter().sum::<f64>() / max_idx as f64;
        let volume_at_peak = volumes[max_idx];
        let avg_volume_after: f64 = volumes[max_idx..].iter().sum::<f64>() / (volumes.len() - max_idx) as f64;

        let volume_spike = volume_at_peak > avg_volume_before * 2.0;
        let volume_decline = avg_volume_after < volume_at_peak * 0.5;

        if rise > 50.0 && fall > 30.0 && volume_spike && volume_decline {
            let confidence = ((rise.min(100.0) + fall.min(100.0)) / 2.0) as u8;

            return Some(PatternResult {
                pattern: Pattern::PumpAndDump,
                confidence: confidence.min(95),
                description: format!(
                    "Pump and dump detected: +{:.1}% rise, -{:.1}% fall from peak",
                    rise, fall
                ),
                trade_recommendation: TradeRecommendation::Avoid,
            });
        }

        None
    }

    /// Detect breakout pattern
    /// Characteristics:
    /// - Price moves above resistance level
    /// - Increased volume on breakout
    /// - Sustained momentum after breakout
    fn detect_breakout(&self) -> Option<PatternResult> {
        if self.price_history.len() < 10 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().map(|p| p.price).collect();
        let volumes: Vec<f64> = self.price_history.iter().map(|p| p.volume).collect();

        // Calculate resistance (recent high before latest prices)
        let resistance = prices[..prices.len() - 3]
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);

        let current_price = prices[prices.len() - 1];
        let prev_price = prices[prices.len() - 2];

        // Check if price broke above resistance
        if current_price > resistance * 1.02 && prev_price <= resistance {
            // Check for volume increase
            let recent_volume = volumes[volumes.len() - 1];
            let avg_volume: f64 = volumes[..volumes.len() - 1].iter().sum::<f64>()
                / (volumes.len() - 1) as f64;

            let volume_increase = recent_volume > avg_volume * 1.5;

            if volume_increase {
                let breakout_strength = ((current_price - resistance) / resistance * 100.0) as u8;
                let confidence = (breakout_strength as f64 * 0.7 + 30.0) as u8;

                return Some(PatternResult {
                    pattern: Pattern::Breakout,
                    confidence: confidence.min(85),
                    description: format!(
                        "Breakout detected: {:.2}% above resistance",
                        (current_price - resistance) / resistance * 100.0
                    ),
                    trade_recommendation: if confidence > 70 {
                        TradeRecommendation::Buy
                    } else {
                        TradeRecommendation::Hold
                    },
                });
            }
        }

        None
    }

    /// Detect accumulation pattern
    /// Characteristics:
    /// - Narrow price range (low volatility)
    /// - Steady or increasing volume
    /// - Price stability (not declining)
    fn detect_accumulation(&self) -> Option<PatternResult> {
        if self.price_history.len() < 10 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().map(|p| p.price).collect();
        let volumes: Vec<f64> = self.price_history.iter().map(|p| p.volume).collect();

        // Calculate price range
        let max_price = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_price = prices.iter().cloned().fold(f64::INFINITY, f64::min);
        let price_range = (max_price - min_price) / min_price * 100.0;

        // Calculate volume trend
        let first_half_volume: f64 = volumes[..volumes.len() / 2].iter().sum::<f64>()
            / (volumes.len() / 2) as f64;
        let second_half_volume: f64 = volumes[volumes.len() / 2..].iter().sum::<f64>()
            / (volumes.len() - volumes.len() / 2) as f64;

        let volume_increasing = second_half_volume > first_half_volume;

        // Check price stability
        let first_price = prices[0];
        let last_price = prices[prices.len() - 1];
        let price_stable = last_price >= first_price * 0.95; // Not declining more than 5%

        if price_range < 15.0 && volume_increasing && price_stable {
            let confidence = (85.0 - price_range) as u8;

            return Some(PatternResult {
                pattern: Pattern::Accumulation,
                confidence: confidence.min(80),
                description: format!(
                    "Accumulation phase: {:.1}% range, increasing volume",
                    price_range
                ),
                trade_recommendation: TradeRecommendation::StrongBuy,
            });
        }

        None
    }

    /// Detect distribution pattern
    /// Characteristics:
    /// - Price topping out
    /// - High volume with no price increase
    /// - Beginning of downtrend
    fn detect_distribution(&self) -> Option<PatternResult> {
        if self.price_history.len() < 10 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().map(|p| p.price).collect();
        let volumes: Vec<f64> = self.price_history.iter().map(|p| p.volume).collect();

        // Check if price is topping out (recent prices similar but not increasing)
        let recent_prices = &prices[prices.len() - 5..];
        let max_recent = recent_prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_recent = recent_prices.iter().cloned().fold(f64::INFINITY, f64::min);
        let recent_range = (max_recent - min_recent) / min_recent * 100.0;

        // Check volume is high
        let recent_volume: f64 = volumes[volumes.len() - 5..].iter().sum::<f64>() / 5.0;
        let avg_volume: f64 = volumes[..volumes.len() - 5].iter().sum::<f64>()
            / (volumes.len() - 5) as f64;

        let high_volume = recent_volume > avg_volume * 1.3;

        // Check for beginning of downtrend
        let price_declining = recent_prices[recent_prices.len() - 1]
            < recent_prices[0] * 0.98;

        if recent_range < 10.0 && high_volume && price_declining {
            let confidence = 70;

            return Some(PatternResult {
                pattern: Pattern::Distribution,
                confidence,
                description: "Distribution phase detected: high volume, no gains".to_string(),
                trade_recommendation: TradeRecommendation::Sell,
            });
        }

        None
    }

    /// Detect consolidation pattern
    /// Characteristics:
    /// - Narrow price range
    /// - Low volume
    /// - Typically precedes a move
    fn detect_consolidation(&self) -> Option<PatternResult> {
        if self.price_history.len() < 8 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().map(|p| p.price).collect();
        let volumes: Vec<f64> = self.price_history.iter().map(|p| p.volume).collect();

        // Calculate price range
        let max_price = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_price = prices.iter().cloned().fold(f64::INFINITY, f64::min);
        let price_range = (max_price - min_price) / min_price * 100.0;

        // Check volume is decreasing
        let recent_volume: f64 = volumes[volumes.len() - 4..].iter().sum::<f64>() / 4.0;
        let earlier_volume: f64 = volumes[..volumes.len() - 4].iter().sum::<f64>()
            / (volumes.len() - 4) as f64;

        let volume_decreasing = recent_volume < earlier_volume * 0.8;

        if price_range < 8.0 && volume_decreasing {
            let confidence = 60;

            return Some(PatternResult {
                pattern: Pattern::Consolidation,
                confidence,
                description: format!(
                    "Consolidation: {:.1}% range, low volume",
                    price_range
                ),
                trade_recommendation: TradeRecommendation::Hold,
            });
        }

        None
    }

    /// Get the current price trend (for last N points)
    pub fn get_price_trend(&self, num_points: usize) -> f64 {
        if self.price_history.len() < num_points {
            return 0.0;
        }

        let start_idx = self.price_history.len() - num_points;
        let start_price = self.price_history[start_idx].price;
        let end_price = self.price_history[self.price_history.len() - 1].price;

        ((end_price - start_price) / start_price) * 100.0
    }

    /// Calculate average volume for last N points
    pub fn get_average_volume(&self, num_points: usize) -> f64 {
        if self.price_history.is_empty() {
            return 0.0;
        }

        let start_idx = if num_points >= self.price_history.len() {
            0
        } else {
            self.price_history.len() - num_points
        };

        let sum: f64 = self.price_history
            .iter()
            .skip(start_idx)
            .map(|p| p.volume)
            .sum();

        sum / (self.price_history.len() - start_idx) as f64
    }

    /// Get number of price points in history
    pub fn history_size(&self) -> usize {
        self.price_history.len()
    }

    /// Clear price history
    pub fn clear(&mut self) {
        self.price_history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pump_and_dump_detection() {
        let mut detector = PatternDetector::new(20);

        // Simulate pump and dump
        // Normal prices
        for i in 0..5 {
            detector.add_price_point(i * 1000, 1.0, 1000.0);
        }

        // Pump (rapid rise)
        detector.add_price_point(5000, 1.2, 2000.0);
        detector.add_price_point(6000, 1.5, 3000.0);
        detector.add_price_point(7000, 2.0, 5000.0); // Peak

        // Dump (rapid fall)
        detector.add_price_point(8000, 1.5, 2000.0);
        detector.add_price_point(9000, 1.2, 1500.0);
        detector.add_price_point(10000, 1.0, 1000.0);

        let result = detector.detect_pattern();
        assert_eq!(result.pattern, Pattern::PumpAndDump);
        assert!(result.confidence > 50);
        assert_eq!(result.trade_recommendation, TradeRecommendation::Avoid);
    }

    #[test]
    fn test_breakout_detection() {
        let mut detector = PatternDetector::new(20);

        // Consolidation around 1.0
        for i in 0..9 {
            detector.add_price_point(i * 1000, 1.0, 1000.0);
        }

        // Breakout
        detector.add_price_point(9000, 1.03, 2000.0);

        let result = detector.detect_pattern();
        assert_eq!(result.pattern, Pattern::Breakout);
        assert!(result.confidence > 30);
    }

    #[test]
    fn test_accumulation_detection() {
        let mut detector = PatternDetector::new(20);

        // Narrow price range with increasing volume
        for i in 0..10 {
            let price = 1.0 + (i as f64 * 0.005); // Very small price increase
            let volume = 1000.0 + (i as f64 * 100.0); // Increasing volume
            detector.add_price_point(i * 1000, price, volume);
        }

        let result = detector.detect_pattern();
        assert_eq!(result.pattern, Pattern::Accumulation);
        assert_eq!(result.trade_recommendation, TradeRecommendation::StrongBuy);
    }

    #[test]
    fn test_insufficient_data() {
        let detector = PatternDetector::new(20);

        let result = detector.detect_pattern();
        assert_eq!(result.pattern, Pattern::None);
        assert_eq!(result.confidence, 0);
    }
}
