/// Machine Learning Feature Engineering
///
/// Extracts 15 features from token data for ML model training and prediction:
///
/// Price Features (3):
/// 1. price_change_5m - Short-term price momentum
/// 2. price_change_15m - Medium-term price momentum
/// 3. price_change_1h - Long-term price trend
///
/// Volume Features (3):
/// 4. volume_24h_normalized - Trading activity level
/// 5. volume_trend - Volume acceleration/deceleration
/// 6. buy_sell_ratio - Buy pressure vs sell pressure
///
/// Liquidity Features (2):
/// 7. liquidity_sol - Pool depth
/// 8. volume_to_liquidity_ratio - Turnover rate
///
/// Safety Features (4):
/// 9. holder_concentration - Token distribution
/// 10. holder_count_normalized - Community size
/// 11. has_mint_authority - Rug pull risk (0 or 1)
/// 12. has_freeze_authority - Token freezing risk (0 or 1)
///
/// Market Features (3):
/// 13. market_sentiment_index - Overall market condition
/// 14. fear_greed_index - Market psychology
/// 15. pattern_confidence - Detected pattern strength

use crate::strategies::{VolumeProfile, MarketSentiment, PatternResult};
use crate::services::TokenSafetyCheck;

#[derive(Debug, Clone)]
pub struct FeatureVector {
    pub features: [f64; 15],
    pub feature_names: [&'static str; 15],
}

impl FeatureVector {
    /// Feature names for logging and debugging
    pub const FEATURE_NAMES: [&'static str; 15] = [
        "price_change_5m",
        "price_change_15m",
        "price_change_1h",
        "volume_24h_normalized",
        "volume_trend",
        "buy_sell_ratio",
        "liquidity_sol",
        "volume_to_liquidity_ratio",
        "holder_concentration",
        "holder_count_normalized",
        "has_mint_authority",
        "has_freeze_authority",
        "market_sentiment_index",
        "fear_greed_index",
        "pattern_confidence",
    ];

    pub fn new(features: [f64; 15]) -> Self {
        Self {
            features,
            feature_names: Self::FEATURE_NAMES,
        }
    }

    /// Get a specific feature by index
    pub fn get(&self, index: usize) -> Option<f64> {
        self.features.get(index).copied()
    }

    /// Get feature by name
    pub fn get_by_name(&self, name: &str) -> Option<f64> {
        self.feature_names
            .iter()
            .position(|&n| n == name)
            .and_then(|idx| self.get(idx))
    }

    /// Normalize all features to 0-1 range (min-max scaling)
    pub fn normalize(&mut self, min_values: &[f64; 15], max_values: &[f64; 15]) {
        for i in 0..15 {
            let min = min_values[i];
            let max = max_values[i];
            if max > min {
                self.features[i] = (self.features[i] - min) / (max - min);
            } else {
                self.features[i] = 0.0;
            }
        }
    }

    /// Convert to vector for easier manipulation
    pub fn to_vec(&self) -> Vec<f64> {
        self.features.to_vec()
    }
}

pub struct FeatureExtractor;

impl FeatureExtractor {
    /// Extract all features from token data
    pub fn extract_features(
        price_changes: (f64, f64, f64), // (5m, 15m, 1h)
        volume_profile: &VolumeProfile,
        liquidity_sol: f64,
        volume_24h: f64,
        holder_count: usize,
        holder_concentration: f64, // Top holder percentage
        safety_check: &TokenSafetyCheck,
        market_sentiment: &MarketSentiment,
        pattern_result: &PatternResult,
    ) -> FeatureVector {
        let mut features = [0.0f64; 15];

        // Price features (0-2)
        features[0] = Self::normalize_price_change(price_changes.0);
        features[1] = Self::normalize_price_change(price_changes.1);
        features[2] = Self::normalize_price_change(price_changes.2);

        // Volume features (3-5)
        features[3] = Self::normalize_volume(volume_24h);
        features[4] = Self::volume_trend_to_feature(&volume_profile.volume_trend);
        features[5] = Self::normalize_ratio(volume_profile.buy_sell_ratio);

        // Liquidity features (6-7)
        features[6] = Self::normalize_liquidity(liquidity_sol);
        features[7] = Self::normalize_ratio(volume_24h / (liquidity_sol * 150.0)); // Assuming SOL ~$150

        // Safety features (8-11)
        features[8] = holder_concentration / 100.0; // Already in percentage
        features[9] = Self::normalize_holder_count(holder_count);
        features[10] = if safety_check.risk_score > 40 { 1.0 } else { 0.0 }; // Simplified mint authority check
        features[11] = if safety_check.risk_score > 40 { 1.0 } else { 0.0 }; // Simplified freeze authority check

        // Market features (12-14)
        features[12] = Self::sentiment_to_feature(market_sentiment);
        features[13] = market_sentiment.market_fear_greed as f64 / 100.0;
        features[14] = pattern_result.confidence as f64 / 100.0;

        FeatureVector::new(features)
    }

    /// Normalize price change to 0-1 range
    /// Assumes price changes between -50% and +50%
    fn normalize_price_change(change_percent: f64) -> f64 {
        let clamped = change_percent.clamp(-50.0, 50.0);
        (clamped + 50.0) / 100.0
    }

    /// Normalize volume to 0-1 range
    /// Assumes volume between $0 and $1M
    fn normalize_volume(volume: f64) -> f64 {
        let clamped = volume.clamp(0.0, 1_000_000.0);
        clamped / 1_000_000.0
    }

    /// Normalize liquidity to 0-1 range
    /// Assumes liquidity between 0 and 1000 SOL
    fn normalize_liquidity(liquidity: f64) -> f64 {
        let clamped = liquidity.clamp(0.0, 1000.0);
        clamped / 1000.0
    }

    /// Normalize holder count to 0-1 range
    /// Assumes holder count between 0 and 10,000
    fn normalize_holder_count(count: usize) -> f64 {
        let clamped = (count as f64).clamp(0.0, 10_000.0);
        clamped / 10_000.0
    }

    /// Normalize ratio to 0-1 range
    /// Assumes ratios between 0 and 10
    fn normalize_ratio(ratio: f64) -> f64 {
        let clamped = ratio.clamp(0.0, 10.0);
        clamped / 10.0
    }

    /// Convert volume trend to numeric feature
    fn volume_trend_to_feature(trend: &crate::strategies::VolumeTrend) -> f64 {
        use crate::strategies::VolumeTrend;
        match trend {
            VolumeTrend::Accelerating => 1.0,
            VolumeTrend::Steady => 0.5,
            VolumeTrend::Declining => 0.25,
            VolumeTrend::Suspicious => 0.0,
        }
    }

    /// Convert market sentiment to numeric feature
    fn sentiment_to_feature(sentiment: &MarketSentiment) -> f64 {
        use crate::strategies::Trend;
        match sentiment.sol_trend {
            Trend::StrongBullish => 1.0,
            Trend::Bullish => 0.75,
            Trend::Neutral => 0.5,
            Trend::Bearish => 0.25,
            Trend::StrongBearish => 0.0,
        }
    }
}

/// Feature statistics for normalization
#[derive(Debug, Clone)]
pub struct FeatureStats {
    pub min_values: [f64; 15],
    pub max_values: [f64; 15],
    pub mean_values: [f64; 15],
    pub std_values: [f64; 15],
    pub sample_count: usize,
}

impl FeatureStats {
    pub fn new() -> Self {
        Self {
            min_values: [f64::INFINITY; 15],
            max_values: [f64::NEG_INFINITY; 15],
            mean_values: [0.0; 15],
            std_values: [0.0; 15],
            sample_count: 0,
        }
    }

    /// Update statistics with a new feature vector
    pub fn update(&mut self, features: &FeatureVector) {
        for i in 0..15 {
            let value = features.features[i];

            // Update min/max
            if value < self.min_values[i] {
                self.min_values[i] = value;
            }
            if value > self.max_values[i] {
                self.max_values[i] = value;
            }

            // Update running mean (we'll compute std later if needed)
            let old_mean = self.mean_values[i];
            self.mean_values[i] = old_mean + (value - old_mean) / (self.sample_count + 1) as f64;
        }

        self.sample_count += 1;
    }

    /// Save statistics to a string format (for persistence)
    pub fn to_string(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.min_values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            self.max_values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            self.mean_values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            self.std_values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            self.sample_count
        )
    }

    /// Load statistics from a string format
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() != 5 {
            return None;
        }

        let parse_array = |s: &str| -> Option<[f64; 15]> {
            let values: Vec<f64> = s.split(',').filter_map(|v| v.parse().ok()).collect();
            if values.len() == 15 {
                let mut array = [0.0; 15];
                array.copy_from_slice(&values);
                Some(array)
            } else {
                None
            }
        };

        Some(Self {
            min_values: parse_array(parts[0])?,
            max_values: parse_array(parts[1])?,
            mean_values: parse_array(parts[2])?,
            std_values: parse_array(parts[3])?,
            sample_count: parts[4].parse().ok()?,
        })
    }
}

impl Default for FeatureStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::{VolumeProfile, VolumeTrend, MarketSentiment, Trend};
    use crate::strategies::{PatternResult, Pattern, TradeRecommendation};
    use crate::services::TokenSafetyCheck;

    #[test]
    fn test_feature_extraction() {
        let volume_profile = VolumeProfile {
            volume_trend: VolumeTrend::Accelerating,
            buy_sell_ratio: 1.5,
            quality_score: 80,
        };

        let safety_check = TokenSafetyCheck {
            passed: true,
            reasons: vec![],
            risk_score: 10,
        };

        let sentiment = MarketSentiment {
            sol_trend: Trend::Bullish,
            sol_change_1h: 2.5,
            sol_change_24h: 5.0,
            fear_greed_index: 70,
            trading_recommended: true,
        };

        let pattern = PatternResult {
            pattern: Pattern::Breakout,
            confidence: 75,
            description: "Test".to_string(),
            trade_recommendation: TradeRecommendation::Buy,
        };

        let features = FeatureExtractor::extract_features(
            (2.0, 3.0, 5.0),
            &volume_profile,
            100.0,
            50000.0,
            500,
            15.0,
            &safety_check,
            &sentiment,
            &pattern,
        );

        assert_eq!(features.features.len(), 15);

        // Check that all features are in valid range (0-1 or close to it)
        for (i, &feature) in features.features.iter().enumerate() {
            assert!(
                feature >= 0.0 && feature <= 1.5,
                "Feature {} ({}) out of range: {}",
                i,
                features.feature_names[i],
                feature
            );
        }
    }

    #[test]
    fn test_feature_stats() {
        let mut stats = FeatureStats::new();

        let features1 = FeatureVector::new([1.0; 15]);
        let features2 = FeatureVector::new([2.0; 15]);
        let features3 = FeatureVector::new([3.0; 15]);

        stats.update(&features1);
        stats.update(&features2);
        stats.update(&features3);

        assert_eq!(stats.sample_count, 3);
        assert_eq!(stats.min_values[0], 1.0);
        assert_eq!(stats.max_values[0], 3.0);
        assert!((stats.mean_values[0] - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_feature_stats_serialization() {
        let stats = FeatureStats {
            min_values: [1.0; 15],
            max_values: [10.0; 15],
            mean_values: [5.0; 15],
            std_values: [2.0; 15],
            sample_count: 100,
        };

        let serialized = stats.to_string();
        let deserialized = FeatureStats::from_string(&serialized).unwrap();

        assert_eq!(deserialized.sample_count, 100);
        assert_eq!(deserialized.min_values[0], 1.0);
        assert_eq!(deserialized.max_values[0], 10.0);
    }

    #[test]
    fn test_get_feature_by_name() {
        let features = FeatureVector::new([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 0.0, 0.0, 0.5, 0.7, 0.8]);

        assert_eq!(features.get_by_name("price_change_5m"), Some(0.1));
        assert_eq!(features.get_by_name("liquidity_sol"), Some(0.7));
        assert_eq!(features.get_by_name("nonexistent"), None);
    }
}
