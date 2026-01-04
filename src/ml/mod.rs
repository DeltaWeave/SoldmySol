/// Machine Learning Module
///
/// Provides ML capabilities for the trading bot:
/// - Feature extraction from token data
/// - Logistic regression for win/loss prediction
/// - Continuous learning from trade outcomes
/// - Model persistence and evaluation

pub mod features;
pub mod predictor;
pub mod trainer;

pub use features::{FeatureExtractor, FeatureVector, FeatureStats};
pub use predictor::{LogisticRegression, PredictionResult, ConfidenceLevel, Recommendation, ModelMetrics};
pub use trainer::{MLTrainer, TrainingSample, TrainingConfig, TrainingProgress, TrainingDataCollector};
