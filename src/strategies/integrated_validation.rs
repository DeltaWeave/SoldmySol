/// Integrated validation flow: Stage A -> Stage B -> Direct DEX Fallback
/// This is a temporary file to hold the new validation logic
/// Will be integrated into token_sniper.rs

use anyhow::Result;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Integrated validation with all improvements:
/// 1. Add to validation queue (non-blocking)
/// 2. Stage A: On-chain pool liquidity check
/// 3. Stage B: Jupiter route validation
/// 4. Fallback: Direct DEX quote if Jupiter unavailable
///
/// Returns: true if validation succeeded, false if rejected
pub async fn validate_token_integrated(
    token_mint: String,
    pool_address: String,
    dex_program: String,
    validation_queue: &crate::services::ValidationQueue,
    pool_validator: &crate::services::PoolValidator,
    jupiter: &crate::services::JupiterService,
    direct_dex: &crate::services::DirectDexSwap,
    entry_amount_sol: f64,
    max_impact_bps: u64,
) -> Result<bool> {
    // Step 1: Add to validation queue
    let added = validation_queue.add_token(
        token_mint.clone(),
        pool_address.clone(),
        dex_program.clone(),
    ).await;

    if !added {
        warn!("❌ Queue rejected {} (at capacity or duplicate)", token_mint);
        return Ok(false);
    }

    // Step 2: Stage A - On-chain pool liquidity validation
    info!("🔍 STAGE_A | token={} | pool={} | checking on-chain liquidity", token_mint, pool_address);

    let pool_state = match pool_validator.validate_pool_liquidity(
        &pool_address,
        &dex_program,
        entry_amount_sol,
    ).await? {
        Some(state) => {
            info!(
                "✅ STAGE_A_PASS | token={} | liquidity={:.2} SOL | price=${:.9}",
                token_mint, state.quote_reserve as f64 / 1e9, state.estimated_price
            );

            // Update queue
            validation_queue.update_liquidity_check(
                &token_mint,
                Some(state.quote_reserve as f64 / 1e9),
                Some(state.estimated_price),
            ).await?;

            state
        }
        None => {
            warn!("❌ STAGE_A_FAIL | token={} | reason=no_liquidity", token_mint);
            validation_queue.reject_token(&token_mint, "No on-chain liquidity".to_string()).await;
            return Ok(false);
        }
    };

    // Step 3: Stage B - Jupiter route validation (with retries)
    info!("🔍 STAGE_B | token={} | checking Jupiter routes", token_mint);

    let sol_mint = "So11111111111111111111111111111111111111112";
    let retry_delays = vec![0, 250, 250, 500, 1000, 2000, 4000]; // ms
    let max_validation_time_ms = 20_000;

    let mut jupiter_success = false;
    let mut last_error: Option<String> = None;

    for (attempt, &delay_ms) in retry_delays.iter().enumerate() {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        // Try Jupiter quote
        let entry_quote_result = jupiter.get_quote(
            sol_mint,
            &token_mint,
            (entry_amount_sol * 1e9) as u64,
            500,
        ).await;

        match entry_quote_result {
            Ok(entry_quote) => {
                // Got entry quote, now try exit
                let entry_tokens: u64 = entry_quote.out_amount.parse().unwrap_or(0);

                let exit_quote_result = jupiter.get_quote(
                    &token_mint,
                    sol_mint,
                    entry_tokens,
                    500,
                ).await;

                match exit_quote_result {
                    Ok(exit_quote) => {
                        let exit_sol: u64 = exit_quote.out_amount.parse().unwrap_or(0);
                        let entry_sol = (entry_amount_sol * 1e9) as u64;
                        let impact_bps = if entry_sol > 0 {
                            let loss = entry_sol.saturating_sub(exit_sol);
                            ((loss as f64 / entry_sol as f64) * 10000.0) as u64
                        } else {
                            10000
                        };

                        info!(
                            "✅ STAGE_B_PASS | token={} | impact={}bps | attempt={}/{}",
                            token_mint, impact_bps, attempt + 1, retry_delays.len()
                        );

                        // Update queue
                        validation_queue.update_route_check(
                            &token_mint,
                            true,
                            true,
                            Some(impact_bps),
                        ).await?;

                        jupiter_success = true;
                        break;
                    }
                    Err(e) => {
                        last_error = Some(format!("exit_failed: {:?}", e));
                        debug!("Exit quote failed (attempt {}): {:?}", attempt + 1, e);
                    }
                }
            }
            Err(e) => {
                last_error = Some(format!("entry_failed: {:?}", e));
                debug!("Entry quote failed (attempt {}): {:?}", attempt + 1, e);
            }
        }
    }

    // Step 4: Fallback to Direct DEX if Jupiter failed
    if !jupiter_success {
        warn!(
            "⚠️  STAGE_B_FAIL | token={} | reason={} | trying direct DEX...",
            token_mint,
            last_error.as_ref().map(|s| s.as_str()).unwrap_or("unknown")
        );

        // Try direct DEX quote
        let direct_quote_result = if dex_program.contains("CAMM") || dex_program.contains("Raydium") {
            direct_dex.get_raydium_clmm_quote(
                &pool_address,
                sol_mint,
                &pool_state.base_mint,
                (entry_amount_sol * 1e9) as u64,
            ).await
        } else if dex_program.contains("Orca") || dex_program.contains("whir") {
            direct_dex.get_orca_whirlpool_quote(
                &pool_address,
                sol_mint,
                &pool_state.base_mint,
                (entry_amount_sol * 1e9) as u64,
            ).await
        } else {
            Err(anyhow::anyhow!("Unsupported DEX for direct quote"))
        };

        match direct_quote_result {
            Ok(direct_quote) => {
                info!(
                    "✅ DIRECT_DEX_PASS | token={} | output={} | impact={}bps",
                    token_mint, direct_quote.output_amount, direct_quote.price_impact_bps
                );

                // Mark as validated via direct route
                validation_queue.update_route_check(
                    &token_mint,
                    true,
                    true,
                    Some(direct_quote.price_impact_bps),
                ).await?;

                return Ok(true);
            }
            Err(e) => {
                error!(
                    "❌ DIRECT_DEX_FAIL | token={} | error={:?}",
                    token_mint, e
                );
                validation_queue.reject_token(&token_mint, "No route (Jupiter + Direct DEX failed)".to_string()).await;
                return Ok(false);
            }
        }
    }

    Ok(jupiter_success)
}
