/// Machine Learning Training Pipeline
///
/// Handles:
/// - Data collection from completed trades
/// - Model training with gradient descent
/// - Model evaluation and validation
/// - Continuous learning (incremental updates)
/// - Model persistence

use super::features::{FeatureExtractor, FeatureVector, FeatureStats};
use super::predictor::{LogisticRegression, ModelMetrics};
use anyhow::Result;
use std::collections::VecDeque;

/// Training sample with features and outcome
#[derive(Debug, Clone)]
pub struct TrainingSample {
    pub features: FeatureVector,
    pub outcome: bool, // true = win, false = loss
    pub timestamp: i64,
    pub pnl_percent: f64,
}

/// Manages the ML training pipeline
pub struct MLTrainer {
    /// Current model
    pub model: LogisticRegression,
    /// Training samples buffer
    training_buffer: VecDeque<TrainingSample>,
    /// Feature statistics for normalization
    pub feature_stats: FeatureStats,
    /// Training configuration
    config: TrainingConfig,
    /// Training history
    training_history: Vec<TrainingEpoch>,
}

#[derive(Debug, Clone)]
pub struct TrainingConfig {
    /// Minimum samples before training
    pub min_samples: usize,
    /// Retrain every N new samples
    pub retrain_interval: usize,
    /// Maximum samples to keep in buffer
    pub max_buffer_size: usize,
    /// Learning rate for gradient descent
    pub learning_rate: f64,
    /// Number of training epochs
    pub epochs: usize,
    /// Train/test split ratio
    pub test_split: f64,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            min_samples: 50,        // Need at least 50 trades
            retrain_interval: 100,  // Retrain every 100 trades
            max_buffer_size: 1000,  // Keep last 1000 trades
            learning_rate: 0.01,    // Standard learning rate
            epochs: 100,            // 100 training iterations
            test_split: 0.2,        // 20% for testing
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrainingEpoch {
    pub epoch: usize,
    pub loss: f64,
    pub metrics: ModelMetrics,
    pub timestamp: i64,
}

impl MLTrainer {
    pub fn new(config: TrainingConfig) -> Self {
        Self {
            model: LogisticRegression::new(),
            training_buffer: VecDeque::with_capacity(config.max_buffer_size),
            feature_stats: FeatureStats::new(),
            config,
            training_history: Vec::new(),
        }
    }

    /// Add a new training sample
    pub fn add_sample(&mut self, sample: TrainingSample) {
        // Update feature statistics
        self.feature_stats.update(&sample.features);

        // Add to buffer
        if self.training_buffer.len() >= self.config.max_buffer_size {
            self.training_buffer.pop_front();
        }
        self.training_buffer.push_back(sample);

        // Check if we should retrain
        if self.should_retrain() {
            if let Err(e) = self.train() {
                eprintln!("Training failed: {}", e);
            }
        }
    }

    /// Check if we should retrain the model
    fn should_retrain(&self) -> bool {
        let sample_count = self.training_buffer.len();

        // Not enough samples yet
        if sample_count < self.config.min_samples {
            return false;
        }

        // Retrain at intervals
        sample_count % self.config.retrain_interval == 0
    }

    /// Train the model on collected samples
    pub fn train(&mut self) -> Result<ModelMetrics> {
        if self.training_buffer.len() < self.config.min_samples {
            return Err(anyhow::anyhow!(
                "Not enough samples: {} < {}",
                self.training_buffer.len(),
                self.config.min_samples
            ));
        }

        // Normalize features
        let mut normalized_samples: Vec<_> = self
            .training_buffer
            .iter()
            .map(|sample| {
                let mut normalized_features = sample.features.clone();
                normalized_features.normalize(
                    &self.feature_stats.min_values,
                    &self.feature_stats.max_values,
                );
                (normalized_features, sample.outcome)
            })
            .collect();

        // Split into train/test
        let split_idx = (normalized_samples.len() as f64 * (1.0 - self.config.test_split)) as usize;
        let (train_samples, test_samples) = normalized_samples.split_at(split_idx);

        // Train for multiple epochs
        for epoch in 0..self.config.epochs {
            // Shuffle training samples (simple shuffle)
            let mut shuffled = train_samples.to_vec();
            Self::simple_shuffle(&mut shuffled, epoch);

            // Batch update
            self.model.batch_update(&shuffled, self.config.learning_rate);

            // Evaluate periodically
            if epoch % 10 == 0 {
                let loss = self.model.calculate_loss(&shuffled);
                let metrics = self.model.evaluate(test_samples);

                self.training_history.push(TrainingEpoch {
                    epoch,
                    loss,
                    metrics: metrics.clone(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                });
            }
        }

        // Final evaluation
        let metrics = self.model.evaluate(test_samples);
        self.model.accuracy = metrics.accuracy;

        Ok(metrics)
    }

    /// Simple deterministic shuffle (no external RNG needed)
    fn simple_shuffle<T: Clone>(items: &mut Vec<T>, seed: usize) {
        let len = items.len();
        for i in 0..len {
            let j = ((i + seed) * 7919) % len; // Prime number for better distribution
            if i != j {
                items.swap(i, j);
            }
        }
    }

    /// Get the current model
    pub fn get_model(&self) -> &LogisticRegression {
        &self.model
    }

    /// Get number of training samples
    pub fn sample_count(&self) -> usize {
        self.training_buffer.len()
    }

    /// Check if model is ready for use
    pub fn is_model_ready(&self) -> bool {
        self.training_buffer.len() >= self.config.min_samples && self.model.accuracy > 0.0
    }

    /// Get training progress
    pub fn get_progress(&self) -> TrainingProgress {
        TrainingProgress {
            samples_collected: self.training_buffer.len(),
            samples_needed: self.config.min_samples,
            model_ready: self.is_model_ready(),
            current_accuracy: self.model.accuracy,
            total_trained: self.model.trained_samples,
        }
    }

    /// Get latest training metrics
    pub fn get_latest_metrics(&self) -> Option<&TrainingEpoch> {
        self.training_history.last()
    }

    /// Clear training history (keep model)
    pub fn clear_history(&mut self) {
        self.training_history.clear();
    }

    /// Reset everything
    pub fn reset(&mut self) {
        self.model = LogisticRegression::new();
        self.training_buffer.clear();
        self.feature_stats = FeatureStats::new();
        self.training_history.clear();
    }

    /// Export model and stats for persistence
    pub fn export(&self) -> String {
        format!(
            "MODEL:{}|STATS:{}|SAMPLES:{}",
            self.model.to_string(),
            self.feature_stats.to_string(),
            self.training_buffer.len()
        )
    }

    /// Import model and stats from persistence
    pub fn import(&mut self, data: &str) -> Result<()> {
        let parts: Vec<&str> = data.split('|').collect();

        for part in parts {
            if let Some(model_data) = part.strip_prefix("MODEL:") {
                self.model = LogisticRegression::from_string(model_data)?;
            } else if let Some(stats_data) = part.strip_prefix("STATS:") {
                self.feature_stats = FeatureStats::from_string(stats_data)
                    .ok_or_else(|| anyhow::anyhow!("Invalid stats format"))?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TrainingProgress {
    pub samples_collected: usize,
    pub samples_needed: usize,
    pub model_ready: bool,
    pub current_accuracy: f64,
    pub total_trained: usize,
}

impl TrainingProgress {
    pub fn progress_percent(&self) -> f64 {
        if self.samples_needed == 0 {
            return 100.0;
        }
        (self.samples_collected as f64 / self.samples_needed as f64 * 100.0).min(100.0)
    }

    pub fn to_string(&self) -> String {
        format!(
            "Progress: {:.1}% ({}/{} samples) | Accuracy: {:.2}% | Model Ready: {}",
            self.progress_percent(),
            self.samples_collected,
            self.samples_needed,
            self.current_accuracy * 100.0,
            if self.model_ready { "Yes" } else { "No" }
        )
    }
}

/// Helper to create training samples from trade data
pub struct TrainingDataCollector;

impl TrainingDataCollector {
    /// Create a training sample from trade outcome
    pub fn create_sample(
        features: FeatureVector,
        pnl_percent: f64,
        timestamp: i64,
    ) -> TrainingSample {
        // Define win as positive PnL
        let outcome = pnl_percent > 0.0;

        TrainingSample {
            features,
            outcome,
            timestamp,
            pnl_percent,
        }
    }

    /// Determine if outcome was a win (can be customized)
    pub fn is_win(pnl_percent: f64, min_win_threshold: f64) -> bool {
        pnl_percent >= min_win_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trainer_creation() {
        let config = TrainingConfig::default();
        let trainer = MLTrainer::new(config);

        assert_eq!(trainer.sample_count(), 0);
        assert!(!trainer.is_model_ready());
    }

    #[test]
    fn test_add_sample() {
        let config = TrainingConfig {
            min_samples: 10,
            max_buffer_size: 100,
            ..Default::default()
        };
        let mut trainer = MLTrainer::new(config);

        let sample = TrainingSample {
            features: FeatureVector::new([0.5; 15]),
            outcome: true,
            timestamp: 1000,
            pnl_percent: 5.0,
        };

        trainer.add_sample(sample);

        assert_eq!(trainer.sample_count(), 1);
    }

    #[test]
    fn test_should_retrain() {
        let config = TrainingConfig {
            min_samples: 10,
            retrain_interval: 20,
            ..Default::default()
        };
        let mut trainer = MLTrainer::new(config);

        // Add samples
        for i in 0..50 {
            let sample = TrainingSample {
                features: FeatureVector::new([0.5; 15]),
                outcome: i % 2 == 0,
                timestamp: i,
                pnl_percent: if i % 2 == 0 { 5.0 } else { -3.0 },
            };
            trainer.add_sample(sample);
        }

        assert!(trainer.sample_count() >= config.min_samples);
    }

    #[test]
    fn test_training_progress() {
        let config = TrainingConfig {
            min_samples: 100,
            ..Default::default()
        };
        let mut trainer = MLTrainer::new(config);

        // Add 50 samples
        for i in 0..50 {
            let sample = TrainingSample {
                features: FeatureVector::new([0.5; 15]),
                outcome: true,
                timestamp: i,
                pnl_percent: 5.0,
            };
            trainer.add_sample(sample);
        }

        let progress = trainer.get_progress();
        assert_eq!(progress.samples_collected, 50);
        assert_eq!(progress.samples_needed, 100);
        assert_eq!(progress.progress_percent(), 50.0);
        assert!(!progress.model_ready);
    }

    #[test]
    fn test_export_import() {
        let config = TrainingConfig::default();
        let mut trainer = MLTrainer::new(config);

        // Add some samples
        for i in 0..10 {
            let sample = TrainingSample {
                features: FeatureVector::new([0.5; 15]),
                outcome: i % 2 == 0,
                timestamp: i,
                pnl_percent: if i % 2 == 0 { 5.0 } else { -3.0 },
            };
            trainer.add_sample(sample);
        }

        // Export
        let exported = trainer.export();

        // Import to new trainer
        let mut new_trainer = MLTrainer::new(TrainingConfig::default());
        new_trainer.import(&exported).unwrap();

        // Model should be the same
        assert_eq!(
            new_trainer.model.trained_samples,
            trainer.model.trained_samples
        );
    }

    #[test]
    fn test_data_collector() {
        let features = FeatureVector::new([0.5; 15]);
        let sample = TrainingDataCollector::create_sample(features, 10.0, 1000);

        assert!(sample.outcome); // Positive PnL = win
        assert_eq!(sample.pnl_percent, 10.0);

        assert!(TrainingDataCollector::is_win(5.0, 2.0));
        assert!(!TrainingDataCollector::is_win(1.0, 2.0));
    }
}
