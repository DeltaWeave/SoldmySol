use crate::services::SolanaConnection;
use anyhow::{anyhow, Result};
use solana_client::rpc_config::{RpcProgramAccountsConfig, RpcAccountInfoConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::{commitment_config::CommitmentConfig, program_pack::Pack, pubkey::Pubkey};
use spl_token::state::{Account as TokenAccount, Mint};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct TokenSafetyCheck {
    pub passed: bool,
    pub reasons: Vec<String>,
    pub risk_score: u32, // 0-100, higher = more risky
}

pub struct TokenSafetyChecker {
    solana: std::sync::Arc<SolanaConnection>,
}

impl TokenSafetyChecker {
    pub fn new(solana: std::sync::Arc<SolanaConnection>) -> Self {
        Self { solana }
    }

    /// Comprehensive token safety check
    pub async fn check_token_safety(&self, token_mint: &Pubkey) -> Result<TokenSafetyCheck> {
        let mut reasons = Vec::new();
        let mut risk_score = 0u32;

        info!("🔍 Running safety checks on token: {}", token_mint);

        // Check 1: Mint Authority
        match self.check_mint_authority(token_mint).await {
            Ok(true) => {
                reasons.push("❌ CRITICAL: Mint authority NOT revoked - can create infinite tokens".to_string());
                risk_score += 40;
            }
            Ok(false) => {
                info!("  ✓ Mint authority revoked");
            }
            Err(e) => {
                warn!("  ⚠️  Could not check mint authority: {}", e);
                risk_score += 20;
            }
        }

        // Check 2: Freeze Authority
        match self.check_freeze_authority(token_mint).await {
            Ok(true) => {
                reasons.push("❌ CRITICAL: Freeze authority exists - can freeze your tokens".to_string());
                risk_score += 40;
            }
            Ok(false) => {
                info!("  ✓ Freeze authority revoked");
            }
            Err(e) => {
                warn!("  ⚠️  Could not check freeze authority: {}", e);
                risk_score += 20;
            }
        }

        // Check 3: Token Supply
        match self.check_supply(token_mint).await {
            Ok(supply) if supply == 0 => {
                reasons.push("❌ Token has zero supply".to_string());
                risk_score += 30;
            }
            Ok(supply) if supply > 1_000_000_000_000_000_000 => {
                reasons.push("⚠️  Extremely high supply - possible scam".to_string());
                risk_score += 15;
            }
            Ok(_) => {
                info!("  ✓ Supply looks normal");
            }
            Err(e) => {
                warn!("  ⚠️  Could not check supply: {}", e);
                risk_score += 10;
            }
        }

        // Check 4: Decimals (should be reasonable)
        match self.check_decimals(token_mint).await {
            Ok(decimals) if decimals > 18 => {
                reasons.push("⚠️  Unusual decimals (> 18)".to_string());
                risk_score += 10;
            }
            Ok(0) => {
                reasons.push("⚠️  Zero decimals - unusual for SPL token".to_string());
                risk_score += 5;
            }
            Ok(_) => {
                info!("  ✓ Decimals are normal");
            }
            Err(e) => {
                warn!("  ⚠️  Could not check decimals: {}", e);
                risk_score += 10;
            }
        }

        // Check 5: Holder Distribution
        match self.check_holder_concentration(token_mint).await {
            Ok((top_holder_percent, holder_count)) => {
                info!("  🔍 Top holder owns {:.2}% (total holders: {})", top_holder_percent, holder_count);

                if top_holder_percent > 50.0 {
                    reasons.push(format!("❌ CRITICAL: Top holder owns {:.2}% - extreme concentration", top_holder_percent));
                    risk_score += 40;
                } else if top_holder_percent > 30.0 {
                    reasons.push(format!("⚠️  WARNING: Top holder owns {:.2}% - high concentration", top_holder_percent));
                    risk_score += 25;
                } else if top_holder_percent > 15.0 {
                    reasons.push(format!("⚠️  Top holder owns {:.2}% - moderate concentration", top_holder_percent));
                    risk_score += 15;
                } else {
                    info!("  ✓ Holder distribution looks healthy");
                }

                if holder_count < 10 {
                    reasons.push(format!("⚠️  Very few holders: {}", holder_count));
                    risk_score += 20;
                } else if holder_count < 50 {
                    reasons.push(format!("⚠️  Low holder count: {}", holder_count));
                    risk_score += 10;
                }
            }
            Err(e) => {
                warn!("  ⚠️  Could not check holder distribution: {}", e);
                risk_score += 15;
            }
        }

        let passed = risk_score < 50; // Pass if risk score below 50

        if !passed {
            warn!("❌ Token FAILED safety checks (risk score: {})", risk_score);
        } else if risk_score > 0 {
            warn!("⚠️  Token has some risks (risk score: {})", risk_score);
        } else {
            info!("✅ Token passed all safety checks");
        }

        Ok(TokenSafetyCheck {
            passed,
            reasons,
            risk_score,
        })
    }

    /// Check if mint authority still exists
    async fn check_mint_authority(&self, token_mint: &Pubkey) -> Result<bool> {
        let account = self.solana.get_client().get_account(token_mint).await?;
        let mint = Mint::unpack(&account.data)?;

        Ok(mint.mint_authority.is_some())
    }

    /// Check if freeze authority still exists
    async fn check_freeze_authority(&self, token_mint: &Pubkey) -> Result<bool> {
        let account = self.solana.get_client().get_account(token_mint).await?;
        let mint = Mint::unpack(&account.data)?;

        Ok(mint.freeze_authority.is_some())
    }

    /// Check token supply
    async fn check_supply(&self, token_mint: &Pubkey) -> Result<u64> {
        let account = self.solana.get_client().get_account(token_mint).await?;
        let mint = Mint::unpack(&account.data)?;

        Ok(mint.supply)
    }

    /// Check token decimals
    async fn check_decimals(&self, token_mint: &Pubkey) -> Result<u8> {
        let account = self.solana.get_client().get_account(token_mint).await?;
        let mint = Mint::unpack(&account.data)?;

        Ok(mint.decimals)
    }

    /// Advanced check: Test if token is a honeypot by simulating a sell
    /// This should be called BEFORE buying
    pub async fn check_honeypot(
        &self,
        jupiter: &crate::services::JupiterService,
        token_mint: &str,
        test_amount: u64,
    ) -> Result<bool> {
        use crate::services::jupiter::SOL_MINT;

        info!("🍯 Testing for honeypot...");

        // Try to get a quote for selling the token
        match jupiter.get_quote(token_mint, SOL_MINT, test_amount, 5000).await {
            Ok(quote) => {
                let price_impact: f64 = quote.price_impact_pct.parse().unwrap_or(100.0);

                if price_impact > 50.0 {
                    warn!("  ⚠️  HIGH price impact on sell: {}%", price_impact);
                    Ok(true) // Likely honeypot
                } else {
                    info!("  ✓ Sell quote looks normal ({}% impact)", price_impact);
                    Ok(false)
                }
            }
            Err(e) => {
                warn!("  ❌ Cannot get sell quote - possible honeypot: {}", e);
                Ok(true) // If we can't get a sell quote, assume honeypot
            }
        }
    }

    /// Check holder concentration
    /// Returns (top_holder_percentage, total_holder_count)
    pub async fn check_holder_concentration(&self, token_mint: &Pubkey) -> Result<(f64, usize)> {
        // Get total supply
        let account = self.solana.get_client().get_account(token_mint).await?;
        let mint = Mint::unpack(&account.data)?;
        let total_supply = mint.supply;

        if total_supply == 0 {
            return Err(anyhow!("Token has zero supply"));
        }

        // Get all token accounts for this mint
        let config = RpcProgramAccountsConfig {
            filters: Some(vec![
                // Filter by token account size (165 bytes)
                RpcFilterType::DataSize(165),
                // Filter by mint address at offset 0
                RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                    0,
                    token_mint.to_bytes().to_vec(),
                )),
            ]),
            account_config: RpcAccountInfoConfig {
                encoding: None,  // Encoding not needed for token account fetch
                commitment: Some(CommitmentConfig::confirmed()),
                ..Default::default()
            },
            ..Default::default()
        };

        let accounts = self.solana.get_client()
            .get_program_accounts_with_config(&spl_token::id(), config)
            .await?;

        if accounts.is_empty() {
            return Err(anyhow!("No token accounts found"));
        }

        // Parse balances and find the largest holder
        let mut balances: Vec<u64> = Vec::new();

        for (_pubkey, account) in &accounts {
            if let Ok(token_account) = TokenAccount::unpack(&account.data) {
                if token_account.amount > 0 {
                    balances.push(token_account.amount);
                }
            }
        }

        if balances.is_empty() {
            return Err(anyhow!("No accounts with balance found"));
        }

        // Sort to find top holder
        balances.sort_by(|a, b| b.cmp(a));
        let top_holder_balance = balances[0];
        let top_holder_percent = (top_holder_balance as f64 / total_supply as f64) * 100.0;

        Ok((top_holder_percent, balances.len()))
    }
}
