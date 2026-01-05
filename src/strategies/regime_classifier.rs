/// Regime Classification System (CRITICAL FIX #4)
///
/// Classifies tokens into trading regimes and applies regime-specific playbooks:
/// - NewPair: Stop -6%, TP +8/15/runner, Time: 12min
/// - Momentum: Stop -4%, Breakout hold 20-40s, TP +6/10/runner
/// - MeanReversion: Stop -5%, Bounce confirm ≥1min, Time: 20min
/// - Majors: Stop -2%, Trailing only
///
/// Priority: NewPair > Momentum > MeanReversion > Majors

use crate::models::TokenPool;
use crate::strategies::{VolumeProfile, VolumeTrend, TimeframeAnalyzer};
use chrono::Utc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Regime {
    NewPair,       // Newly launched tokens (sniper mode)
    Momentum,      // Strong directional moves
    MeanReversion, // Oversold/overbought bounces
    Majors,        // Established low-volatility tokens
}

impl Regime {
    pub fn name(&self) -> &str {
        match self {
            Regime::NewPair => "NewPair",
            Regime::Momentum => "Momentum",
            Regime::MeanReversion => "MeanReversion",
            Regime::Majors => "Majors",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegimePlaybook {
    pub regime: Regime,
    pub stop_loss_pct: f64,
    pub take_profit_pcts: Vec<f64>,  // Multiple TPs (partial exits)
    pub trailing_stop_pct: f64,
    pub time_stop_minutes: Option<u64>,
    pub min_score: u32,  // Minimum quant score required
    pub breakout_confirmation_sec: Option<u64>,  // For momentum
    pub bounce_confirmation_sec: Option<u64>,    // For mean reversion
}

impl RegimePlaybook {
    pub fn for_regime(regime: Regime) -> Self {
        match regime {
            Regime::NewPair => RegimePlaybook {
                regime,
                stop_loss_pct: 6.0,         // -6%
                take_profit_pcts: vec![8.0, 15.0],  // +8%, +15%, runner
                trailing_stop_pct: 5.0,      // 5% trail
                time_stop_minutes: Some(12), // 12 minute max hold
                min_score: 78,               // High bar for new pairs
                breakout_confirmation_sec: None,
                bounce_confirmation_sec: None,
            },
            Regime::Momentum => RegimePlaybook {
                regime,
                stop_loss_pct: 4.0,         // -4%
                take_profit_pcts: vec![6.0, 10.0],  // +6%, +10%, runner
                trailing_stop_pct: 4.0,
                time_stop_minutes: None,     // No time stop
                min_score: 70,
                breakout_confirmation_sec: Some(30),  // 20-40s average = 30s
                bounce_confirmation_sec: None,
            },
            Regime::MeanReversion => RegimePlaybook {
                regime,
                stop_loss_pct: 5.0,         // -5%
                take_profit_pcts: vec![5.0, 10.0],
                trailing_stop_pct: 4.0,
                time_stop_minutes: Some(20), // 20 minute max hold
                min_score: 72,
                breakout_confirmation_sec: None,
                bounce_confirmation_sec: Some(60),  // ≥1 minute
            },
            Regime::Majors => RegimePlaybook {
                regime,
                stop_loss_pct: 2.0,         // -2%
                take_profit_pcts: vec![3.0, 5.0],
                trailing_stop_pct: 2.0,
                time_stop_minutes: None,     // Hold longer
                min_score: 65,
                breakout_confirmation_sec: None,
                bounce_confirmation_sec: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegimeClassification {
    pub regime: Regime,
    pub playbook: RegimePlaybook,
    pub confidence: f64,  // 0.0 - 1.0
    pub reasons: Vec<String>,
}

pub struct RegimeClassifier;

impl RegimeClassifier {
    /// Classify a token into its trading regime
    /// Priority: NewPair > Momentum > MeanReversion > Majors
    pub fn classify(
        pool: &TokenPool,
        volume_profile: &VolumeProfile,
        timeframe_analysis: Option<&TimeframeAnalyzer>,
    ) -> RegimeClassification {
        let mut reasons = Vec::new();

        // Check 1: NewPair (highest priority)
        if Self::is_new_pair(pool) {
            reasons.push(format!("Token age: {} hours (new pair)", pool.age_hours()));
            reasons.push("Sniper mode: high volatility expected".to_string());

            return RegimeClassification {
                regime: Regime::NewPair,
                playbook: RegimePlaybook::for_regime(Regime::NewPair),
                confidence: 0.95,
                reasons,
            };
        }

        // Check 2: Momentum (second priority)
        if let Some(momentum_conf) = Self::check_momentum(pool, volume_profile, timeframe_analysis) {
            return momentum_conf;
        }

        // Check 3: Mean Reversion (third priority)
        if let Some(mean_rev_conf) = Self::check_mean_reversion(pool, volume_profile, timeframe_analysis) {
            return mean_rev_conf;
        }

        // Check 4: Majors (fallback)
        let major_conf = Self::check_majors(pool, volume_profile);
        if let Some(conf) = major_conf {
            return conf;
        }

        // Default to NewPair if nothing else matches (conservative)
        RegimeClassification {
            regime: Regime::NewPair,
            playbook: RegimePlaybook::for_regime(Regime::NewPair),
            confidence: 0.5,
            reasons: vec!["No clear regime - defaulting to NewPair (conservative)".to_string()],
        }
    }

    fn is_new_pair(pool: &TokenPool) -> bool {
        // Tokens less than 60 minutes old
        pool.age_hours() < 1.0
    }

    fn check_momentum(
        pool: &TokenPool,
        volume_profile: &VolumeProfile,
        timeframe_analysis: Option<&TimeframeAnalyzer>,
    ) -> Option<RegimeClassification> {
        let mut reasons = Vec::new();
        let mut score = 0;

        // Strong volume acceleration
        if matches!(volume_profile.volume_trend, VolumeTrend::Accelerating) {
            score += 30;
            reasons.push(format!("Volume trend: {:?}", volume_profile.volume_trend));
        }

        // High buy pressure
        if volume_profile.buy_sell_ratio > 1.5 {
            score += 25;
            reasons.push(format!("Buy/sell ratio: {:.2}", volume_profile.buy_sell_ratio));
        }

        // Strong uptrend in multiple timeframes
        if let Some(tf_analysis) = timeframe_analysis {
            let bullish_timeframes = tf_analysis.count_bullish_timeframes();
            if bullish_timeframes >= 2 {
                score += 25;
                reasons.push(format!("{} timeframes bullish", bullish_timeframes));
            }
        }

        // Liquidity growing (momentum confirmation)
        if pool.liquidity_sol > 50.0 {
            score += 20;
            reasons.push(format!("Strong liquidity: {:.1} SOL", pool.liquidity_sol));
        }

        if score >= 60 {
            Some(RegimeClassification {
                regime: Regime::Momentum,
                playbook: RegimePlaybook::for_regime(Regime::Momentum),
                confidence: (score as f64) / 100.0,
                reasons,
            })
        } else {
            None
        }
    }

    fn check_mean_reversion(
        pool: &TokenPool,
        volume_profile: &VolumeProfile,
        timeframe_analysis: Option<&TimeframeAnalyzer>,
    ) -> Option<RegimeClassification> {
        let mut reasons = Vec::new();
        let mut score = 0;

        // Declining volume (exhaustion)
        if matches!(volume_profile.volume_trend, VolumeTrend::Declining) {
            score += 30;
            reasons.push("Volume declining (potential reversal)".to_string());
        }

        // Extreme buy/sell imbalance (reversal setup)
        let extreme_sell = volume_profile.buy_sell_ratio < 0.5;
        let extreme_buy = volume_profile.buy_sell_ratio > 3.0;

        if extreme_sell || extreme_buy {
            score += 35;
            if extreme_sell {
                reasons.push("Extreme selling pressure (oversold bounce setup)".to_string());
            } else {
                reasons.push("Extreme buying pressure (overbought pullback setup)".to_string());
            }
        }

        // Divergence in timeframes (reversal signal)
        if let Some(tf_analysis) = timeframe_analysis {
            if tf_analysis.has_divergence() {
                score += 35;
                reasons.push("Timeframe divergence detected".to_string());
            }
        }

        if score >= 65 {
            Some(RegimeClassification {
                regime: Regime::MeanReversion,
                playbook: RegimePlaybook::for_regime(Regime::MeanReversion),
                confidence: (score as f64) / 100.0,
                reasons,
            })
        } else {
            None
        }
    }

    fn check_majors(
        pool: &TokenPool,
        volume_profile: &VolumeProfile,
    ) -> Option<RegimeClassification> {
        let mut reasons = Vec::new();
        let mut score = 0;

        // High liquidity (established token)
        if pool.liquidity_sol > 200.0 {
            score += 40;
            reasons.push(format!("High liquidity: {:.1} SOL (established)", pool.liquidity_sol));
        }

        // Stable volume (low volatility)
        if matches!(volume_profile.volume_trend, VolumeTrend::Steady) {
            score += 30;
            reasons.push("Stable volume (low volatility)".to_string());
        }

        // Balanced buy/sell
        if volume_profile.buy_sell_ratio > 0.8 && volume_profile.buy_sell_ratio < 1.2 {
            score += 30;
            reasons.push("Balanced buy/sell pressure".to_string());
        }

        if score >= 70 {
            Some(RegimeClassification {
                regime: Regime::Majors,
                playbook: RegimePlaybook::for_regime(Regime::Majors),
                confidence: (score as f64) / 100.0,
                reasons,
            })
        } else {
            None
        }
    }
}

// Helper trait for TokenPool
trait TokenAge {
    fn age_hours(&self) -> f64;
}

impl TokenAge for TokenPool {
    fn age_hours(&self) -> f64 {
        if let Some(created_at) = self.created_at {
            let created_ms = if created_at > 1_000_000_000_000 {
                created_at
            } else {
                created_at * 1000
            };

            let now_ms = Utc::now().timestamp_millis();
            let age_ms = now_ms.saturating_sub(created_ms);
            (age_ms as f64) / (1000.0 * 60.0 * 60.0)
        } else {
            // Unknown age -> treat as older to avoid misclassifying as NewPair
            24.0
        }
    }
}

// Helper trait for TimeframeAnalyzer
trait TimeframeDivergence {
    fn count_bullish_timeframes(&self) -> u32;
    fn has_divergence(&self) -> bool;
}

impl TimeframeDivergence for TimeframeAnalyzer {
    fn count_bullish_timeframes(&self) -> u32 {
        let now = Utc::now().timestamp_millis();
        if let Some(analysis) = self.analyze(now) {
            let mut bullish = 0;
            if analysis.five_minute.trend.is_bullish() {
                bullish += 1;
            }
            if analysis.fifteen_minute.trend.is_bullish() {
                bullish += 1;
            }
            if analysis.one_hour.trend.is_bullish() {
                bullish += 1;
            }
            bullish
        } else {
            0
        }
    }

    fn has_divergence(&self) -> bool {
        let now = Utc::now().timestamp_millis();
        if let Some(analysis) = self.analyze(now) {
            let short_bull = analysis.five_minute.trend.is_bullish();
            let short_bear = analysis.five_minute.trend.is_bearish();
            let mid_bull = analysis.fifteen_minute.trend.is_bullish();
            let mid_bear = analysis.fifteen_minute.trend.is_bearish();
            let long_bull = analysis.one_hour.trend.is_bullish();
            let long_bear = analysis.one_hour.trend.is_bearish();

            (short_bull && long_bear)
                || (short_bear && long_bull)
                || (short_bull && mid_bear)
                || (short_bear && mid_bull)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_pair_classification() {
        // Would test classification logic
    }

    #[test]
    fn test_playbook_parameters() {
        let new_pair_playbook = RegimePlaybook::for_regime(Regime::NewPair);
        assert_eq!(new_pair_playbook.stop_loss_pct, 6.0);
        assert_eq!(new_pair_playbook.min_score, 78);

        let momentum_playbook = RegimePlaybook::for_regime(Regime::Momentum);
        assert_eq!(momentum_playbook.stop_loss_pct, 4.0);
        assert_eq!(momentum_playbook.min_score, 70);
    }
}
