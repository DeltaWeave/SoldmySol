use crate::services::{JupiterService, PoolValidator, PoolAccountStatus};
use anyhow::{anyhow, Result};
use std::time::Instant;
use tracing::{debug, info, warn};

/// ⭐ P0 CRITICAL: Professional sniper validation
/// TWO-STAGE validation:
/// Stage A: On-chain pool liquidity check (fast, deterministic)
/// Stage B: Jupiter route validation (slower, but confirms routing)
pub struct SniperValidator {
    jupiter: std::sync::Arc<JupiterService>,
    pool_validator: PoolValidator,
}

impl SniperValidator {
    pub fn new(jupiter: std::sync::Arc<JupiterService>, rpc_url: String) -> Self {
        Self {
            jupiter,
            pool_validator: PoolValidator::new(rpc_url),
        }
    }

    /// Validate token is tradeable with acceptable round-trip impact
    /// TWO-STAGE validation for sniper efficiency:
    /// 1. Stage A: On-chain pool check (fast, no API rate limits)
    /// 2. Stage B: Jupiter route validation (only if Stage A passes)
    ///
    /// Returns: (entry_amount_tokens, exit_amount_sol, impact_bps, latency_ms)
    pub async fn validate_token(
        &self,
        token_mint: &str,
        entry_amount_sol: f64,
        max_impact_bps: u64,
    ) -> Result<(u64, u64, u64, u64)> {
        let start = Instant::now();

        // P0.1: Validate entry + exit routes (Stage B only - Jupiter)
        let (entry_quote, exit_quote) = self
            .jupiter
            .validate_entry_and_exit(token_mint, entry_amount_sol, max_impact_bps)
            .await?;

        let entry_tokens: u64 = entry_quote
            .out_amount
            .parse()
            .map_err(|_| anyhow!("Invalid entry token amount"))?;

        let exit_sol_lamports: u64 = exit_quote
            .out_amount
            .parse()
            .map_err(|_| anyhow!("Invalid exit SOL amount"))?;

        // Calculate actual impact
        let entry_sol_lamports = (entry_amount_sol * 1_000_000_000.0) as u64;
        let impact_bps = if entry_sol_lamports > 0 {
            let loss = entry_sol_lamports.saturating_sub(exit_sol_lamports);
            ((loss as f64 / entry_sol_lamports as f64) * 10000.0) as u64
        } else {
            10000
        };

        let latency_ms = start.elapsed().as_millis() as u64;

        // P1.1: Warn if validation too slow
        if latency_ms > 400 {
            warn!(
                "⚠️  Slow validation: {}ms for {} (target <400ms)",
                latency_ms, token_mint
            );
        }

        info!(
            "✅ Token {} validated in {}ms: {} SOL → {} tokens → {} SOL (impact: {}bps)",
            token_mint,
            latency_ms,
            entry_amount_sol,
            entry_tokens,
            exit_sol_lamports as f64 / 1_000_000_000.0,
            impact_bps
        );

        Ok((entry_tokens, exit_sol_lamports, impact_bps, latency_ms))
    }

    /// Stage A: On-chain pool liquidity validation (FAST - no Jupiter API)
    /// This is called BEFORE Jupiter to avoid wasting API calls on pools with no liquidity
    ///
    /// Returns: PoolAccountStatus (NotReady, NoLiquidity, or Valid)
    pub async fn validate_pool_liquidity(
        &self,
        pool_address: &str,
        dex_program: &str,
        snipe_amount_sol: f64,
    ) -> Result<PoolAccountStatus> {
        debug!(
            "🔍 Stage A validation: pool={}, dex={}, amount={} SOL",
            pool_address, dex_program, snipe_amount_sol
        );

        self.pool_validator
            .validate_pool_liquidity(pool_address, dex_program, snipe_amount_sol)
            .await
    }

    /// Stage B: Jupiter route validation (SLOW - only call after Stage A passes)
    /// Returns: (entry_tokens, exit_sol_lamports, impact_bps, latency_ms)
    pub async fn validate_jupiter_routes(
        &self,
        token_mint: &str,
        entry_amount_sol: f64,
        max_impact_bps: u64,
    ) -> Result<(u64, u64, u64, u64)> {
        // This is the same as the old validate_token logic
        self.validate_token(token_mint, entry_amount_sol, max_impact_bps)
            .await
    }

    /// Quick check if token has ANY route (faster, less thorough)
    pub async fn has_basic_route(&self, token_mint: &str) -> bool {
        let sol_mint = "So11111111111111111111111111111111111111112";
        self.jupiter
            .get_quote(sol_mint, token_mint, 10_000_000, 500)
            .await
            .is_ok()
    }
}
