use crate::services::token_safety::TokenSafetyCheck;
use crate::strategies::market_sentiment::MarketSentiment;
use crate::strategies::volume_analyzer::VolumeProfile;

pub struct PositionSizer;

impl PositionSizer {
    /// Calculate optimal position size based on confidence score
    /// Uses Kelly Criterion inspired approach
    pub fn calculate_position_size(
        base_amount: f64,
        confidence_score: u8,  // 0-100
        current_capital: f64,
        max_position_percent: f64,  // e.g., 0.05 for 5%
    ) -> f64 {
        // Convert confidence to 0-1 range
        let confidence_factor = (confidence_score as f64) / 100.0;

        // Scale from 0.5x to 1.5x base amount based on confidence
        // Low confidence (40) = 0.7x base
        // Medium confidence (70) = 1.2x base
        // High confidence (90) = 1.4x base
        let size_multiplier = 0.5 + confidence_factor;

        let proposed_size = base_amount * size_multiplier;
        let max_allowed = current_capital * max_position_percent;

        // Return the smaller of proposed size or maximum allowed
        proposed_size.min(max_allowed).max(base_amount * 0.3) // Never go below 30% of base
    }

    /// Calculate confidence score from multiple signals
    /// Returns score 0-100 where higher = more confident
    pub fn calculate_confidence(
        safety_check: &TokenSafetyCheck,
        volume_profile: &VolumeProfile,
        market_sentiment: &MarketSentiment,
        liquidity_sol: f64,
        holder_count: usize,
    ) -> u8 {
        let mut score = 0u8;

        // Safety check contribution (0-30 points)
        // Lower risk score = higher confidence
        let safety_score = 100 - safety_check.risk_score;
        score += ((safety_score as f64 * 0.3) as u8).min(30);

        // Volume quality (0-25 points)
        score += ((volume_profile.quality_score as f64 * 0.25) as u8).min(25);

        // Market sentiment (0-20 points)
        if market_sentiment.trading_recommended {
            score += 20;
        } else {
            // Partial credit based on fear/greed index
            score += ((market_sentiment.market_fear_greed as f64 * 0.2) as u8).min(20);
        }

        // Liquidity scoring (0-15 points)
        let liquidity_score = if liquidity_sol > 50.0 {
            15 // Excellent liquidity
        } else if liquidity_sol > 20.0 {
            12 // Good liquidity
        } else if liquidity_sol > 10.0 {
            8  // Acceptable liquidity
        } else if liquidity_sol > 5.0 {
            5  // Minimum liquidity
        } else {
            0  // Too low
        };
        score += liquidity_score;

        // Holder count contribution (0-10 points)
        let holder_score = if holder_count > 500 {
            10 // Many holders - good distribution
        } else if holder_count > 200 {
            8  // Good holder count
        } else if holder_count > 100 {
            6  // Decent holder count
        } else if holder_count > 50 {
            4  // Acceptable
        } else if holder_count > 20 {
            2  // Low but not terrible
        } else {
            0  // Too few holders
        };
        score += holder_score;

        score.min(100)
    }

    /// Get position size category for logging
    pub fn get_size_category(confidence: u8) -> &'static str {
        match confidence {
            90..=100 => "MAXIMUM (Extremely Confident)",
            75..=89 => "LARGE (Very Confident)",
            60..=74 => "NORMAL (Confident)",
            45..=59 => "REDUCED (Moderate Confidence)",
            30..=44 => "SMALL (Low Confidence)",
            _ => "MINIMUM (Very Low Confidence)",
        }
    }

    /// Calculate expected value for informational purposes
    /// win_rate: 0-1, avg_win: positive %, avg_loss: negative %
    pub fn calculate_expected_value(
        win_rate: f64,
        avg_win_percent: f64,
        avg_loss_percent: f64,
    ) -> f64 {
        (win_rate * avg_win_percent) + ((1.0 - win_rate) * avg_loss_percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_sizing() {
        // High confidence should scale up
        let size_high = PositionSizer::calculate_position_size(0.5, 90, 100.0, 0.05);
        assert!(size_high > 0.5);
        assert!(size_high <= 5.0); // Max 5% of 100 = 5 SOL

        // Low confidence should scale down
        let size_low = PositionSizer::calculate_position_size(0.5, 40, 100.0, 0.05);
        assert!(size_low < 0.5);

        // Should respect max position size
        let size_capped = PositionSizer::calculate_position_size(10.0, 90, 100.0, 0.05);
        assert_eq!(size_capped, 5.0); // 5% of 100
    }

    #[test]
    fn test_confidence_calculation() {
        use crate::services::token_safety::TokenSafetyCheck;
        use crate::strategies::market_sentiment::{MarketSentiment, Trend};
        use crate::strategies::volume_analyzer::{VolumeProfile, VolumeTrend};

        let safety = TokenSafetyCheck {
            passed: true,
            reasons: vec![],
            risk_score: 10, // Low risk
        };

        let volume = VolumeProfile {
            volume_1h: 10000.0,
            volume_6h: 40000.0,
            volume_24h: 100000.0,
            volume_trend: VolumeTrend::Accelerating,
            buy_sell_ratio: 1.5,
            quality_score: 85,
        };

        let sentiment = MarketSentiment {
            sol_trend: Trend::Bullish,
            sol_price_change_1h: 3.0,
            sol_price_change_24h: 8.0,
            market_fear_greed: 70,
            trading_recommended: true,
        };

        let confidence = PositionSizer::calculate_confidence(
            &safety,
            &volume,
            &sentiment,
            30.0,  // Good liquidity
            250,   // Good holder count
        );

        // Should be high confidence with all good signals
        assert!(confidence >= 70, "Confidence was {}", confidence);
    }

    #[test]
    fn test_expected_value() {
        // Positive EV: 70% win rate, +100% avg win, -20% avg loss
        let ev_positive = PositionSizer::calculate_expected_value(0.7, 100.0, -20.0);
        assert!(ev_positive > 0.0);
        assert_eq!(ev_positive, 64.0); // (0.7 * 100) + (0.3 * -20) = 70 - 6 = 64

        // Negative EV: 40% win rate, +50% avg win, -80% avg loss
        let ev_negative = PositionSizer::calculate_expected_value(0.4, 50.0, -80.0);
        assert!(ev_negative < 0.0);
    }

    #[test]
    fn test_size_categories() {
        assert_eq!(PositionSizer::get_size_category(95), "MAXIMUM (Extremely Confident)");
        assert_eq!(PositionSizer::get_size_category(80), "LARGE (Very Confident)");
        assert_eq!(PositionSizer::get_size_category(65), "NORMAL (Confident)");
        assert_eq!(PositionSizer::get_size_category(50), "REDUCED (Moderate Confidence)");
        assert_eq!(PositionSizer::get_size_category(35), "SMALL (Low Confidence)");
        assert_eq!(PositionSizer::get_size_category(20), "MINIMUM (Very Low Confidence)");
    }
}
