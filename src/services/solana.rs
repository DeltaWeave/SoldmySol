use anyhow::{anyhow, Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient as AsyncRpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use zeroize::Zeroize;

pub struct SolanaConnection {
    client: AsyncRpcClient,
    wallet: Keypair,
}

impl SolanaConnection {
    pub fn new(rpc_url: &str, private_key: &str) -> Result<Self> {
        let client = AsyncRpcClient::new_with_commitment(
            rpc_url.to_string(),
            CommitmentConfig::confirmed(),
        );

        // Decode private key from base58 - use zeroize for security
        let mut private_key_bytes = bs58::decode(private_key)
            .into_vec()
            .map_err(|_| anyhow!("Invalid private key format"))?;

        let wallet = Keypair::from_bytes(&private_key_bytes)
            .map_err(|_| anyhow!("Invalid keypair bytes"))?;

        // Zero out private key from memory immediately
        private_key_bytes.zeroize();

        info!("✓ Wallet loaded: {}", wallet.pubkey());

        Ok(Self { client, wallet })
    }

    pub async fn initialize(&self) -> Result<bool> {
        // Test connection
        let version = self
            .client
            .get_version()
            .await
            .context("Failed to connect to RPC")?;
        info!("✓ Connected to Solana RPC: {:?}", version);

        // Check balance
        let balance = self.get_balance(None).await?;
        info!("✓ Wallet Balance: {} SOL", balance);

        if balance < 0.01 {
            warn!("⚠️  Low SOL balance! You need SOL for transaction fees.");
        }

        Ok(true)
    }

    pub async fn get_balance(&self, pubkey: Option<&Pubkey>) -> Result<f64> {
        let default_key = self.wallet.pubkey();
        let key = pubkey.unwrap_or(&default_key);
        let lamports = self
            .client
            .get_balance(key)
            .await
            .context("Failed to get balance")?;
        Ok(lamports as f64 / 1_000_000_000.0)
    }

    pub async fn get_token_balance(&self, token_mint: &Pubkey) -> Result<f64> {
        let (amount, decimals) = self.get_token_balance_raw(token_mint).await?;

        Ok(amount as f64 / 10_f64.powi(decimals as i32))
    }

    pub async fn get_token_balance_raw(&self, token_mint: &Pubkey) -> Result<(u64, u8)> {
        // ✅ FIX #2: Correct offsets for token account filters
        let filters = vec![
            RpcFilterType::DataSize(165),
            RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                32,  // ✅ FIXED: Owner is at offset 32
                self.wallet.pubkey().to_bytes().to_vec(),
            )),
            RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                0,   // ✅ Mint is at offset 0
                token_mint.to_bytes().to_vec(),
            )),
        ];

        let accounts = self
            .client
            .get_program_accounts_with_config(
                &spl_token::id(),
                solana_client::rpc_config::RpcProgramAccountsConfig {
                    filters: Some(filters),
                    ..Default::default()
                },
            )
            .await
            .context("Failed to get token accounts")?;

        if accounts.is_empty() {
            let mint_account = self.client.get_account(token_mint).await?;
            let mint = spl_token::state::Mint::unpack(&mint_account.data)?;
            return Ok((0, mint.decimals));
        }

        // Parse first account
        let account_data = &accounts[0].1.data;
        let token_account = spl_token::state::Account::unpack(account_data)
            .context("Failed to unpack token account")?;

        // Get mint to determine decimals
        let mint_account = self.client.get_account(token_mint).await?;
        let mint = spl_token::state::Mint::unpack(&mint_account.data)?;

        Ok((token_account.amount, mint.decimals))
    }

    /// Get holder count and top holder percentage for a token mint.
    pub async fn get_token_holder_stats(&self, token_mint: &Pubkey) -> Result<(usize, f64)> {
        use spl_token::state::Account as TokenAccount;

        let config = RpcProgramAccountsConfig {
            filters: Some(vec![
                RpcFilterType::DataSize(165),
                RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
                    0,
                    token_mint.to_bytes().to_vec(),
                )),
            ]),
            account_config: RpcAccountInfoConfig {
                encoding: None,
                commitment: Some(CommitmentConfig::confirmed()),
                ..Default::default()
            },
            ..Default::default()
        };

        let accounts = self
            .client
            .get_program_accounts_with_config(&spl_token::id(), config)
            .await
            .context("Failed to fetch token accounts")?;

        let holder_count = accounts
            .iter()
            .filter_map(|(_, account)| TokenAccount::unpack(&account.data).ok())
            .filter(|account| account.amount > 0)
            .count();

        let supply = self
            .client
            .get_token_supply(token_mint)
            .await
            .context("Failed to fetch token supply")?;
        let supply_amount = supply.amount.parse::<u64>().unwrap_or(0);

        let largest_accounts = self
            .client
            .get_token_largest_accounts(token_mint)
            .await
            .context("Failed to fetch largest accounts")?;
        let top_amount = largest_accounts
            .first()
            .and_then(|account| account.amount.amount.parse::<u64>().ok())
            .unwrap_or(0);

        let top_holder_percent = if supply_amount > 0 {
            (top_amount as f64 / supply_amount as f64) * 100.0
        } else {
            0.0
        };

        Ok((holder_count, top_holder_percent))
    }

    pub async fn check_health(&self) -> Result<HealthCheck> {
        let start = Instant::now();
        let slot = self.client.get_slot().await.context("Failed to get slot")?;
        let latency = start.elapsed().as_millis() as u64;

        Ok(HealthCheck {
            healthy: true,
            slot,
            latency,
        })
    }

    pub async fn wait_for_confirmation(
        &self,
        signature: &Signature,
        max_retries: u32,
    ) -> Result<bool> {
        for i in 0..max_retries {
            match self.client.get_signature_status(signature).await {
                Ok(Some(status)) => {
                    if let Err(e) = status {
                        error!("Transaction failed: {:?}", e);
                        return Ok(false);
                    }
                    info!("✓ Transaction confirmed: {}", signature);
                    return Ok(true);
                }
                Ok(None) => {
                    if i % 5 == 0 {
                        info!("⏳ Waiting for confirmation... ({}/{})", i + 1, max_retries);
                    }
                }
                Err(e) => {
                    warn!("⚠️  Error checking signature status: {}", e);
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        error!("❌ Transaction confirmation timeout");
        Ok(false)
    }

    pub async fn send_and_confirm_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<Signature> {
        let signature = self
            .client
            .send_and_confirm_transaction(transaction)
            .await
            .context("Failed to send transaction")?;

        Ok(signature)
    }

    pub fn get_client(&self) -> &AsyncRpcClient {
        &self.client
    }

    pub fn get_wallet(&self) -> &Keypair {
        &self.wallet
    }

    pub fn pubkey(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    /// Send SOL to another wallet (for profit sweeps)
    pub async fn send_sol(&self, to: &Pubkey, lamports: u64) -> Result<Signature> {
        use solana_sdk::system_instruction;

        info!("Sending {} lamports to {}", lamports, to);

        let instruction = system_instruction::transfer(
            &self.wallet.pubkey(),
            to,
            lamports,
        );

        let recent_blockhash = self.client
            .get_latest_blockhash()
            .await
            .context("Failed to get recent blockhash")?;

        let transaction = solana_sdk::transaction::Transaction::new_signed_with_payer(
            &[instruction],
            Some(&self.wallet.pubkey()),
            &[&self.wallet],
            recent_blockhash,
        );

        let signature = self.client
            .send_and_confirm_transaction(&transaction)
            .await
            .context("Failed to send SOL transfer")?;

        info!("✓ SOL transfer confirmed: {}", signature);

        Ok(signature)
    }

    pub async fn estimate_priority_fee(&self, default_fee: u64) -> u64 {
        // ✅ FIX #7: Made async
        match self.client.get_recent_prioritization_fees(&[]).await {
            Ok(fees) if !fees.is_empty() => {
                let avg_fee: u64 = fees.iter().map(|f| f.prioritization_fee).sum::<u64>()
                    / fees.len() as u64;
                // Add 50% buffer for safety
                (avg_fee as f64 * 1.5) as u64
            }
            _ => default_fee,
        }
    }

    /// Estimate total transaction cost including rent, fees, and priority
    pub async fn estimate_transaction_cost(&self, _snipe_amount_sol: f64) -> Result<f64> {
        let priority_fee = self.estimate_priority_fee(50000).await;
        let base_fee = 5000; // 0.000005 SOL base transaction fee
        let rent_exempt = 2039280; // ~0.002 SOL for token account rent

        let total_lamports = priority_fee + base_fee + rent_exempt;
        let total_sol = total_lamports as f64 / 1_000_000_000.0;

        Ok(total_sol)
    }

    /// Get account info for a given public key
    pub async fn get_account_info(&self, pubkey: &Pubkey) -> Result<Option<solana_sdk::account::Account>> {
        match self.client.get_account(pubkey).await {
            Ok(account) => Ok(Some(account)),
            Err(e) => {
                // Check if it's a "not found" error
                if e.to_string().contains("AccountNotFound") {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("Failed to get account info: {:?}", e))
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HealthCheck {
    pub healthy: bool,
    pub slot: u64,
    pub latency: u64,
}
