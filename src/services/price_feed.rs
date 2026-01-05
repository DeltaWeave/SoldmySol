use crate::models::TokenPool;
use anyhow::{Context, Result};
use governor::{Quota, RateLimiter};
use nonzero_ext::nonzero;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DexScreenerPair {
    #[serde(rename = "chainId")]
    chain_id: String,
    #[serde(rename = "dexId")]
    dex_id: String,
    #[serde(rename = "pairAddress")]
    pair_address: String,
    #[serde(rename = "baseToken")]
    base_token: BaseToken,
    #[serde(rename = "priceUsd")]
    price_usd: Option<String>,
    liquidity: Option<Liquidity>,
    volume: Option<Volume>,
    #[serde(rename = "priceChange")]
    price_change: Option<PriceChange>,
    #[serde(rename = "pairCreatedAt")]
    pair_created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaseToken {
    address: String,
    symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Liquidity {
    usd: Option<f64>,
    base: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Volume {
    h1: Option<f64>,
    h6: Option<f64>,
    h24: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PriceChange {
    h24: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DexScreenerResponse {
    pairs: Option<Vec<DexScreenerPair>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BoostedToken {
    amount: Option<i64>,
    #[serde(rename = "chainId")]
    chain_id: Option<String>,
    description: Option<String>,
    header: Option<String>,
    icon: Option<String>,
    links: Option<Vec<HashMap<String, String>>>,
    #[serde(rename = "tokenAddress")]
    token_address: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BirdeyeNewListing {
    address: String,
    symbol: Option<String>,
    #[serde(rename = "listingTime")]
    listing_time: Option<i64>,
    liquidity: Option<f64>,
    #[serde(rename = "v24hUSD")]
    volume_24h: Option<f64>,
    #[serde(rename = "priceChange24h")]
    price_change_24h: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BirdeyeResponse {
    data: Option<BirdeyeData>,
    success: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BirdeyeData {
    tokens: Option<Vec<BirdeyeNewListing>>,
}

pub struct PriceFeed {
    client: Client,
    rate_limiter: RateLimiter<
        governor::state::direct::NotKeyed,
        governor::state::InMemoryState,
        governor::clock::DefaultClock,
    >,
    search_index: std::sync::atomic::AtomicUsize,
}

impl PriceFeed {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client for PriceFeed")?;

        // Rate limit: 20 requests per minute for DexScreener
        let quota = Quota::per_minute(nonzero!(20u32));
        let rate_limiter = RateLimiter::direct(quota);

        Ok(Self {
            client,
            rate_limiter,
            search_index: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub async fn get_new_pools(&self) -> Result<Vec<TokenPool>> {
        // Multi-source token discovery strategy (prioritized)
        // NOTE: Real-time WebSocket monitoring (program_monitor.rs) is PRIMARY discovery method
        // This API polling is FALLBACK only for tokens that weren't caught in real-time
        // 1. DexScreener boosted tokens (projects paying for promotion)
        // 2. Rotating search queries (diverse discovery)

        let mut all_pools = Vec::new();

        // Strategy 1: Get boosted tokens (often new launches)
        if let Ok(boosted_pools) = self.get_boosted_tokens().await {
            all_pools.extend(boosted_pools);
        }

        // Strategy 2: Rotating search queries to discover different tokens
        if all_pools.len() < 10 {
            if let Ok(search_pools) = self.get_pools_by_search().await {
                all_pools.extend(search_pools);
            }
        }

        // Sort by creation time (newest first) and remove duplicates
        all_pools.sort_by(|a, b| {
            let a_time = a.created_at.unwrap_or(0);
            let b_time = b.created_at.unwrap_or(0);
            b_time.cmp(&a_time)
        });

        // Deduplicate by token address
        let mut seen = std::collections::HashSet::new();
        all_pools.retain(|pool| seen.insert(pool.token_address.clone()));

        debug!("Fetched {} new pools from multiple sources", all_pools.len());
        Ok(all_pools.into_iter().take(20).collect())
    }

    async fn get_birdeye_new_listings(&self) -> Result<Vec<TokenPool>> {
        // Check if Birdeye API key is configured
        let api_key = match env::var("BIRDEYE_API_KEY") {
            Ok(key) if !key.is_empty() => key,
            _ => {
                debug!("Birdeye API key not configured, skipping");
                return Ok(Vec::new());
            }
        };

        // Wait for rate limiter
        self.rate_limiter.until_ready().await;

        let url = "https://public-api.birdeye.so/defi/token_new_listing?chain=solana&limit=20&offset=0";

        let response = self
            .client
            .get(url)
            .header("X-API-KEY", api_key)
            .send()
            .await
            .context("Failed to fetch Birdeye new listings")?;

        if !response.status().is_success() {
            warn!("Birdeye API error: {}", response.status());
            return Ok(Vec::new());
        }

        let birdeye_data: BirdeyeResponse = response.json().await?;

        let mut pools = Vec::new();

        if let Some(data) = birdeye_data.data {
            if let Some(tokens) = data.tokens {
                for token in tokens.iter().take(20) {
                    // Fetch full details from DexScreener for this token
                    if let Ok(Some(pool)) = self.get_token_info(&token.address).await {
                        pools.push(pool);
                    }
                }
            }
        }

        info!("Birdeye API returned {} new Solana listings", pools.len());
        Ok(pools)
    }

    async fn get_boosted_tokens(&self) -> Result<Vec<TokenPool>> {
        // Wait for rate limiter
        self.rate_limiter.until_ready().await;

        let url = "https://api.dexscreener.com/token-boosts/latest/v1";

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch boosted tokens")?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let boosted: Vec<BoostedToken> = response.json().await?;

        let mut pools = Vec::new();

        // Fetch details for each boosted Solana token
        for token in boosted.iter().take(5) {
            if let Some(chain) = &token.chain_id {
                if chain == "solana" {
                    if let Some(address) = &token.token_address {
                        // Fetch full token info
                        if let Ok(Some(pool)) = self.get_token_info(address).await {
                            pools.push(pool);
                        }
                    }
                }
            }
        }

        debug!("Found {} boosted Solana tokens", pools.len());
        Ok(pools)
    }

    async fn get_pools_by_search(&self) -> Result<Vec<TokenPool>> {
        // Wait for rate limiter
        self.rate_limiter.until_ready().await;

        // Rotate through different search queries to discover diverse tokens
        let search_queries = [
            "meme",
            "ai",
            "dog",
            "cat",
            "pepe",
            "wojak",
            "moon",
            "doge",
        ];

        let index = self.search_index.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let query = search_queries[index % search_queries.len()];

        let url = format!("https://api.dexscreener.com/latest/dex/search?q={}", query);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch pools from DexScreener")?;

        if !response.status().is_success() {
            warn!("DexScreener search API error: {}", response.status());
            return Ok(Vec::new());
        }

        let data: DexScreenerResponse = response
            .json()
            .await
            .context("Failed to parse DexScreener response")?;

        let mut pools = Vec::new();
        let accepted_dexes = ["raydium", "pumpswap", "meteora", "orca", "jupiter", "launchlab"];

        if let Some(pairs) = data.pairs {
            for pair in pairs.iter().take(20) {
                if pair.chain_id == "solana"
                    && accepted_dexes.contains(&pair.dex_id.as_str()) {
                    let pool = TokenPool {
                        token_address: pair.base_token.address.clone(),
                        token_symbol: pair.base_token.symbol.clone(),
                        pair_address: pair.pair_address.clone(),
                        liquidity_usd: pair.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0),
                        liquidity_sol: pair.liquidity.as_ref().and_then(|l| l.base).unwrap_or(0.0),
                        price_usd: pair
                            .price_usd
                            .as_ref()
                            .and_then(|p| p.parse().ok())
                            .unwrap_or(0.0),
                        volume_1h: pair.volume.as_ref().and_then(|v| v.h1).unwrap_or(0.0),
                        volume_6h: pair.volume.as_ref().and_then(|v| v.h6).unwrap_or(0.0),
                        volume_24h: pair.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0),
                        price_change_24h: pair
                            .price_change
                            .as_ref()
                            .and_then(|pc| pc.h24)
                            .unwrap_or(0.0),
                        created_at: pair.pair_created_at,
                    };

                    pools.push(pool);
                }
            }
        }

        debug!("Search '{}' found {} Solana pools", query, pools.len());
        Ok(pools)
    }

    pub async fn get_token_price(&self, token_address: &str) -> Result<f64> {
        let url = format!(
            "https://api.dexscreener.com/latest/dex/tokens/{}",
            token_address
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch token price")?;

        if !response.status().is_success() {
            return Ok(0.0);
        }

        let data: DexScreenerResponse = response.json().await?;

        if let Some(pairs) = data.pairs {
            if let Some(pair) = pairs.first() {
                if let Some(price_str) = &pair.price_usd {
                    return Ok(price_str.parse().unwrap_or(0.0));
                }
            }
        }

        Ok(0.0)
    }

    pub async fn get_token_info(&self, token_address: &str) -> Result<Option<TokenPool>> {
        let url = format!(
            "https://api.dexscreener.com/latest/dex/tokens/{}",
            token_address
        );

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let data: DexScreenerResponse = response.json().await?;

        if let Some(pairs) = data.pairs {
            let accepted_dexes = ["raydium", "pumpswap", "meteora", "orca", "jupiter", "launchlab"];

            if let Some(pair) = pairs
                .iter()
                .find(|p| p.chain_id == "solana" && accepted_dexes.contains(&p.dex_id.as_str()))
            {
                return Ok(Some(TokenPool {
                    token_address: pair.base_token.address.clone(),
                    token_symbol: pair.base_token.symbol.clone(),
                    pair_address: pair.pair_address.clone(),
                    liquidity_usd: pair.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0),
                    liquidity_sol: pair.liquidity.as_ref().and_then(|l| l.base).unwrap_or(0.0),
                    price_usd: pair
                        .price_usd
                        .as_ref()
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(0.0),
                    volume_1h: pair.volume.as_ref().and_then(|v| v.h1).unwrap_or(0.0),
                    volume_6h: pair.volume.as_ref().and_then(|v| v.h6).unwrap_or(0.0),
                    volume_24h: pair.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0),
                    price_change_24h: pair
                        .price_change
                        .as_ref()
                        .and_then(|pc| pc.h24)
                        .unwrap_or(0.0),
                    created_at: pair.pair_created_at,
                }));
            }
        }

        Ok(None)
    }

    // Get current price using Jupiter quote (more accurate for actual trading)
    pub async fn get_token_price_jupiter(
        &self,
        jupiter: &crate::services::jupiter::JupiterService,
        token_mint: &str,
    ) -> Result<f64> {
        jupiter.get_token_price(token_mint).await
    }

    /// Fetch pool info directly from on-chain (used when DexScreener hasn't indexed yet)
    /// This is faster and more reliable for newly created pools
    ///
    /// For new pools, we retry with delays: immediate, +3s, +7s, +15s (total 25s)
    /// This gives time for liquidity to be added and indexed by Jupiter
    pub async fn get_token_info_onchain(
        &self,
        pool_address: &str,
        program_name: &str,
        solana: &crate::services::solana::SolanaConnection,
        jupiter: &crate::services::jupiter::JupiterService,
    ) -> Result<Option<TokenPool>> {
        use solana_sdk::pubkey::Pubkey;
        use std::str::FromStr;

        info!("📡 Fetching on-chain data for pool: {} ({})", pool_address, program_name);

        // Parse pool address
        let pool_pubkey = match Pubkey::from_str(pool_address) {
            Ok(pk) => pk,
            Err(e) => {
                warn!("Invalid pool address {}: {:?}", pool_address, e);
                return Ok(None);
            }
        };

        // Get account info from Solana
        let account_info = match solana.get_account_info(&pool_pubkey).await {
            Ok(Some(account)) => account,
            Ok(None) => {
                warn!("Pool account {} not found on-chain", pool_address);
                return Ok(None);
            }
            Err(e) => {
                warn!("Error fetching pool account {}: {:?}", pool_address, e);
                return Ok(None);
            }
        };

        // Extract token mint from pool account data based on program type
        let token_mint = self.extract_token_mint_from_pool(&account_info.data, program_name)?;

        if token_mint.is_none() {
            warn!("Could not extract token mint from pool {}", pool_address);
            return Ok(None);
        }

        let token_mint = token_mint.unwrap();
        info!("🔍 Extracted token mint: {} from pool {}", token_mint, pool_address);

        // Retry strategy: Try immediately, then with increasing delays
        let retry_delays = vec![0, 3, 7, 15]; // seconds
        let sol_mint = "So11111111111111111111111111111111111111112";

        for (attempt, delay_secs) in retry_delays.iter().enumerate() {
            if *delay_secs > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(*delay_secs)).await;
                info!("🔄 Retry {} for token {} (waited {}s)", attempt + 1, token_mint, delay_secs);
            }

            // Try to get a quote to verify token is tradeable
            // Swap 0.01 SOL worth to test
            match jupiter.get_quote(sol_mint, &token_mint, 10_000_000, 500).await {
                Ok(quote) => {
                    info!("✅ Token {} is tradeable via Jupiter (attempt {})", token_mint, attempt + 1);

                    // Calculate price from quote
                    let out_amount: u64 = quote.out_amount.parse().unwrap_or(0);
                    let in_amount: u64 = quote.in_amount.parse().unwrap_or(10_000_000);
                    let price_per_token_sol = if out_amount > 0 {
                        in_amount as f64 / out_amount as f64
                    } else {
                        0.0
                    };
                    let price_usd = price_per_token_sol * 150.0; // Rough SOL price

                    // Create TokenPool with real data
                    return Ok(Some(TokenPool {
                        token_address: token_mint.clone(),
                        token_symbol: "UNKNOWN".to_string(), // Will be updated from metadata later
                        pair_address: pool_address.to_string(),
                        liquidity_usd: 0.0, // Not easily calculable without pool parsing
                        liquidity_sol: account_info.lamports as f64 / 1_000_000_000.0,
                        price_usd,
                        volume_1h: 0.0,
                        volume_6h: 0.0,
                        volume_24h: 0.0,
                        price_change_24h: 0.0,
                        created_at: Some(chrono::Utc::now().timestamp_millis()),
                    }));
                }
                Err(e) => {
                    if attempt < retry_delays.len() - 1 {
                        debug!("⏳ Token {} not tradeable yet (attempt {}): {:?}", token_mint, attempt + 1, e);
                    } else {
                        warn!("❌ Token {} not tradeable after {} attempts", token_mint, retry_delays.len());
                    }
                }
            }
        }

        Ok(None)
    }

    /// Extract token mint address from pool account data
    fn extract_token_mint_from_pool(&self, data: &[u8], program_name: &str) -> Result<Option<String>> {
        use solana_sdk::pubkey::Pubkey;

        const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

        // For Pump.fun bonding curves, token mint is at offset 8 (after discriminator)
        if program_name == "PumpFun" && data.len() >= 40 {
            let mint_bytes = &data[8..40];
            if let Ok(mint) = Pubkey::try_from(mint_bytes) {
                return Ok(Some(mint.to_string()));
            }
        }

        if program_name.contains("RaydiumCLMM") && data.len() >= 136 {
            let mint_a = Pubkey::new_from_array(data[8..40].try_into().unwrap_or([0u8; 32]));
            let mint_b = Pubkey::new_from_array(data[40..72].try_into().unwrap_or([0u8; 32]));

            let candidate = if mint_a.to_string() == WSOL_MINT {
                mint_b
            } else {
                mint_a
            };
            return Ok(Some(candidate.to_string()));
        }

        if program_name.contains("OrcaWhirlpool") && data.len() >= 136 {
            let mint_a = Pubkey::new_from_array(data[40..72].try_into().unwrap_or([0u8; 32]));
            let mint_b = Pubkey::new_from_array(data[72..104].try_into().unwrap_or([0u8; 32]));

            let candidate = if mint_a.to_string() == WSOL_MINT {
                mint_b
            } else {
                mint_a
            };
            return Ok(Some(candidate.to_string()));
        }

        if program_name.contains("Raydium") && data.len() >= 432 {
            let base_mint = Pubkey::new_from_array(data[400..432].try_into().unwrap_or([0u8; 32]));
            if base_mint.to_string() != WSOL_MINT {
                return Ok(Some(base_mint.to_string()));
            }
        }

        Ok(None)
    }
}
