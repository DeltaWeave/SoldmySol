use anyhow::{anyhow, Context, Result};
use std::time::Instant;

use crate::config::Config;
use crate::models::JupiterQuote;
use crate::services::JupiterService;

pub struct RouteValidationResult {
    pub entry_quote: JupiterQuote,
    pub exit_quote: JupiterQuote,
    pub impact_bps: u64,
}

pub async fn validate_entry_exit_routes(
    jupiter: &JupiterService,
    config: &Config,
    input_mint: &str,
    output_mint: &str,
    amount: u64,
    slippage_bps: u64,
) -> Result<RouteValidationResult> {
    let entry_start = Instant::now();
    let entry_quote = jupiter
        .get_quote(input_mint, output_mint, amount, slippage_bps)
        .await
        .context("Failed to fetch entry quote")?;
    let entry_age_ms = entry_start.elapsed().as_millis() as u64;

    if entry_age_ms > config.performance.entry_quote_max_age_ms {
        return Err(anyhow!(
            "Entry quote too old: {}ms (max {}ms)",
            entry_age_ms,
            config.performance.entry_quote_max_age_ms
        ));
    }

    let entry_out_amount: u64 = entry_quote
        .out_amount
        .parse()
        .context("Failed to parse entry quote out_amount")?;

    if entry_out_amount == 0 {
        return Err(anyhow!("Entry quote returned zero output"));
    }

    let exit_start = Instant::now();
    let exit_quote = jupiter
        .get_quote(output_mint, input_mint, entry_out_amount, slippage_bps)
        .await
        .context("Failed to fetch exit quote")?;
    let exit_age_ms = exit_start.elapsed().as_millis() as u64;

    if exit_age_ms > config.performance.exit_quote_max_age_ms {
        return Err(anyhow!(
            "Exit quote too old: {}ms (max {}ms)",
            exit_age_ms,
            config.performance.exit_quote_max_age_ms
        ));
    }

    let exit_out_amount: u64 = exit_quote
        .out_amount
        .parse()
        .context("Failed to parse exit quote out_amount")?;

    let loss = amount.saturating_sub(exit_out_amount);
    let impact_bps = if amount > 0 {
        ((loss as f64 / amount as f64) * 10000.0) as u64
    } else {
        0
    };

    if impact_bps > config.sniper.max_exit_impact_bps {
        return Err(anyhow!(
            "Exit impact too high: {}bps (max {}bps)",
            impact_bps,
            config.sniper.max_exit_impact_bps
        ));
    }

    Ok(RouteValidationResult {
        entry_quote,
        exit_quote,
        impact_bps,
    })
}
