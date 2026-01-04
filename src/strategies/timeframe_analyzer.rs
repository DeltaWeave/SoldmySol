/// Multi-Timeframe Analysis
///
/// Analyzes price action across multiple timeframes to get a complete picture:
/// - 5-minute: Short-term momentum
/// - 15-minute: Medium-term trends
/// - 1-hour: Long-term context
///
/// Provides alignment analysis: when all timeframes agree, signals are stronger

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Timeframe {
    FiveMinute,    // 5 minutes
    FifteenMinute, // 15 minutes
    OneHour,       // 60 minutes
}

impl Timeframe {
    pub fn duration_ms(&self) -> i64 {
        match self {
            Timeframe::FiveMinute => 5 * 60 * 1000,
            Timeframe::FifteenMinute => 15 * 60 * 1000,
            Timeframe::OneHour => 60 * 60 * 1000,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Timeframe::FiveMinute => "5m",
            Timeframe::FifteenMinute => "15m",
            Timeframe::OneHour => "1h",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimeframeTrend {
    StrongBullish,   // +5% or more
    Bullish,         // +2% to +5%
    Neutral,         // -2% to +2%
    Bearish,         // -5% to -2%
    StrongBearish,   // -5% or worse
}

impl TimeframeTrend {
    pub fn from_percent_change(change: f64) -> Self {
        if change >= 5.0 {
            TimeframeTrend::StrongBullish
        } else if change >= 2.0 {
            TimeframeTrend::Bullish
        } else if change >= -2.0 {
            TimeframeTrend::Neutral
        } else if change >= -5.0 {
            TimeframeTrend::Bearish
        } else {
            TimeframeTrend::StrongBearish
        }
    }

    pub fn is_bullish(&self) -> bool {
        matches!(self, TimeframeTrend::Bullish | TimeframeTrend::StrongBullish)
    }

    pub fn is_bearish(&self) -> bool {
        matches!(self, TimeframeTrend::Bearish | TimeframeTrend::StrongBearish)
    }
}

#[derive(Debug, Clone)]
pub struct TimeframeData {
    pub timeframe: Timeframe,
    pub trend: TimeframeTrend,
    pub percent_change: f64,
    pub volume_trend: f64, // Percent change in volume
    pub momentum: f64,     // Rate of change
}

#[derive(Debug, Clone)]
pub struct MultiTimeframeAnalysis {
    pub five_minute: TimeframeData,
    pub fifteen_minute: TimeframeData,
    pub one_hour: TimeframeData,
    pub alignment_score: u8, // 0-100, higher = more aligned
    pub overall_trend: TimeframeTrend,
    pub trade_signal: TradeSignal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TradeSignal {
    StrongBuy,   // All timeframes bullish
    Buy,         // Majority bullish
    Hold,        // Mixed or neutral
    Sell,        // Majority bearish
    StrongSell,  // All timeframes bearish
}

#[derive(Debug, Clone)]
struct PricePoint {
    timestamp: i64,
    price: f64,
    volume: f64,
}

pub struct TimeframeAnalyzer {
    price_history: VecDeque<PricePoint>,
    max_history_ms: i64,
}

impl TimeframeAnalyzer {
    pub fn new() -> Self {
        // Store 2 hours of data (enough for 1h timeframe with buffer)
        let max_history_ms = 2 * 60 * 60 * 1000;

        Self {
            price_history: VecDeque::new(),
            max_history_ms,
        }
    }

    /// Add a new price point
    pub fn add_price(&mut self, timestamp: i64, price: f64, volume: f64) {
        // Remove old data
        let cutoff = timestamp - self.max_history_ms;
        while let Some(first) = self.price_history.front() {
            if first.timestamp < cutoff {
                self.price_history.pop_front();
            } else {
                break;
            }
        }

        // Add new point
        self.price_history.push_back(PricePoint {
            timestamp,
            price,
            volume,
        });
    }

    /// Analyze across all timeframes
    pub fn analyze(&self, current_timestamp: i64) -> Option<MultiTimeframeAnalysis> {
        if self.price_history.is_empty() {
            return None;
        }

        // Analyze each timeframe
        let five_min = self.analyze_timeframe(Timeframe::FiveMinute, current_timestamp)?;
        let fifteen_min = self.analyze_timeframe(Timeframe::FifteenMinute, current_timestamp)?;
        let one_hour = self.analyze_timeframe(Timeframe::OneHour, current_timestamp)?;

        // Calculate alignment score
        let alignment_score = self.calculate_alignment(&five_min, &fifteen_min, &one_hour);

        // Determine overall trend (weighted by timeframe)
        let overall_trend = self.calculate_overall_trend(&five_min, &fifteen_min, &one_hour);

        // Generate trade signal
        let trade_signal = self.generate_trade_signal(&five_min, &fifteen_min, &one_hour, alignment_score);

        Some(MultiTimeframeAnalysis {
            five_minute: five_min,
            fifteen_minute: fifteen_min,
            one_hour: one_hour,
            alignment_score,
            overall_trend,
            trade_signal,
        })
    }

    /// Analyze a specific timeframe
    fn analyze_timeframe(&self, timeframe: Timeframe, current_timestamp: i64) -> Option<TimeframeData> {
        let duration_ms = timeframe.duration_ms();
        let start_time = current_timestamp - duration_ms;

        // Get data points within this timeframe
        let points: Vec<&PricePoint> = self.price_history
            .iter()
            .filter(|p| p.timestamp >= start_time)
            .collect();

        if points.is_empty() {
            return None;
        }

        // Calculate price change
        let first_price = points[0].price;
        let last_price = points[points.len() - 1].price;
        let percent_change = ((last_price - first_price) / first_price) * 100.0;

        // Calculate volume trend
        let mid_point = points.len() / 2;
        let first_half_volume: f64 = points[..mid_point].iter().map(|p| p.volume).sum::<f64>()
            / mid_point as f64;
        let second_half_volume: f64 = points[mid_point..].iter().map(|p| p.volume).sum::<f64>()
            / (points.len() - mid_point) as f64;
        let volume_trend = ((second_half_volume - first_half_volume) / first_half_volume) * 100.0;

        // Calculate momentum (rate of change over last quarter of timeframe)
        let quarter_point = (points.len() * 3) / 4;
        let quarter_price = points[quarter_point].price;
        let momentum = ((last_price - quarter_price) / quarter_price) * 100.0;

        Some(TimeframeData {
            timeframe,
            trend: TimeframeTrend::from_percent_change(percent_change),
            percent_change,
            volume_trend,
            momentum,
        })
    }

    /// Calculate how aligned the timeframes are
    fn calculate_alignment(
        &self,
        five_min: &TimeframeData,
        fifteen_min: &TimeframeData,
        one_hour: &TimeframeData,
    ) -> u8 {
        let mut score = 0u8;

        // Check if all trends are in the same direction
        let all_bullish = five_min.trend.is_bullish()
            && fifteen_min.trend.is_bullish()
            && one_hour.trend.is_bullish();

        let all_bearish = five_min.trend.is_bearish()
            && fifteen_min.trend.is_bearish()
            && one_hour.trend.is_bearish();

        if all_bullish || all_bearish {
            score += 60; // Strong alignment
        } else if (five_min.trend.is_bullish() && fifteen_min.trend.is_bullish())
            || (fifteen_min.trend.is_bullish() && one_hour.trend.is_bullish())
        {
            score += 40; // Partial alignment (bullish)
        } else if (five_min.trend.is_bearish() && fifteen_min.trend.is_bearish())
            || (fifteen_min.trend.is_bearish() && one_hour.trend.is_bearish())
        {
            score += 40; // Partial alignment (bearish)
        } else {
            score += 10; // Weak alignment
        }

        // Add points for momentum alignment
        let momentum_aligned = (five_min.momentum > 0.0 && fifteen_min.momentum > 0.0 && one_hour.momentum > 0.0)
            || (five_min.momentum < 0.0 && fifteen_min.momentum < 0.0 && one_hour.momentum < 0.0);

        if momentum_aligned {
            score += 25;
        }

        // Add points for volume alignment
        let volume_aligned = five_min.volume_trend > 0.0
            && fifteen_min.volume_trend > 0.0;

        if volume_aligned {
            score += 15;
        }

        score.min(100)
    }

    /// Calculate overall trend (weighted by timeframe)
    fn calculate_overall_trend(
        &self,
        five_min: &TimeframeData,
        fifteen_min: &TimeframeData,
        one_hour: &TimeframeData,
    ) -> TimeframeTrend {
        // Weight: 5m = 20%, 15m = 30%, 1h = 50%
        let weighted_change = five_min.percent_change * 0.2
            + fifteen_min.percent_change * 0.3
            + one_hour.percent_change * 0.5;

        TimeframeTrend::from_percent_change(weighted_change)
    }

    /// Generate trade signal based on timeframe analysis
    fn generate_trade_signal(
        &self,
        five_min: &TimeframeData,
        fifteen_min: &TimeframeData,
        one_hour: &TimeframeData,
        alignment_score: u8,
    ) -> TradeSignal {
        // Count bullish and bearish timeframes
        let bullish_count = [five_min, fifteen_min, one_hour]
            .iter()
            .filter(|tf| tf.trend.is_bullish())
            .count();

        let bearish_count = [five_min, fifteen_min, one_hour]
            .iter()
            .filter(|tf| tf.trend.is_bearish())
            .count();

        // Strong signals require high alignment
        if alignment_score >= 80 {
            if bullish_count == 3 {
                return TradeSignal::StrongBuy;
            } else if bearish_count == 3 {
                return TradeSignal::StrongSell;
            }
        }

        // Regular signals
        if bullish_count >= 2 && alignment_score >= 50 {
            TradeSignal::Buy
        } else if bearish_count >= 2 && alignment_score >= 50 {
            TradeSignal::Sell
        } else {
            TradeSignal::Hold
        }
    }

    /// Get the number of data points currently stored
    pub fn data_points(&self) -> usize {
        self.price_history.len()
    }

    /// Clear all historical data
    pub fn clear(&mut self) {
        self.price_history.clear();
    }
}

impl Default for TimeframeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeframe_trend_from_percent() {
        assert_eq!(
            TimeframeTrend::from_percent_change(6.0),
            TimeframeTrend::StrongBullish
        );
        assert_eq!(
            TimeframeTrend::from_percent_change(3.0),
            TimeframeTrend::Bullish
        );
        assert_eq!(
            TimeframeTrend::from_percent_change(0.0),
            TimeframeTrend::Neutral
        );
        assert_eq!(
            TimeframeTrend::from_percent_change(-3.0),
            TimeframeTrend::Bearish
        );
        assert_eq!(
            TimeframeTrend::from_percent_change(-6.0),
            TimeframeTrend::StrongBearish
        );
    }

    #[test]
    fn test_add_price_and_cleanup() {
        let mut analyzer = TimeframeAnalyzer::new();

        let now = 1000000;

        // Add some recent prices
        for i in 0..10 {
            analyzer.add_price(now + i * 60000, 100.0 + i as f64, 1000.0);
        }

        assert_eq!(analyzer.data_points(), 10);

        // Add a price far in the future (should trigger cleanup)
        let far_future = now + 3 * 60 * 60 * 1000; // 3 hours later
        analyzer.add_price(far_future, 110.0, 1000.0);

        // Old data should be cleaned up (older than 2 hours)
        assert!(analyzer.data_points() < 10);
    }

    #[test]
    fn test_bullish_alignment() {
        let mut analyzer = TimeframeAnalyzer::new();

        let now = 1000000;

        // Create uptrend: prices gradually increasing
        let base_time = now - 2 * 60 * 60 * 1000; // Start 2 hours ago

        for i in 0..120 {
            let timestamp = base_time + (i * 60 * 1000); // Every minute
            let price = 100.0 + (i as f64 * 0.1); // Gradual increase
            analyzer.add_price(timestamp, price, 1000.0);
        }

        let analysis = analyzer.analyze(now).unwrap();

        // Should detect bullish trend
        assert!(analysis.five_minute.trend.is_bullish());
        assert!(analysis.alignment_score > 50);
        assert!(matches!(
            analysis.trade_signal,
            TradeSignal::Buy | TradeSignal::StrongBuy
        ));
    }

    #[test]
    fn test_bearish_alignment() {
        let mut analyzer = TimeframeAnalyzer::new();

        let now = 1000000;
        let base_time = now - 2 * 60 * 60 * 1000; // Start 2 hours ago

        // Create downtrend: prices gradually decreasing
        for i in 0..120 {
            let timestamp = base_time + (i * 60 * 1000); // Every minute
            let price = 100.0 - (i as f64 * 0.1); // Gradual decrease
            analyzer.add_price(timestamp, price, 1000.0);
        }

        let analysis = analyzer.analyze(now).unwrap();

        // Should detect bearish trend
        assert!(analysis.five_minute.trend.is_bearish());
        assert!(matches!(
            analysis.trade_signal,
            TradeSignal::Sell | TradeSignal::StrongSell
        ));
    }

    #[test]
    fn test_insufficient_data() {
        let analyzer = TimeframeAnalyzer::new();

        let now = 1000000;
        let analysis = analyzer.analyze(now);

        assert!(analysis.is_none());
    }
}
