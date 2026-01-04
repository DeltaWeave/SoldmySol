/// Machine Learning Predictor
///
/// Implements logistic regression for binary classification (win/loss prediction)
/// Uses sigmoid activation function to output probability (0-1)
///
/// Model: P(win) = sigmoid(w0 + w1*x1 + w2*x2 + ... + w15*x15)
/// where xi are the 15 features and wi are the learned weights

use super::features::FeatureVector;
use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct LogisticRegression {
    /// Weights for each of the 15 features plus bias term (16 total)
    pub weights: Vec<f64>,
    /// Model metadata
    pub trained_samples: usize,
    pub accuracy: f64,
    pub version: u32,
}

impl LogisticRegression {
    /// Create a new model with random initialization
    pub fn new() -> Self {
        // Initialize weights to small random values
        let weights: Vec<f64> = (0..16)
            .map(|i| {
                // Simple pseudo-random initialization based on index
                let seed = (i * 7919) as f64;
                (seed.sin() * 0.01).clamp(-0.1, 0.1)
            })
            .collect();

        Self {
            weights,
            trained_samples: 0,
            accuracy: 0.0,
            version: 1,
        }
    }

    /// Predict probability of success (0-1)
    pub fn predict(&self, features: &FeatureVector) -> f64 {
        // Calculate weighted sum: w0 + w1*x1 + w2*x2 + ... + w15*x15
        let mut z = self.weights[0]; // bias term

        for i in 0..15 {
            z += self.weights[i + 1] * features.features[i];
        }

        // Apply sigmoid activation
        Self::sigmoid(z)
    }

    /// Predict with confidence categorization
    pub fn predict_with_confidence(&self, features: &FeatureVector) -> PredictionResult {
        let probability = self.predict(features);

        let confidence_level = if probability >= 0.8 {
            ConfidenceLevel::VeryHigh
        } else if probability >= 0.7 {
            ConfidenceLevel::High
        } else if probability >= 0.6 {
            ConfidenceLevel::Medium
        } else if probability >= 0.5 {
            ConfidenceLevel::Low
        } else {
            ConfidenceLevel::VeryLow
        };

        let recommendation = if probability >= 0.7 {
            Recommendation::StrongBuy
        } else if probability >= 0.6 {
            Recommendation::Buy
        } else if probability >= 0.4 {
            Recommendation::Hold
        } else {
            Recommendation::Avoid
        };

        PredictionResult {
            probability,
            confidence_level,
            recommendation,
            model_version: self.version,
        }
    }

    /// Sigmoid activation function: σ(z) = 1 / (1 + e^(-z))
    fn sigmoid(z: f64) -> f64 {
        1.0 / (1.0 + (-z).exp())
    }

    /// Derivative of sigmoid: σ'(z) = σ(z) * (1 - σ(z))
    fn sigmoid_derivative(sigmoid_output: f64) -> f64 {
        sigmoid_output * (1.0 - sigmoid_output)
    }

    /// Update weights using gradient descent (single sample)
    pub fn update_weights(&mut self, features: &FeatureVector, actual_outcome: bool, learning_rate: f64) {
        let prediction = self.predict(features);
        let target = if actual_outcome { 1.0 } else { 0.0 };

        // Calculate error
        let error = prediction - target;

        // Update bias
        self.weights[0] -= learning_rate * error;

        // Update feature weights
        for i in 0..15 {
            let gradient = error * features.features[i];
            self.weights[i + 1] -= learning_rate * gradient;
        }

        self.trained_samples += 1;
    }

    /// Batch update with multiple samples
    pub fn batch_update(&mut self, samples: &[(FeatureVector, bool)], learning_rate: f64) {
        for (features, outcome) in samples {
            self.update_weights(features, *outcome, learning_rate);
        }
    }

    /// Calculate loss (cross-entropy) for a set of samples
    pub fn calculate_loss(&self, samples: &[(FeatureVector, bool)]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }

        let mut total_loss = 0.0;

        for (features, outcome) in samples {
            let prediction = self.predict(features);
            let target = if *outcome { 1.0 } else { 0.0 };

            // Cross-entropy loss: -[y*log(p) + (1-y)*log(1-p)]
            let epsilon = 1e-10; // Prevent log(0)
            let p = prediction.clamp(epsilon, 1.0 - epsilon);

            total_loss += -(target * p.ln() + (1.0 - target) * (1.0 - p).ln());
        }

        total_loss / samples.len() as f64
    }

    /// Evaluate model performance
    pub fn evaluate(&self, test_samples: &[(FeatureVector, bool)]) -> ModelMetrics {
        if test_samples.is_empty() {
            return ModelMetrics::default();
        }

        let mut true_positives = 0;
        let mut true_negatives = 0;
        let mut false_positives = 0;
        let mut false_negatives = 0;

        for (features, actual_outcome) in test_samples {
            let prediction = self.predict(features);
            let predicted_outcome = prediction >= 0.5;

            match (predicted_outcome, *actual_outcome) {
                (true, true) => true_positives += 1,
                (false, false) => true_negatives += 1,
                (true, false) => false_positives += 1,
                (false, true) => false_negatives += 1,
            }
        }

        let accuracy = (true_positives + true_negatives) as f64 / test_samples.len() as f64;

        let precision = if true_positives + false_positives > 0 {
            true_positives as f64 / (true_positives + false_positives) as f64
        } else {
            0.0
        };

        let recall = if true_positives + false_negatives > 0 {
            true_positives as f64 / (true_positives + false_negatives) as f64
        } else {
            0.0
        };

        let f1_score = if precision + recall > 0.0 {
            2.0 * (precision * recall) / (precision + recall)
        } else {
            0.0
        };

        ModelMetrics {
            accuracy,
            precision,
            recall,
            f1_score,
            true_positives,
            true_negatives,
            false_positives,
            false_negatives,
        }
    }

    /// Serialize model to string (for persistence)
    pub fn to_string(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.weights.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(","),
            self.trained_samples,
            self.accuracy,
            self.version
        )
    }

    /// Deserialize model from string
    pub fn from_string(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() != 4 {
            return Err(anyhow!("Invalid model format"));
        }

        let weights: Vec<f64> = parts[0]
            .split(',')
            .filter_map(|w| w.parse().ok())
            .collect();

        if weights.len() != 16 {
            return Err(anyhow!("Invalid weights count: expected 16, got {}", weights.len()));
        }

        let trained_samples = parts[1].parse()?;
        let accuracy = parts[2].parse()?;
        let version = parts[3].parse()?;

        Ok(Self {
            weights,
            trained_samples,
            accuracy,
            version,
        })
    }

    /// Get feature importance (magnitude of weights)
    pub fn get_feature_importance(&self) -> Vec<(usize, f64)> {
        let mut importance: Vec<(usize, f64)> = (0..15)
            .map(|i| (i, self.weights[i + 1].abs()))
            .collect();

        importance.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        importance
    }
}

impl Default for LogisticRegression {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct PredictionResult {
    pub probability: f64,
    pub confidence_level: ConfidenceLevel,
    pub recommendation: Recommendation,
    pub model_version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfidenceLevel {
    VeryHigh,  // >= 0.8
    High,      // >= 0.7
    Medium,    // >= 0.6
    Low,       // >= 0.5
    VeryLow,   // < 0.5
}

#[derive(Debug, Clone, PartialEq)]
pub enum Recommendation {
    StrongBuy,  // >= 0.7 probability
    Buy,        // >= 0.6 probability
    Hold,       // >= 0.4 probability
    Avoid,      // < 0.4 probability
}

#[derive(Debug, Clone, Default)]
pub struct ModelMetrics {
    pub accuracy: f64,
    pub precision: f64,
    pub recall: f64,
    pub f1_score: f64,
    pub true_positives: usize,
    pub true_negatives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
}

impl ModelMetrics {
    pub fn to_string(&self) -> String {
        format!(
            "Accuracy: {:.2}%, Precision: {:.2}%, Recall: {:.2}%, F1: {:.2}%",
            self.accuracy * 100.0,
            self.precision * 100.0,
            self.recall * 100.0,
            self.f1_score * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ml::features::FeatureVector;

    #[test]
    fn test_sigmoid() {
        assert!((LogisticRegression::sigmoid(0.0) - 0.5).abs() < 0.01);
        assert!(LogisticRegression::sigmoid(10.0) > 0.99);
        assert!(LogisticRegression::sigmoid(-10.0) < 0.01);
    }

    #[test]
    fn test_prediction() {
        let model = LogisticRegression::new();
        let features = FeatureVector::new([0.5; 15]);

        let prediction = model.predict(&features);
        assert!(prediction >= 0.0 && prediction <= 1.0);
    }

    #[test]
    fn test_weight_update() {
        let mut model = LogisticRegression::new();
        let features = FeatureVector::new([0.5; 15]);

        let initial_weights = model.weights.clone();

        // Train with a positive outcome
        model.update_weights(&features, true, 0.01);

        // Weights should have changed
        assert_ne!(model.weights, initial_weights);
        assert_eq!(model.trained_samples, 1);
    }

    #[test]
    fn test_model_serialization() {
        let model = LogisticRegression {
            weights: vec![0.1; 16],
            trained_samples: 100,
            accuracy: 0.85,
            version: 1,
        };

        let serialized = model.to_string();
        let deserialized = LogisticRegression::from_string(&serialized).unwrap();

        assert_eq!(deserialized.trained_samples, 100);
        assert_eq!(deserialized.version, 1);
        assert!((deserialized.accuracy - 0.85).abs() < 0.01);
    }

    #[test]
    fn test_evaluate() {
        let model = LogisticRegression::new();

        let test_samples = vec![
            (FeatureVector::new([0.8; 15]), true),
            (FeatureVector::new([0.2; 15]), false),
            (FeatureVector::new([0.7; 15]), true),
            (FeatureVector::new([0.3; 15]), false),
        ];

        let metrics = model.evaluate(&test_samples);

        assert!(metrics.accuracy >= 0.0 && metrics.accuracy <= 1.0);
        assert!(metrics.precision >= 0.0 && metrics.precision <= 1.0);
        assert!(metrics.recall >= 0.0 && metrics.recall <= 1.0);
    }

    #[test]
    fn test_batch_update() {
        let mut model = LogisticRegression::new();

        let samples = vec![
            (FeatureVector::new([0.8; 15]), true),
            (FeatureVector::new([0.2; 15]), false),
        ];

        let initial_samples = model.trained_samples;
        model.batch_update(&samples, 0.01);

        assert_eq!(model.trained_samples, initial_samples + 2);
    }

    #[test]
    fn test_feature_importance() {
        let mut model = LogisticRegression::new();
        model.weights = vec![0.5, 0.1, 0.9, 0.3, 0.2, 0.8, 0.4, 0.6, 0.7, 0.5, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6];

        let importance = model.get_feature_importance();

        // Should be sorted by magnitude
        assert_eq!(importance.len(), 15);
        assert!(importance[0].1 >= importance[1].1);
    }
}
