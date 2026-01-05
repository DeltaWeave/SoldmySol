/// On-chain pool liquidity validator (Stage A - before Jupiter)
/// Validates that a pool has sufficient liquidity for trading
///
/// This is CRITICAL for sniper efficiency:
/// - Don't waste time on Jupiter for pools with no liquidity
/// - Don't spam Jupiter API for non-tradeable pools
/// - Get deterministic pool state directly from chain

use anyhow::{anyhow, Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use tracing::{debug, info, warn};

/// Minimum liquidity thresholds (in lamports)
const MIN_SOL_LIQUIDITY_LAMPORTS: u64 = 1_000_000_000; // 1 SOL
const MIN_TOKEN_LIQUIDITY: u64 = 1_000_000; // 1M tokens (adjust based on decimals)

/// Result of pool account validation check
#[derive(Debug, Clone)]
pub enum PoolAccountStatus {
    /// Pool account not readable yet (0 bytes) - needs retry, don't reject
    NotReady { pool_address: String, attempts: u32 },

    /// Pool account readable but no SOL pair or insufficient liquidity
    NoLiquidity { pool_address: String, sol_liquidity: f64 },

    /// Pool validated with sufficient liquidity
    Valid(PoolLiquidityState),
}

#[derive(Debug, Clone)]
pub struct PoolLiquidityState {
    pub pool_address: String,
    pub base_mint: String,
    pub quote_mint: String,
    pub base_reserve: u64,
    pub quote_reserve: u64,
    pub liquidity_usd: f64,
    pub estimated_price: f64,
    pub tradeable: bool,
    pub dex_type: DexType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DexType {
    RaydiumAMM,
    RaydiumCLMM,
    OrcaWhirlpool,
    Meteora,
    Unknown,
}

pub struct PoolValidator {
    rpc_client: RpcClient,
}

impl PoolValidator {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_client: RpcClient::new_with_commitment(
                rpc_url,
                CommitmentConfig::processed(), // Use processed for fastest pool detection
            ),
        }
    }

    /// Fetch account data with retry logic for newly created accounts
    /// Pool accounts may not be immediately available after creation event
    async fn fetch_account_with_retry(
        &self,
        pubkey: &Pubkey,
        max_retries: u32,
    ) -> Result<Vec<u8>> {
        let mut attempts = 0;
        let mut delay_ms = 150; // Start with 150ms delay

        loop {
            match self.rpc_client.get_account_data(pubkey) {
                Ok(data) if !data.is_empty() => {
                    if attempts > 0 {
                        info!("✅ Account data received after {} retries", attempts);
                    }
                    return Ok(data);
                }
                Ok(_) => {
                    // Empty account data
                    if attempts >= max_retries {
                        return Err(anyhow!("Account data empty after {} attempts", attempts + 1));
                    }
                }
                Err(e) => {
                    if attempts >= max_retries {
                        return Err(anyhow!("Failed to fetch account after {} attempts: {}", attempts + 1, e));
                    }
                }
            }

            attempts += 1;
            info!("⏳ Retry {}/{}: waiting {}ms for pool {}", attempts, max_retries, delay_ms, &pubkey.to_string()[..8]);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            delay_ms = (delay_ms * 2).min(800); // Exponential backoff, max 800ms
        }
    }

    /// Main validation entry point
    /// Returns PoolAccountStatus to distinguish "not ready" from "no liquidity"
    pub async fn validate_pool_liquidity(
        &self,
        pool_address: &str,
        dex_program: &str,
        snipe_amount_sol: f64,
    ) -> Result<PoolAccountStatus> {
        // Determine DEX type
        let dex_type = self.identify_dex_type(dex_program);

        debug!(
            "🔍 Stage A validation: pool={}, dex={:?}, amount={} SOL",
            pool_address, dex_type, snipe_amount_sol
        );

        // Route to appropriate validator
        match dex_type {
            DexType::RaydiumCLMM => self.validate_raydium_clmm(pool_address, snipe_amount_sol).await,
            DexType::OrcaWhirlpool => self.validate_orca_whirlpool(pool_address, snipe_amount_sol).await,
            DexType::RaydiumAMM => self.validate_raydium_amm(pool_address, snipe_amount_sol).await,
            DexType::Meteora => self.validate_meteora(pool_address, snipe_amount_sol).await,
            DexType::Unknown => {
                // For unknown DEX types (PumpFun, Phoenix, etc.), skip on-chain validation
                // and rely on Jupiter to determine if it's tradeable
                info!("⏭️  Skipping Stage A for {} - will validate via Jupiter", pool_address);
                Ok(PoolAccountStatus::Valid(PoolLiquidityState {
                    pool_address: pool_address.to_string(),
                    base_mint: "unknown".to_string(),
                    quote_mint: "So11111111111111111111111111111111111111112".to_string(), // SOL
                    base_reserve: 0,
                    quote_reserve: 0,
                    liquidity_usd: 0.0, // Will be determined by Jupiter
                    estimated_price: 0.0,
                    tradeable: true, // Assume tradeable, Jupiter will verify
                    dex_type: DexType::Unknown,
                }))
            }
        }
    }

    fn identify_dex_type(&self, program_id: &str) -> DexType {
        match program_id {
            "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK" => DexType::RaydiumCLMM,
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8" => DexType::RaydiumAMM,
            "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc" => DexType::OrcaWhirlpool,
            "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo" => DexType::Meteora,
            "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P" => DexType::Unknown, // PumpFun - skip on-chain validation
            "39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg" => DexType::Unknown, // Raydium Migration
            "PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY" => DexType::Unknown, // Phoenix
            "2wT8Yq49kHgDzXuPxZSaeLaH1qbmGXtEyPy64bL7aD3c" => DexType::Unknown, // Lifinity
            _ => DexType::Unknown,
        }
    }

    /// Validate Raydium CLMM pool
    async fn validate_raydium_clmm(
        &self,
        pool_address: &str,
        snipe_amount_sol: f64,
    ) -> Result<PoolAccountStatus> {
        let pool_pubkey = Pubkey::from_str(pool_address)
            .context("Invalid pool address")?;

        // Fetch pool account data with retries (pool may not exist immediately after creation event)
        let account_data = match self.fetch_account_with_retry(&pool_pubkey, 5).await {
            Ok(data) => data,
            Err(e) => {
                // Account not readable yet - return NotReady, don't reject!
                info!("⚠️  CLMM pool account not ready {}: {:?}", pool_address, e);
                return Ok(PoolAccountStatus::NotReady {
                    pool_address: pool_address.to_string(),
                    attempts: 5,
                });
            }
        };

        // Parse Raydium CLMM pool state
        // Pool state structure (simplified):
        // - token_0_mint: [32]u8
        // - token_1_mint: [32]u8
        // - token_0_vault: [32]u8
        // - token_1_vault: [32]u8
        // - current_tick: i32
        // - liquidity: u128

        if account_data.len() < 500 {
            info!("⚠️  CLMM pool account too small: {} bytes", account_data.len());
            return Ok(PoolAccountStatus::NotReady {
                pool_address: pool_address.to_string(),
                attempts: 5,
            });
        }

        // Extract mints (positions 8-40, 40-72 typically)
        let token_0_mint = Pubkey::new_from_array(
            account_data[8..40].try_into().unwrap_or([0u8; 32])
        );
        let token_1_mint = Pubkey::new_from_array(
            account_data[40..72].try_into().unwrap_or([0u8; 32])
        );

        // Extract vault addresses (to check reserves)
        let token_0_vault = Pubkey::new_from_array(
            account_data[72..104].try_into().unwrap_or([0u8; 32])
        );
        let token_1_vault = Pubkey::new_from_array(
            account_data[104..136].try_into().unwrap_or([0u8; 32])
        );

        // Fetch vault balances
        let (reserve_0, reserve_1) = self.fetch_vault_balances(&token_0_vault, &token_1_vault)?;

        info!(
            "🔍 CLMM vaults: mint0={}, mint1={}, reserve0={} ({:.4}), reserve1={} ({:.4})",
            &token_0_mint.to_string()[..8],
            &token_1_mint.to_string()[..8],
            reserve_0,
            reserve_0 as f64 / 1e9,
            reserve_1,
            reserve_1 as f64 / 1e9
        );

        // Determine which is SOL (wrapped SOL)
        let wsol_mint = "So11111111111111111111111111111111111111112";
        let (base_mint, quote_mint, base_reserve, quote_reserve) =
            if token_0_mint.to_string() == wsol_mint {
                (token_1_mint.to_string(), token_0_mint.to_string(), reserve_1, reserve_0)
            } else if token_1_mint.to_string() == wsol_mint {
                (token_0_mint.to_string(), token_1_mint.to_string(), reserve_0, reserve_1)
            } else {
                // No SOL pair, skip
                info!("⚠️  CLMM pool has no SOL pair (token-token pool)");
                return Ok(PoolAccountStatus::NoLiquidity {
                    pool_address: pool_address.to_string(),
                    sol_liquidity: 0.0,
                });
            };

        // Check minimum liquidity
        if quote_reserve < MIN_SOL_LIQUIDITY_LAMPORTS {
            info!(
                "⚠️  CLMM pool below min: {:.4} SOL (need {:.1} SOL)",
                quote_reserve as f64 / 1e9,
                MIN_SOL_LIQUIDITY_LAMPORTS as f64 / 1e9
            );
            return Ok(PoolAccountStatus::NoLiquidity {
                pool_address: pool_address.to_string(),
                sol_liquidity: quote_reserve as f64 / 1e9,
            });
        }

        // Estimate price (simple ratio - not accounting for ticks/sqrt price)
        let estimated_price = if base_reserve > 0 {
            (quote_reserve as f64 / 1e9) / (base_reserve as f64 / 1e6) // Assumes 6 decimals
        } else {
            0.0
        };

        let liquidity_usd = (quote_reserve as f64 / 1e9) * 150.0; // Assume $150 SOL

        info!(
            "✅ CLMM pool VALID: {} SOL liquidity, price ~${:.9}, liq ~${:.0}",
            quote_reserve as f64 / 1e9,
            estimated_price,
            liquidity_usd
        );

        Ok(PoolAccountStatus::Valid(PoolLiquidityState {
            pool_address: pool_address.to_string(),
            base_mint,
            quote_mint,
            base_reserve,
            quote_reserve,
            liquidity_usd,
            estimated_price,
            tradeable: true,
            dex_type: DexType::RaydiumCLMM,
        }))
    }

    /// Validate Orca Whirlpool
    async fn validate_orca_whirlpool(
        &self,
        pool_address: &str,
        snipe_amount_sol: f64,
    ) -> Result<PoolAccountStatus> {
        let pool_pubkey = Pubkey::from_str(pool_address)
            .context("Invalid pool address")?;

        // Fetch pool account data with retries (pool may not exist immediately after creation event)
        let account_data = match self.fetch_account_with_retry(&pool_pubkey, 5).await {
            Ok(data) => data,
            Err(e) => {
                info!("⚠️  Failed to fetch Whirlpool account {}: {:?}", pool_address, e);
                return Ok(PoolAccountStatus::NotReady { pool_address: pool_address.to_string(), attempts: 5 });
            }
        };

        if account_data.len() < 500 {
            info!("⚠️  Whirlpool account too small: {} bytes", account_data.len());
            return Ok(PoolAccountStatus::NotReady { pool_address: pool_address.to_string(), attempts: 5 });
        }

        // Whirlpool layout (similar to CLMM):
        // - discriminator: 8
        // - whirlpool_config: 32
        // - token_mint_a: 32
        // - token_mint_b: 32
        // - token_vault_a: 32
        // - token_vault_b: 32
        // - current_sqrt_price: u128
        // - liquidity: u128

        let token_a_mint = Pubkey::new_from_array(
            account_data[40..72].try_into().unwrap_or([0u8; 32])
        );
        let token_b_mint = Pubkey::new_from_array(
            account_data[72..104].try_into().unwrap_or([0u8; 32])
        );
        let token_a_vault = Pubkey::new_from_array(
            account_data[104..136].try_into().unwrap_or([0u8; 32])
        );
        let token_b_vault = Pubkey::new_from_array(
            account_data[136..168].try_into().unwrap_or([0u8; 32])
        );

        let (reserve_a, reserve_b) = self.fetch_vault_balances(&token_a_vault, &token_b_vault)?;

        info!(
            "🔍 Whirlpool vaults: mintA={}, mintB={}, reserveA={} ({:.4}), reserveB={} ({:.4})",
            &token_a_mint.to_string()[..8],
            &token_b_mint.to_string()[..8],
            reserve_a,
            reserve_a as f64 / 1e9,
            reserve_b,
            reserve_b as f64 / 1e9
        );

        // Determine SOL pair
        let wsol_mint = "So11111111111111111111111111111111111111112";
        let (base_mint, quote_mint, base_reserve, quote_reserve) =
            if token_a_mint.to_string() == wsol_mint {
                (token_b_mint.to_string(), token_a_mint.to_string(), reserve_b, reserve_a)
            } else if token_b_mint.to_string() == wsol_mint {
                (token_a_mint.to_string(), token_b_mint.to_string(), reserve_a, reserve_b)
            } else {
                info!("⚠️  Whirlpool has no SOL pair (token-token pool)");
                return Ok(PoolAccountStatus::NoLiquidity { pool_address: pool_address.to_string(), sol_liquidity: 0.0 });
            };

        if quote_reserve < MIN_SOL_LIQUIDITY_LAMPORTS {
            info!(
                "⚠️  Whirlpool below min: {:.4} SOL (need {:.1} SOL)",
                quote_reserve as f64 / 1e9,
                MIN_SOL_LIQUIDITY_LAMPORTS as f64 / 1e9
            );
            return Ok(PoolAccountStatus::NoLiquidity { pool_address: pool_address.to_string(), sol_liquidity: quote_reserve as f64 / 1e9 });
        }

        let estimated_price = if base_reserve > 0 {
            (quote_reserve as f64 / 1e9) / (base_reserve as f64 / 1e6)
        } else {
            0.0
        };

        let liquidity_usd = (quote_reserve as f64 / 1e9) * 150.0;

        info!(
            "✅ Whirlpool VALID: {} SOL liquidity, price ~${:.9}",
            quote_reserve as f64 / 1e9,
            estimated_price
        );

        Ok(PoolAccountStatus::Valid(PoolLiquidityState {
            pool_address: pool_address.to_string(),
            base_mint,
            quote_mint,
            base_reserve,
            quote_reserve,
            liquidity_usd,
            estimated_price,
            tradeable: true,
            dex_type: DexType::OrcaWhirlpool,
        }))
    }

    /// Validate Raydium AMM (standard XYK)
    async fn validate_raydium_amm(
        &self,
        pool_address: &str,
        _snipe_amount_sol: f64,
    ) -> Result<PoolAccountStatus> {
        let pool_pubkey = Pubkey::from_str(pool_address)
            .context("Invalid pool address")?;

        let account_data = match self.fetch_account_with_retry(&pool_pubkey, 5).await {
            Ok(data) => data,
            Err(e) => {
                info!("⚠️  Failed to fetch Raydium AMM account {}: {:?}", pool_address, e);
                return Ok(PoolAccountStatus::NotReady { pool_address: pool_address.to_string(), attempts: 5 });
            }
        };

        if account_data.len() < 464 {
            info!("⚠️  Raydium AMM account too small: {} bytes", account_data.len());
            return Ok(PoolAccountStatus::NotReady { pool_address: pool_address.to_string(), attempts: 5 });
        }

        // Raydium AMM v4 layout (approximate offsets):
        // base_vault: 336..368
        // quote_vault: 368..400
        // base_mint: 400..432
        // quote_mint: 432..464
        let base_vault = Pubkey::new_from_array(
            account_data[336..368].try_into().unwrap_or([0u8; 32])
        );
        let quote_vault = Pubkey::new_from_array(
            account_data[368..400].try_into().unwrap_or([0u8; 32])
        );
        let base_mint = Pubkey::new_from_array(
            account_data[400..432].try_into().unwrap_or([0u8; 32])
        );
        let quote_mint = Pubkey::new_from_array(
            account_data[432..464].try_into().unwrap_or([0u8; 32])
        );

        let (reserve_base, reserve_quote) = self.fetch_vault_balances(&base_vault, &quote_vault)?;

        info!(
            "🔍 Raydium AMM vaults: base={}, quote={}, reserve_base={} ({:.4}), reserve_quote={} ({:.4})",
            &base_mint.to_string()[..8],
            &quote_mint.to_string()[..8],
            reserve_base,
            reserve_base as f64 / 1e9,
            reserve_quote,
            reserve_quote as f64 / 1e9
        );

        let wsol_mint = "So11111111111111111111111111111111111111112";
        let (base_mint_str, quote_mint_str, base_reserve, quote_reserve) =
            if base_mint.to_string() == wsol_mint {
                (quote_mint.to_string(), base_mint.to_string(), reserve_quote, reserve_base)
            } else if quote_mint.to_string() == wsol_mint {
                (base_mint.to_string(), quote_mint.to_string(), reserve_base, reserve_quote)
            } else {
                info!("⚠️  Raydium AMM pool has no SOL pair (token-token pool)");
                return Ok(PoolAccountStatus::NoLiquidity {
                    pool_address: pool_address.to_string(),
                    sol_liquidity: 0.0,
                });
            };

        if quote_reserve < MIN_SOL_LIQUIDITY_LAMPORTS {
            info!(
                "⚠️  Raydium AMM below min: {:.4} SOL (need {:.1} SOL)",
                quote_reserve as f64 / 1e9,
                MIN_SOL_LIQUIDITY_LAMPORTS as f64 / 1e9
            );
            return Ok(PoolAccountStatus::NoLiquidity {
                pool_address: pool_address.to_string(),
                sol_liquidity: quote_reserve as f64 / 1e9,
            });
        }

        let estimated_price = if base_reserve > 0 {
            (quote_reserve as f64 / 1e9) / (base_reserve as f64 / 1e6)
        } else {
            0.0
        };

        let liquidity_usd = (quote_reserve as f64 / 1e9) * 150.0;

        info!(
            "✅ Raydium AMM VALID: {} SOL liquidity, price ~${:.9}",
            quote_reserve as f64 / 1e9,
            estimated_price
        );

        Ok(PoolAccountStatus::Valid(PoolLiquidityState {
            pool_address: pool_address.to_string(),
            base_mint: base_mint_str,
            quote_mint: quote_mint_str,
            base_reserve,
            quote_reserve,
            liquidity_usd,
            estimated_price,
            tradeable: true,
            dex_type: DexType::RaydiumAMM,
        }))
    }

    /// Validate Meteora pool
    async fn validate_meteora(
        &self,
        pool_address: &str,
        _snipe_amount_sol: f64,
    ) -> Result<PoolAccountStatus> {
        let pool_pubkey = Pubkey::from_str(pool_address)
            .context("Invalid pool address")?;

        let account_data = match self.fetch_account_with_retry(&pool_pubkey, 5).await {
            Ok(data) => data,
            Err(e) => {
                info!("⚠️  Failed to fetch Meteora account {}: {:?}", pool_address, e);
                return Ok(PoolAccountStatus::NotReady { pool_address: pool_address.to_string(), attempts: 5 });
            }
        };

        if account_data.len() < 136 {
            info!("⚠️  Meteora account too small: {} bytes", account_data.len());
            return Ok(PoolAccountStatus::NotReady { pool_address: pool_address.to_string(), attempts: 5 });
        }

        // Meteora DLMM pools are CLMM-style. Use generic offsets as a best-effort parse.
        let token_a_mint = Pubkey::new_from_array(
            account_data[8..40].try_into().unwrap_or([0u8; 32])
        );
        let token_b_mint = Pubkey::new_from_array(
            account_data[40..72].try_into().unwrap_or([0u8; 32])
        );
        let token_a_vault = Pubkey::new_from_array(
            account_data[72..104].try_into().unwrap_or([0u8; 32])
        );
        let token_b_vault = Pubkey::new_from_array(
            account_data[104..136].try_into().unwrap_or([0u8; 32])
        );

        let (reserve_a, reserve_b) = self.fetch_vault_balances(&token_a_vault, &token_b_vault)?;

        info!(
            "🔍 Meteora vaults: mintA={}, mintB={}, reserveA={} ({:.4}), reserveB={} ({:.4})",
            &token_a_mint.to_string()[..8],
            &token_b_mint.to_string()[..8],
            reserve_a,
            reserve_a as f64 / 1e9,
            reserve_b,
            reserve_b as f64 / 1e9
        );

        let wsol_mint = "So11111111111111111111111111111111111111112";
        let (base_mint, quote_mint, base_reserve, quote_reserve) =
            if token_a_mint.to_string() == wsol_mint {
                (token_b_mint.to_string(), token_a_mint.to_string(), reserve_b, reserve_a)
            } else if token_b_mint.to_string() == wsol_mint {
                (token_a_mint.to_string(), token_b_mint.to_string(), reserve_a, reserve_b)
            } else {
                info!("⚠️  Meteora pool has no SOL pair (token-token pool)");
                return Ok(PoolAccountStatus::NoLiquidity {
                    pool_address: pool_address.to_string(),
                    sol_liquidity: 0.0,
                });
            };

        if quote_reserve < MIN_SOL_LIQUIDITY_LAMPORTS {
            info!(
                "⚠️  Meteora below min: {:.4} SOL (need {:.1} SOL)",
                quote_reserve as f64 / 1e9,
                MIN_SOL_LIQUIDITY_LAMPORTS as f64 / 1e9
            );
            return Ok(PoolAccountStatus::NoLiquidity {
                pool_address: pool_address.to_string(),
                sol_liquidity: quote_reserve as f64 / 1e9,
            });
        }

        let estimated_price = if base_reserve > 0 {
            (quote_reserve as f64 / 1e9) / (base_reserve as f64 / 1e6)
        } else {
            0.0
        };

        let liquidity_usd = (quote_reserve as f64 / 1e9) * 150.0;

        info!(
            "✅ Meteora VALID: {} SOL liquidity, price ~${:.9}",
            quote_reserve as f64 / 1e9,
            estimated_price
        );

        Ok(PoolAccountStatus::Valid(PoolLiquidityState {
            pool_address: pool_address.to_string(),
            base_mint,
            quote_mint,
            base_reserve,
            quote_reserve,
            liquidity_usd,
            estimated_price,
            tradeable: true,
            dex_type: DexType::Meteora,
        }))
    }

    /// Fetch token vault balances
    fn fetch_vault_balances(&self, vault_a: &Pubkey, vault_b: &Pubkey) -> Result<(u64, u64)> {
        // Fetch token account balances
        let balance_a = self.rpc_client
            .get_token_account_balance(vault_a)
            .ok()
            .and_then(|b| b.amount.parse::<u64>().ok())
            .unwrap_or(0);

        let balance_b = self.rpc_client
            .get_token_account_balance(vault_b)
            .ok()
            .and_then(|b| b.amount.parse::<u64>().ok())
            .unwrap_or(0);

        Ok((balance_a, balance_b))
    }
}
