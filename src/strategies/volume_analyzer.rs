
#[derive(Debug, Clone)]
pub struct VolumeProfile {
    pub volume_1h: f64,
    pub volume_6h: f64,
    pub volume_24h: f64,
    pub volume_trend: VolumeTrend,
    pub buy_sell_ratio: f64,
    pub quality_score: u8,  // 0-100
}

#[derive(Debug, PartialEq, Clone)]
pub enum VolumeTrend {
    Accelerating,   // Volume increasing - bullish
    Steady,         // Consistent volume - neutral
    Declining,      // Volume dropping - bearish
    Suspicious,     // Irregular patterns - avoid
}

pub struct VolumeAnalyzer;

impl VolumeAnalyzer {
    /// Analyze volume patterns to detect fake/wash trading
    /// Returns a VolumeProfile with quality score
    pub fn analyze_volume_profile(
        volume_1h: f64,
        volume_6h: f64,
        volume_24h: f64,
        liquidity_sol: f64,
    ) -> VolumeProfile {
        let mut quality_score = 100u8;

        // Use actual 1h/6h volumes when available, fallback to 24h estimates
        let volume_1h = if volume_1h > 0.0 { volume_1h } else { volume_24h / 24.0 };
        let volume_6h = if volume_6h > 0.0 { volume_6h } else { volume_24h / 4.0 };

        // Check volume to liquidity ratio
        let volume_liquidity_ratio = if liquidity_sol > 0.0 {
            volume_24h / (liquidity_sol * 150.0) // Assume SOL = $150
        } else {
            0.0
        };

        // Suspicious if volume is too high or too low relative to liquidity
        if volume_liquidity_ratio > 100.0 {
            quality_score -= 30; // Likely wash trading
        } else if volume_liquidity_ratio < 0.1 {
            quality_score -= 25; // Very low activity
        } else if volume_liquidity_ratio > 10.0 {
            quality_score += 10; // Good volume
        }

        // Determine trend using short-term vs mid-term volume acceleration
        let volume_accel = if volume_6h > 0.0 {
            (volume_1h * 6.0) / volume_6h
        } else {
            1.0
        };

        let trend = if volume_accel > 1.5 && volume_liquidity_ratio > 5.0 {
            VolumeTrend::Accelerating
        } else if volume_accel < 0.7 || volume_liquidity_ratio < 0.5 {
            VolumeTrend::Declining
        } else if volume_liquidity_ratio > 5.0 {
            VolumeTrend::Steady
        } else {
            VolumeTrend::Steady
        };

        // Estimate buy/sell ratio (simplified)
        let buy_sell_ratio = Self::estimate_buy_sell_ratio(volume_liquidity_ratio, volume_accel);

        // Penalize extreme buy/sell imbalances
        if buy_sell_ratio < 0.3 || buy_sell_ratio > 3.0 {
            quality_score = quality_score.saturating_sub(25); // Extreme imbalance
        }

        // Check for minimum volume threshold
        if volume_24h < 1000.0 {
            quality_score = quality_score.saturating_sub(20); // Too low volume
        }

        VolumeProfile {
            volume_1h,
            volume_6h,
            volume_24h,
            volume_trend: trend,
            buy_sell_ratio,
            quality_score,
        }
    }

    /// Estimate buy/sell ratio from volume patterns
    /// Returns ratio where 1.0 = balanced, >1.0 = more buying, <1.0 = more selling
    fn estimate_buy_sell_ratio(volume_liquidity_ratio: f64, volume_accel: f64) -> f64 {
        // Use liquidity-adjusted volume and acceleration to infer order flow skew.
        // Higher ratio and acceleration suggests stronger buy pressure.
        let base_ratio: f64 = if volume_liquidity_ratio > 15.0 {
            1.6 // Strong buying
        } else if volume_liquidity_ratio > 5.0 {
            1.2 // Moderate buying
        } else if volume_liquidity_ratio < 1.0 {
            0.7 // Selling pressure
        } else {
            1.0 // Balanced
        };

        let accel_adjustment = if volume_accel > 1.2 {
            0.2
        } else if volume_accel < 0.8 {
            -0.2
        } else {
            0.0
        };

        (base_ratio + accel_adjustment).clamp(0.1, 10.0)
    }

    /// Check if volume pattern is suspicious (wash trading indicators)
    pub fn is_wash_trading(volume_24h: f64, liquidity_sol: f64, holder_count: usize) -> bool {
        let volume_liquidity_ratio = if liquidity_sol > 0.0 {
            volume_24h / (liquidity_sol * 150.0)
        } else {
            0.0
        };

        // Red flags for wash trading:
        // 1. Extremely high volume vs liquidity with few holders
        if volume_liquidity_ratio > 100.0 && holder_count < 50 {
            return true;
        }

        // 2. Very low holder count but high volume
        if holder_count < 20 && volume_24h > 50000.0 {
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_profile_healthy() {
        let profile = VolumeAnalyzer::analyze_volume_profile(2000.0, 12000.0, 50000.0, 25.0);
        assert!(profile.quality_score >= 60);
        assert_eq!(profile.volume_trend, VolumeTrend::Steady);
    }

    #[test]
    fn test_volume_profile_suspicious() {
        let profile = VolumeAnalyzer::analyze_volume_profile(50000.0, 200000.0, 500000.0, 1.0);
        // Very high volume vs liquidity = suspicious
        assert!(profile.quality_score < 80);
    }

    #[test]
    fn test_wash_trading_detection() {
        // High volume, low liquidity, few holders = wash trading
        assert!(VolumeAnalyzer::is_wash_trading(1000000.0, 2.0, 15));

        // Normal pattern = not wash trading
        assert!(!VolumeAnalyzer::is_wash_trading(50000.0, 25.0, 100));
    }
}
