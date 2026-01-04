pub mod backtest;
pub mod config;
pub mod ml;
pub mod models;
pub mod services;
pub mod strategies;
pub mod monitoring;

use anyhow::{Context, Result};
use config::Config;
use services::{Database, JupiterService, PriceFeed, PumpFunSwap, SolanaConnection};
use strategies::HybridStrategy;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    print_banner();

    // Load configuration
    info!("Loading configuration...");
    let mut config = Config::from_env()?;

    // Initialize services
    info!("Initializing services...");

    let solana = Arc::new(SolanaConnection::new(&config.rpc.url, &config.wallet.private_key)?);
    solana.initialize().await?;

    let jupiter = Arc::new(JupiterService::new()
        .context("Failed to initialize Jupiter service")?);
    let price_feed = Arc::new(PriceFeed::new()
        .context("Failed to initialize price feed service")?);
    let pumpfun = Arc::new(PumpFunSwap::new(&config.rpc.url));

    // Load capital from database
    let db = Arc::new(Database::new(&config.database.path)?);
    config.capital.current = db.get_current_capital(config.capital.initial)?;

    info!("Bot Configuration:");
    info!("  Initial Capital: {} SOL", config.capital.initial);
    info!("  Current Capital: {} SOL", config.capital.current);
    info!("  Target Capital: {} SOL", config.capital.target);
    info!("  Stop Loss: {:.0}%", config.risk.stop_loss_percent * 100.0);
    info!(
        "  Take Profit: {:.0}%",
        config.risk.take_profit_percent * 100.0
    );
    info!(
        "  Max Position Size: {:.0}% of capital",
        config.sniper.snipe_amount_sol / config.capital.current * 100.0
    );
    info!(
        "  Max Concurrent Positions: {}",
        config.risk.max_concurrent_positions
    );

    // Health check
    perform_health_check(&solana).await?;

    // Start hybrid trading strategy
    let hybrid = HybridStrategy::new(config, solana.clone(), jupiter, price_feed, pumpfun, db);

    // Setup graceful shutdown
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!("Failed to listen for ctrl+c signal: {}", e);
            std::process::exit(1);
        }

        info!("\nReceived interrupt signal, shutting down...");
        std::process::exit(0);
    });

    // Start the hybrid strategy (this blocks until error or shutdown)
    if let Err(e) = hybrid.start().await {
        error!("Fatal error: {}", e);
        return Err(e);
    }

    Ok(())
}

fn print_banner() {
    println!(
        r#"
╔════════════════════════════════════════════════════════════════╗
║                                                                ║
║           🚀 SOLANA TRADING BOT - RUST EDITION 🚀              ║
║                                                                ║
║              Goal: 100 SOL → 10,000 SOL (6 months)             ║
║                                                                ║
╚════════════════════════════════════════════════════════════════╝
    "#
    );
}

async fn perform_health_check(solana: &Arc<SolanaConnection>) -> Result<()> {
    info!("\nSystem Health Check:");

    let health = solana.check_health().await?;
    info!("  Status: {} Healthy", if health.healthy { "✓" } else { "✗" });
    info!("  Latency: {}ms", health.latency);
    info!("  Current Slot: {}", health.slot);

    let balance = solana.get_balance(None).await?;
    info!("  Wallet Balance: {:.4} SOL", balance);

    if balance < 0.1 {
        error!("  WARNING: Low balance! Add SOL for trading.");
        return Err(anyhow::anyhow!("Insufficient balance"));
    }

    info!("✓ Initialization complete!\n");

    Ok(())
}
