use anyhow::Result;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct MarketSentiment {
    pub sol_trend: Trend,
    pub sol_price_change_1h: f64,
    pub sol_price_change_24h: f64,
    pub market_fear_greed: u8,  // 0-100 (0=extreme fear, 100=extreme greed)
    pub trading_recommended: bool,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Trend {
    StrongBullish,
    Bullish,
    Neutral,
    Bearish,
    StrongBearish,
}

pub struct SentimentAnalyzer {
    sol_price_history: VecDeque<(i64, f64)>,  // (timestamp, price)
    max_history: usize,
}

impl SentimentAnalyzer {
    pub fn new() -> Self {
        Self {
            sol_price_history: VecDeque::with_capacity(288), // 24h at 5-min intervals
            max_history: 288,
        }
    }

    /// Update SOL price and analyze market sentiment
    pub fn update_sentiment(&mut self, current_sol_price: f64) -> Result<MarketSentiment> {
        let now = chrono::Utc::now().timestamp();

        // Add current price
        self.sol_price_history.push_back((now, current_sol_price));

        // Keep only recent history
        if self.sol_price_history.len() > self.max_history {
            self.sol_price_history.pop_front();
        }

        // Remove entries older than 24 hours
        let cutoff = now - 86400;
        while let Some(&(ts, _)) = self.sol_price_history.front() {
            if ts < cutoff {
                self.sol_price_history.pop_front();
            } else {
                break;
            }
        }

        // Calculate price changes
        let price_1h_ago = self.get_price_n_hours_ago(1, current_sol_price);
        let price_24h_ago = self.get_price_n_hours_ago(24, current_sol_price);

        let change_1h = ((current_sol_price - price_1h_ago) / price_1h_ago) * 100.0;
        let change_24h = ((current_sol_price - price_24h_ago) / price_24h_ago) * 100.0;

        // Determine trend
        let sol_trend = Self::determine_trend(change_1h, change_24h);

        // Calculate fear/greed index
        let market_fear_greed = Self::calculate_fear_greed_index(change_1h, change_24h);

        // Recommendation logic
        let trading_recommended = Self::should_trade(&sol_trend, market_fear_greed);

        Ok(MarketSentiment {
            sol_trend,
            sol_price_change_1h: change_1h,
            sol_price_change_24h: change_24h,
            market_fear_greed,
            trading_recommended,
        })
    }

    fn get_price_n_hours_ago(&self, hours: i64, current_price: f64) -> f64 {
        if self.sol_price_history.is_empty() {
            return current_price; // No history, assume no change
        }

        let target_time = chrono::Utc::now().timestamp() - (hours * 3600);

        // Find closest price to target time
        self.sol_price_history
            .iter()
            .min_by_key(|(ts, _)| (*ts - target_time).abs())
            .map(|(_, price)| *price)
            .unwrap_or(current_price)
    }

    fn determine_trend(change_1h: f64, change_24h: f64) -> Trend {
        if change_1h > 5.0 && change_24h > 10.0 {
            Trend::StrongBullish
        } else if change_1h > 2.0 || change_24h > 5.0 {
            Trend::Bullish
        } else if change_1h < -5.0 && change_24h < -10.0 {
            Trend::StrongBearish
        } else if change_1h < -2.0 || change_24h < -5.0 {
            Trend::Bearish
        } else {
            Trend::Neutral
        }
    }

    fn calculate_fear_greed_index(change_1h: f64, change_24h: f64) -> u8 {
        // Simplified fear/greed calculation
        // 50 = neutral, >50 = greed, <50 = fear
        let score = 50.0 + (change_1h * 2.0) + (change_24h * 1.0);
        score.clamp(0.0, 100.0) as u8
    }

    fn should_trade(trend: &Trend, fear_greed: u8) -> bool {
        match trend {
            Trend::StrongBullish | Trend::Bullish => true,
            Trend::Neutral => fear_greed > 40, // Only trade if leaning towards greed
            Trend::Bearish => fear_greed > 60, // Need strong greed to overcome bearish trend
            Trend::StrongBearish => false,     // Don't trade in strong bear market
        }
    }

    /// Get current market condition as string
    pub fn get_market_condition(&self, sentiment: &MarketSentiment) -> String {
        match sentiment.sol_trend {
            Trend::StrongBullish => format!("🚀 Strong Bull Market (SOL +{:.1}%)", sentiment.sol_price_change_24h),
            Trend::Bullish => format!("📈 Bullish (SOL +{:.1}%)", sentiment.sol_price_change_24h),
            Trend::Neutral => "➡️  Neutral Market".to_string(),
            Trend::Bearish => format!("📉 Bearish (SOL {:.1}%)", sentiment.sol_price_change_24h),
            Trend::StrongBearish => format!("💥 Strong Bear Market (SOL {:.1}%)", sentiment.sol_price_change_24h),
        }
    }
}

impl Default for SentimentAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trend_determination() {
        assert_eq!(Trend::determine_trend(6.0, 12.0), Trend::StrongBullish);
        assert_eq!(Trend::determine_trend(3.0, 4.0), Trend::Bullish);
        assert_eq!(Trend::determine_trend(0.5, 0.5), Trend::Neutral);
        assert_eq!(Trend::determine_trend(-3.0, -2.0), Trend::Bearish);
        assert_eq!(Trend::determine_trend(-6.0, -12.0), Trend::StrongBearish);
    }

    #[test]
    fn test_fear_greed_index() {
        assert_eq!(SentimentAnalyzer::calculate_fear_greed_index(5.0, 10.0), 70);
        assert_eq!(SentimentAnalyzer::calculate_fear_greed_index(0.0, 0.0), 50);
        assert_eq!(SentimentAnalyzer::calculate_fear_greed_index(-5.0, -10.0), 30);
    }

    #[test]
    fn test_trading_recommendations() {
        assert!(SentimentAnalyzer::should_trade(&Trend::StrongBullish, 50));
        assert!(SentimentAnalyzer::should_trade(&Trend::Bullish, 50));
        assert!(!SentimentAnalyzer::should_trade(&Trend::Neutral, 30));
        assert!(!SentimentAnalyzer::should_trade(&Trend::Bearish, 50));
        assert!(!SentimentAnalyzer::should_trade(&Trend::StrongBearish, 80));
    }
}
