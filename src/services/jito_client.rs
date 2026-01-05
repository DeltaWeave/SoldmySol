// ✅ MEV PROTECTION: Jito Bundle Integration
// Prevents sandwich attacks and frontrunning via Jito block engine

use anyhow::{anyhow, Result};
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use std::sync::Arc;
use tracing::{error, info, warn};
use reqwest::Client;
use serde_json::json;
use base64::Engine;

/// Jito Bundle Client for MEV-protected transactions
pub struct JitoClient {
    enabled: bool,
    block_engine_url: String,
    tip_account: Pubkey,
    min_tip_lamports: u64,
    max_tip_lamports: u64,
    http: Client,
}

#[derive(Debug, Clone)]
pub struct BundleResult {
    pub success: bool,
    pub bundle_id: Option<String>,
    pub signatures: Vec<String>,
    pub tip_paid: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BundleConfig {
    pub tip_lamports: u64,
    pub max_retries: u32,
    pub timeout_ms: u64,
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self {
            tip_lamports: 10_000,      // 0.00001 SOL default tip
            max_retries: 3,
            timeout_ms: 30_000,        // 30 second timeout
        }
    }
}

impl JitoClient {
    /// Create new Jito client from environment configuration
    pub fn new() -> Result<Self> {
        let enabled = std::env::var("JITO_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .parse::<bool>()
            .unwrap_or(true);

        let block_engine_url = std::env::var("JITO_BLOCK_ENGINE_URL")
            .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf".to_string());

        // Jito tip accounts (rotate for load balancing)
        let tip_account = std::env::var("JITO_TIP_ACCOUNT")
            .unwrap_or_else(|_| {
                // Default to one of Jito's official tip accounts
                "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5".to_string()
            })
            .parse::<Pubkey>()
            .map_err(|_| anyhow!("Invalid JITO_TIP_ACCOUNT"))?;

        let min_tip_lamports = std::env::var("JITO_MIN_TIP")
            .unwrap_or_else(|_| "1000".to_string())
            .parse::<u64>()
            .unwrap_or(1_000); // 0.000001 SOL

        let max_tip_lamports = std::env::var("JITO_MAX_TIP")
            .unwrap_or_else(|_| "100000".to_string())
            .parse::<u64>()
            .unwrap_or(100_000); // 0.0001 SOL

        info!("🎯 Jito Client initialized");
        info!("   Enabled: {}", enabled);
        info!("   Block Engine: {}", block_engine_url);
        info!("   Tip Account: {}", tip_account);
        info!("   Tip Range: {} - {} lamports", min_tip_lamports, max_tip_lamports);

        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self {
            enabled,
            block_engine_url,
            tip_account,
            min_tip_lamports,
            max_tip_lamports,
            http,
        })
    }

    /// Send transaction bundle with MEV protection
    pub async fn send_bundle(
        &self,
        transactions: Vec<VersionedTransaction>,
        config: BundleConfig,
    ) -> Result<BundleResult> {
        if !self.enabled {
            return Err(anyhow!("Jito bundles are disabled"));
        }

        // Validate tip amount
        let tip = config.tip_lamports.clamp(self.min_tip_lamports, self.max_tip_lamports);

        info!("📦 Sending Jito bundle:");
        info!("   Transactions: {}", transactions.len());
        info!("   Tip: {} lamports ({:.8} SOL)", tip, tip as f64 / 1e9);

        self.send_bundle_internal(transactions, tip, config).await
    }

    /// Internal bundle submission with retry logic
    async fn send_bundle_internal(
        &self,
        transactions: Vec<VersionedTransaction>,
        tip: u64,
        config: BundleConfig,
    ) -> Result<BundleResult> {
        let mut retries = 0;

        loop {
            match self.try_send_bundle(&transactions, tip).await {
                Ok(result) => {
                    if result.success {
                        info!("✅ Bundle accepted by Jito");
                        info!("   Bundle ID: {}", result.bundle_id.as_ref().unwrap_or(&"N/A".to_string()));
                        info!("   Signatures: {}", result.signatures.len());
                        return Ok(result);
                    } else {
                        warn!("⚠️ Bundle rejected: {}", result.error.as_ref().unwrap_or(&"Unknown".to_string()));
                        retries += 1;
                    }
                }
                Err(e) => {
                    error!("❌ Bundle send failed: {}", e);
                    retries += 1;
                }
            }

            if retries >= config.max_retries {
                return Err(anyhow!("Bundle failed after {} retries", retries));
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        }
    }

    /// Single bundle send attempt
    async fn try_send_bundle(
        &self,
        transactions: &[VersionedTransaction],
        tip: u64,
    ) -> Result<BundleResult> {
        info!("📡 Connecting to Jito block engine: {}", self.block_engine_url);

        let encoded_transactions: Vec<String> = transactions
            .iter()
            .map(|tx| {
                let serialized = bincode::serialize(tx)
                    .map_err(|e| anyhow!("Failed to serialize transaction: {}", e))?;
                Ok(base64::engine::general_purpose::STANDARD.encode(serialized))
            })
            .collect::<Result<Vec<_>>>()?;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [{
                "transactions": encoded_transactions,
                "tip": tip
            }]
        });

        let response = self
            .http
            .post(&self.block_engine_url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| anyhow!("Jito request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Jito block engine error: HTTP {}",
                response.status()
            ));
        }

        let response_json: serde_json::Value = response.json().await?;
        if let Some(error) = response_json.get("error") {
            return Ok(BundleResult {
                success: false,
                bundle_id: None,
                signatures: vec![],
                tip_paid: 0,
                error: Some(error.to_string()),
            });
        }

        let result = response_json.get("result").cloned().unwrap_or_default();
        let bundle_id = result
            .get("bundleId")
            .or_else(|| result.get("bundle_id"))
            .and_then(|value| value.as_str())
            .map(|s| s.to_string());

        let signatures = result
            .get("signatures")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(BundleResult {
            success: true,
            bundle_id,
            signatures,
            tip_paid: tip,
            error: None,
        })
    }

    /// Create tip instruction for bundle
    pub fn create_tip_instruction(
        &self,
        payer: &Pubkey,
        tip_lamports: u64,
    ) -> Instruction {
        solana_sdk::system_instruction::transfer(
            payer,
            &self.tip_account,
            tip_lamports,
        )
    }

    /// Calculate dynamic tip based on transaction value
    pub fn calculate_dynamic_tip(&self, transaction_value_lamports: u64) -> u64 {
        // Tip 0.01% of transaction value, clamped to min/max
        let suggested_tip = transaction_value_lamports / 10_000;
        suggested_tip.clamp(self.min_tip_lamports, self.max_tip_lamports)
    }

    /// Check if Jito is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get Jito tip accounts (for load balancing)
    pub fn get_tip_accounts() -> Vec<Pubkey> {
        // Official Jito tip accounts as of 2024
        vec![
            "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
            "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
            "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
            "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
            "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
            "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
            "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
            "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
        ]
        .iter()
        .filter_map(|s| s.parse::<Pubkey>().ok())
        .collect()
    }

    /// Get random tip account for load balancing
    pub fn get_random_tip_account() -> Pubkey {
        let accounts = Self::get_tip_accounts();
        let index = rand::random::<usize>() % accounts.len();
        accounts[index]
    }
}

/// Helper to build a Jito-protected swap bundle
pub struct JitoBundleBuilder {
    client: Arc<JitoClient>,
    transactions: Vec<VersionedTransaction>,
    tip_lamports: u64,
}

impl JitoBundleBuilder {
    pub fn new(client: Arc<JitoClient>) -> Self {
        Self {
            client,
            transactions: Vec::new(),
            tip_lamports: 10_000, // Default tip
        }
    }

    /// Add transaction to bundle
    pub fn add_transaction(mut self, tx: VersionedTransaction) -> Self {
        self.transactions.push(tx);
        self
    }

    /// Set bundle tip
    pub fn with_tip(mut self, lamports: u64) -> Self {
        self.tip_lamports = lamports;
        self
    }

    /// Calculate tip based on transaction value
    pub fn with_dynamic_tip(mut self, transaction_value_lamports: u64) -> Self {
        self.tip_lamports = self.client.calculate_dynamic_tip(transaction_value_lamports);
        self
    }

    /// Build and send bundle
    pub async fn send(self) -> Result<BundleResult> {
        if self.transactions.is_empty() {
            return Err(anyhow!("Bundle has no transactions"));
        }

        let config = BundleConfig {
            tip_lamports: self.tip_lamports,
            max_retries: 3,
            timeout_ms: 30_000,
        };

        self.client.send_bundle(self.transactions, config).await
    }
}

// Example usage in trading bot:
//
// // Initialize Jito client
// let jito = Arc::new(JitoClient::new()?);
//
// // Build bundle for swap
// let swap_tx = /* create swap transaction */;
// let tip = jito.calculate_dynamic_tip(amount_lamports);
//
// let result = JitoBundleBuilder::new(jito.clone())
//     .add_transaction(swap_tx)
//     .with_tip(tip)
//     .send()
//     .await?;
//
// if result.success {
//     info!("Swap protected from MEV!");
// }

// Need to add rand crate for random selection
// Add to Cargo.toml: rand = "0.8"
