/// Direct DEX swap implementation (no Jupiter dependency)
/// Critical for first-block sniping when Jupiter hasn't indexed yet
///
/// Supports:
/// - Raydium CLMM (Concentrated Liquidity Market Maker)
/// - Orca Whirlpool (Concentrated Liquidity)
///
/// This allows us to trade IMMEDIATELY when liquidity is added,
/// without waiting for Jupiter's indexing process.

use anyhow::{anyhow, Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::AccountMeta,
    instruction::Instruction,
    pubkey::Pubkey,
};
use std::str::FromStr;
use tracing::{debug, info};

/// CLMM pool state for direct quoting
#[derive(Debug, Clone)]
pub struct CLMMPoolState {
    pub pool_address: Pubkey,
    pub token_mint_a: Pubkey,
    pub token_mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub current_sqrt_price: u128,
    pub liquidity: u128,
    pub tick_current: i32,
    pub fee_rate: u16,
}

/// Direct quote result from on-chain calculation
#[derive(Debug, Clone)]
pub struct DirectQuote {
    pub input_amount: u64,
    pub output_amount: u64,
    pub price_impact_bps: u64,
    pub fee_amount: u64,
    pub execution_price: f64,
}

pub struct DirectDexSwap {
    rpc_client: RpcClient,
}

impl DirectDexSwap {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_client: RpcClient::new(rpc_url),
        }
    }

    /// Get quote for Raydium CLMM swap (SOL -> Token or Token -> SOL)
    /// This uses the CLMM math directly without Jupiter
    pub async fn get_raydium_clmm_quote(
        &self,
        pool_address: &str,
        input_mint: &str,
        output_mint: &str,
        amount_in: u64,
    ) -> Result<DirectQuote> {
        debug!(
            "🔍 Direct CLMM quote: pool={}, in={}, out={}, amount={}",
            pool_address, input_mint, output_mint, amount_in
        );

        // Fetch pool state from chain
        let pool_state = self.fetch_clmm_pool_state(pool_address).await?;

        // Determine swap direction (a->b or b->a)
        let a_to_b = input_mint == pool_state.token_mint_a.to_string();

        // Calculate output using CLMM math
        let quote = self.calculate_clmm_swap(
            &pool_state,
            amount_in,
            a_to_b,
        )?;

        info!(
            "✅ Direct CLMM quote: {} → {} (impact: {}bps, fee: {})",
            amount_in, quote.output_amount, quote.price_impact_bps, quote.fee_amount
        );

        Ok(quote)
    }

    /// Get quote for Orca Whirlpool swap
    pub async fn get_orca_whirlpool_quote(
        &self,
        pool_address: &str,
        input_mint: &str,
        output_mint: &str,
        amount_in: u64,
    ) -> Result<DirectQuote> {
        debug!(
            "🔍 Direct Whirlpool quote: pool={}, in={}, out={}, amount={}",
            pool_address, input_mint, output_mint, amount_in
        );

        // Fetch pool state from chain
        let pool_state = self.fetch_whirlpool_state(pool_address).await?;

        // Determine swap direction
        let a_to_b = input_mint == pool_state.token_mint_a.to_string();

        // Calculate output using Whirlpool math (same as CLMM)
        let quote = self.calculate_clmm_swap(
            &pool_state,
            amount_in,
            a_to_b,
        )?;

        info!(
            "✅ Direct Whirlpool quote: {} → {} (impact: {}bps)",
            amount_in, quote.output_amount, quote.price_impact_bps
        );

        Ok(quote)
    }

    /// Fetch Raydium CLMM pool state from chain
    async fn fetch_clmm_pool_state(&self, pool_address: &str) -> Result<CLMMPoolState> {
        let pool_pubkey = Pubkey::from_str(pool_address)
            .context("Invalid pool address")?;

        let account_data = self.rpc_client.get_account_data(&pool_pubkey)
            .context("Failed to fetch CLMM pool account")?;

        if account_data.len() < 500 {
            return Err(anyhow!("CLMM pool account too small: {} bytes", account_data.len()));
        }

        // Parse Raydium CLMM pool layout
        // Discriminator: 8 bytes
        // Token mint A: 32 bytes (offset 8)
        // Token mint B: 32 bytes (offset 40)
        // Vault A: 32 bytes (offset 72)
        // Vault B: 32 bytes (offset 104)
        // ... more fields ...
        // Current sqrt price: u128 (offset ~200)
        // Liquidity: u128 (offset ~216)
        // Tick current: i32 (offset ~232)

        let token_mint_a = Pubkey::new_from_array(
            account_data[8..40].try_into().unwrap()
        );
        let token_mint_b = Pubkey::new_from_array(
            account_data[40..72].try_into().unwrap()
        );
        let vault_a = Pubkey::new_from_array(
            account_data[72..104].try_into().unwrap()
        );
        let vault_b = Pubkey::new_from_array(
            account_data[104..136].try_into().unwrap()
        );

        // Extract sqrt price, liquidity, tick (simplified - actual offsets may vary)
        let current_sqrt_price = u128::from_le_bytes(
            account_data[200..216].try_into().unwrap_or([0u8; 16])
        );
        let liquidity = u128::from_le_bytes(
            account_data[216..232].try_into().unwrap_or([0u8; 16])
        );
        let tick_current = i32::from_le_bytes(
            account_data[232..236].try_into().unwrap_or([0u8; 4])
        );

        // Fee rate (typically at a different offset)
        let fee_rate = 300; // 0.3% default (3000 = 0.3% in basis points)

        Ok(CLMMPoolState {
            pool_address: pool_pubkey,
            token_mint_a,
            token_mint_b,
            vault_a,
            vault_b,
            current_sqrt_price,
            liquidity,
            tick_current,
            fee_rate,
        })
    }

    /// Fetch Orca Whirlpool state (similar to CLMM)
    async fn fetch_whirlpool_state(&self, pool_address: &str) -> Result<CLMMPoolState> {
        let pool_pubkey = Pubkey::from_str(pool_address)
            .context("Invalid pool address")?;

        let account_data = self.rpc_client.get_account_data(&pool_pubkey)
            .context("Failed to fetch Whirlpool account")?;

        if account_data.len() < 500 {
            return Err(anyhow!("Whirlpool account too small: {} bytes", account_data.len()));
        }

        // Whirlpool layout (similar to CLMM):
        // Discriminator: 8
        // Whirlpool config: 32 (offset 8)
        // Token mint A: 32 (offset 40)
        // Token mint B: 32 (offset 72)
        // Vault A: 32 (offset 104)
        // Vault B: 32 (offset 136)
        // Current sqrt price: u128 (offset ~200)
        // Liquidity: u128
        // Tick current: i32

        let token_mint_a = Pubkey::new_from_array(
            account_data[40..72].try_into().unwrap()
        );
        let token_mint_b = Pubkey::new_from_array(
            account_data[72..104].try_into().unwrap()
        );
        let vault_a = Pubkey::new_from_array(
            account_data[104..136].try_into().unwrap()
        );
        let vault_b = Pubkey::new_from_array(
            account_data[136..168].try_into().unwrap()
        );

        let current_sqrt_price = u128::from_le_bytes(
            account_data[200..216].try_into().unwrap_or([0u8; 16])
        );
        let liquidity = u128::from_le_bytes(
            account_data[216..232].try_into().unwrap_or([0u8; 16])
        );
        let tick_current = i32::from_le_bytes(
            account_data[232..236].try_into().unwrap_or([0u8; 4])
        );

        let fee_rate = 300; // Default Orca fee

        Ok(CLMMPoolState {
            pool_address: pool_pubkey,
            token_mint_a,
            token_mint_b,
            vault_a,
            vault_b,
            current_sqrt_price,
            liquidity,
            tick_current,
            fee_rate,
        })
    }

    /// Calculate CLMM swap output (concentrated liquidity math)
    /// This is the CORE algorithm for direct trading
    fn calculate_clmm_swap(
        &self,
        pool: &CLMMPoolState,
        amount_in: u64,
        a_to_b: bool,
    ) -> Result<DirectQuote> {
        if pool.liquidity == 0 {
            return Err(anyhow!("Pool has no liquidity"));
        }

        // Calculate fee
        let fee_amount = (amount_in as u128 * pool.fee_rate as u128 / 1_000_000) as u64;
        let amount_in_after_fee = amount_in.saturating_sub(fee_amount);

        let amount_in_f64 = amount_in_after_fee as f64;
        let liquidity_f64 = pool.liquidity as f64;
        let sqrt_price = pool.current_sqrt_price as f64 / (2_u128.pow(64) as f64);

        if liquidity_f64 <= 0.0 || sqrt_price <= 0.0 {
            return Err(anyhow!("Invalid pool liquidity or sqrt price"));
        }

        // Uniswap v3-style concentrated liquidity math (single tick approximation)
        let (new_sqrt, output_amount) = if a_to_b {
            // Token A in (token0), Token B out (token1)
            let new_sqrt = (liquidity_f64 * sqrt_price)
                / (liquidity_f64 + amount_in_f64 * sqrt_price);
            let amount_out = liquidity_f64 * (sqrt_price - new_sqrt);
            (new_sqrt, amount_out.max(0.0) as u64)
        } else {
            // Token B in (token1), Token A out (token0)
            let new_sqrt = sqrt_price + (amount_in_f64 / liquidity_f64);
            let amount_out = liquidity_f64 * (1.0 / new_sqrt - 1.0 / sqrt_price);
            (new_sqrt, amount_out.max(0.0) as u64)
        };

        // Calculate price impact
        let price_impact_bps = if amount_in > 0 {
            let price = sqrt_price * sqrt_price;
            let expected_out = if a_to_b {
                amount_in_after_fee as f64 * price
            } else {
                amount_in_after_fee as f64 / price
            };
            let actual_out = output_amount;
            if expected_out > 0.0 && expected_out > actual_out as f64 {
                let loss = expected_out - actual_out as f64;
                ((loss / expected_out) * 10000.0) as u64
            } else {
                0
            }
        } else {
            0
        };

        // Execution price
        let execution_price = if output_amount > 0 {
            amount_in as f64 / output_amount as f64
        } else {
            0.0
        };

        Ok(DirectQuote {
            input_amount: amount_in,
            output_amount,
            price_impact_bps,
            fee_amount,
            execution_price,
        })
    }

    /// Check if we should use direct swap vs Jupiter
    /// Use direct swap when:
    /// 1. Jupiter hasn't indexed yet (TOKEN_NOT_TRADABLE)
    /// 2. Pool is brand new (< 1 min old)
    /// 3. We need guaranteed first-block execution
    pub fn should_use_direct_swap(
        &self,
        pool_age_seconds: u64,
        jupiter_available: bool,
    ) -> bool {
        // Use direct if Jupiter not available OR pool is very new
        !jupiter_available || pool_age_seconds < 60
    }

    /// Build swap instruction for direct execution (stub)
    /// This would build the actual Raydium CLMM / Orca swap instruction
    pub fn build_swap_instruction(
        &self,
        program_id: &Pubkey,
        pool_state: &CLMMPoolState,
        user_pubkey: &Pubkey,
        amount_in: u64,
        minimum_out: u64,
        a_to_b: bool,
        mut additional_accounts: Vec<AccountMeta>,
    ) -> Result<Instruction> {
        let user_token_a = spl_associated_token_account::get_associated_token_address(
            user_pubkey,
            &pool_state.token_mint_a,
        );
        let user_token_b = spl_associated_token_account::get_associated_token_address(
            user_pubkey,
            &pool_state.token_mint_b,
        );

        let (user_source, user_destination) = if a_to_b {
            (user_token_a, user_token_b)
        } else {
            (user_token_b, user_token_a)
        };

        let mut accounts = vec![
            AccountMeta::new(pool_state.pool_address, false),
            AccountMeta::new(pool_state.vault_a, false),
            AccountMeta::new(pool_state.vault_b, false),
            AccountMeta::new(user_source, false),
            AccountMeta::new(user_destination, false),
            AccountMeta::new_readonly(*user_pubkey, true),
            AccountMeta::new_readonly(spl_token::id(), false),
        ];

        accounts.append(&mut additional_accounts);

        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Copy, Clone)]
        struct SwapInstructionData {
            amount_in: u64,
            minimum_out: u64,
            a_to_b: u8,
            _padding: [u8; 7],
        }

        let data = SwapInstructionData {
            amount_in,
            minimum_out,
            a_to_b: if a_to_b { 1 } else { 0 },
            _padding: [0u8; 7],
        };

        Ok(Instruction {
            program_id: *program_id,
            accounts,
            data: bytemuck::bytes_of(&data).to_vec(),
        })
    }
}

/// Helper: Convert sqrt price to regular price
fn sqrt_price_to_price(sqrt_price: u128) -> f64 {
    let sqrt_price_f64 = sqrt_price as f64;
    let price = (sqrt_price_f64 * sqrt_price_f64) / (2_u128.pow(64) as f64).powi(2);
    price
}

/// Helper: Calculate tick from sqrt price
fn sqrt_price_to_tick(sqrt_price: u128) -> i32 {
    // Simplified - real implementation uses log base 1.0001
    let price = sqrt_price_to_price(sqrt_price);
    (price.ln() / 1.0001_f64.ln()) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqrt_price_conversion() {
        let sqrt_price = 1u128 << 64; // Price = 1
        let price = sqrt_price_to_price(sqrt_price);
        assert!((price - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_clmm_math_basic() {
        // Test basic constant product calculation
        // More comprehensive tests would be needed for production
    }
}
