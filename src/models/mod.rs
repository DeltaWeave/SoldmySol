use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// ⭐ P1.3: Position lifecycle states for Sniper→Momentum handoff
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionState {
    /// Initial sniper mode: Quick flip for 2x-5x (2-10 minutes)
    Sniper,

    /// Promoted to momentum mode: Hold for 5x-20x (higher targets)
    Momentum,

    /// Failed validation window: Exit ASAP
    Failed,
}

impl Default for PositionState {
    fn default() -> Self {
        PositionState::Sniper
    }
}

#[derive(Debug, Clone)]
pub struct Position {
    pub trade_id: i64,
    pub token_address: Pubkey,
    pub token_symbol: String,
    pub entry_price: f64,
    pub amount_sol: f64,
    pub tokens_bought: f64,
    pub entry_time: i64,
    pub highest_price: f64,
    // ⭐ P1.3: Position lifecycle state
    pub state: PositionState,
    pub validation_window_end: i64,  // Timestamp when validation window ends (2-10 min)
    pub higher_lows_count: u8,        // Track consecutive higher lows for momentum
    // ✅ PRIORITY 1.5: Regime-specific playbook parameters
    pub stop_loss_pct: f64,          // From regime playbook (e.g., -6% for NewPair)
    pub take_profit_pcts: Vec<f64>,  // Multi-level TPs from playbook
    pub time_stop_minutes: Option<u64>, // Time-based exit for NewPair/MeanReversion
    // ✅ ML LEARNING: Store features for training when trade exits
    pub ml_features: Option<Vec<f64>>, // Features used for entry decision
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub id: i64,
    pub strategy: String,
    pub token_address: String,
    pub token_symbol: String,
    pub entry_price: f64,
    pub exit_price: Option<f64>,
    pub amount_sol: f64,
    pub tokens_bought: f64,
    pub tokens_sold: Option<f64>,
    pub entry_time: i64,
    pub exit_time: Option<i64>,
    pub pnl_sol: Option<f64>,
    pub pnl_percent: Option<f64>,
    pub status: String,
    pub tx_signature_buy: String,
    pub tx_signature_sell: Option<String>,
    pub fees_sol: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPool {
    pub token_address: String,
    pub token_symbol: String,
    pub pair_address: String,
    pub liquidity_usd: f64,
    pub liquidity_sol: f64,
    pub price_usd: f64,
    pub volume_1h: f64,
    pub volume_6h: f64,
    pub volume_24h: f64,
    pub price_change_24h: f64,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JupiterQuote {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "inAmount")]
    pub in_amount: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "outAmount")]
    pub out_amount: String,
    #[serde(rename = "otherAmountThreshold")]
    pub other_amount_threshold: String,
    #[serde(rename = "swapMode")]
    pub swap_mode: String,
    #[serde(rename = "slippageBps")]
    pub slippage_bps: u64,
    #[serde(rename = "priceImpactPct")]
    pub price_impact_pct: String,
    #[serde(rename = "routePlan")]
    pub route_plan: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JupiterSwapRequest {
    #[serde(rename = "quoteResponse")]
    pub quote_response: JupiterQuote,
    #[serde(rename = "userPublicKey")]
    pub user_public_key: String,
    #[serde(rename = "wrapAndUnwrapSol")]
    pub wrap_and_unwrap_sol: bool,
    #[serde(rename = "computeUnitPriceMicroLamports")]
    pub compute_unit_price_micro_lamports: Option<u64>,
    #[serde(rename = "prioritizationFeeLamports")]
    pub prioritization_fee_lamports: Option<u64>,
    #[serde(rename = "asLegacyTransaction")]
    pub as_legacy_transaction: bool,
    #[serde(rename = "dynamicComputeUnitLimit")]
    pub dynamic_compute_unit_limit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JupiterSwapResponse {
    #[serde(rename = "swapTransaction")]
    pub swap_transaction: String,
}

#[derive(Debug, Clone)]
pub struct SwapResult {
    pub success: bool,
    pub signature: Option<String>,
    pub input_amount: u64,
    pub output_amount: u64,
    pub price_impact: f64,
    pub fees: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SafetyCheckResult {
    pub passed: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AllTimeStats {
    pub total_trades: usize,
    pub win_count: usize,
    pub loss_count: usize,
    pub win_rate: f64,
    pub total_pnl: f64,
    pub avg_pnl: f64,
    pub best_trade: f64,
    pub worst_trade: f64,
    pub avg_hold_time_minutes: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyStats {
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub total_pnl: f64,
    pub win_rate: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub largest_win: f64,
    pub largest_loss: f64,
    pub avg_hold_time_minutes: f64,
    pub open_positions: usize,
    pub pnl_percent: f64,
    pub max_drawdown_percent: f64,
    pub consecutive_losses: usize,
}

