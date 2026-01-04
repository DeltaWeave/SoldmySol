use anyhow::Result;
use solana_trading_bot::backtest::{Backtester, HistoricalTrade};
use solana_trading_bot::config::Config;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    println!(
        r#"
╔════════════════════════════════════════════════════════════════╗
║                                                                ║
║              SOLANA TRADING BOT - BACKTESTER                   ║
║                                                                ║
╚════════════════════════════════════════════════════════════════╝
    "#
    );

    // Load configuration
    let config = Config::from_env()?;

    // Create backtester
    let mut backtester = Backtester::new(config);

    // Load historical data
    println!("Loading historical trade data...\n");
    let historical_data = load_sample_data();

    // Run backtest
    let results = backtester.run(historical_data)?;

    // Print results
    backtester.print_results(&results);

    Ok(())
}

/// Sample historical data for demonstration
/// In production, this would be loaded from a CSV file or API
fn load_sample_data() -> Vec<HistoricalTrade> {
    vec![
        // Successful trades
        HistoricalTrade {
            timestamp: 1704067200000, // Jan 1, 2024
            token_symbol: "BONK".to_string(),
            entry_price: 0.00001,
            exit_price: 0.00003,
            duration_minutes: 45,
            pnl_percent: 200.0,
            liquidity_sol: 25.0,
            volume_24h: 50000.0,
        },
        HistoricalTrade {
            timestamp: 1704070800000,
            token_symbol: "PEPE".to_string(),
            entry_price: 0.00005,
            exit_price: 0.00012,
            duration_minutes: 120,
            pnl_percent: 140.0,
            liquidity_sol: 30.0,
            volume_24h: 75000.0,
        },
        HistoricalTrade {
            timestamp: 1704074400000,
            token_symbol: "WIF".to_string(),
            entry_price: 0.0001,
            exit_price: 0.00025,
            duration_minutes: 90,
            pnl_percent: 150.0,
            liquidity_sol: 40.0,
            volume_24h: 100000.0,
        },
        // Losing trades
        HistoricalTrade {
            timestamp: 1704078000000,
            token_symbol: "RUG1".to_string(),
            entry_price: 0.0002,
            exit_price: 0.00015,
            duration_minutes: 30,
            pnl_percent: -25.0,
            liquidity_sol: 15.0,
            volume_24h: 20000.0,
        },
        HistoricalTrade {
            timestamp: 1704081600000,
            token_symbol: "DOGE2".to_string(),
            entry_price: 0.00008,
            exit_price: 0.000064,
            duration_minutes: 60,
            pnl_percent: -20.0,
            liquidity_sol: 20.0,
            volume_24h: 30000.0,
        },
        // More successful trades
        HistoricalTrade {
            timestamp: 1704085200000,
            token_symbol: "MOON".to_string(),
            entry_price: 0.00003,
            exit_price: 0.00009,
            duration_minutes: 75,
            pnl_percent: 200.0,
            liquidity_sol: 35.0,
            volume_24h: 60000.0,
        },
        HistoricalTrade {
            timestamp: 1704088800000,
            token_symbol: "ROCKET".to_string(),
            entry_price: 0.00015,
            exit_price: 0.00045,
            duration_minutes: 100,
            pnl_percent: 200.0,
            liquidity_sol: 45.0,
            volume_24h: 80000.0,
        },
        // Marginal trade
        HistoricalTrade {
            timestamp: 1704092400000,
            token_symbol: "SLOW".to_string(),
            entry_price: 0.0001,
            exit_price: 0.00012,
            duration_minutes: 150,
            pnl_percent: 20.0,
            liquidity_sol: 25.0,
            volume_24h: 40000.0,
        },
        // Big winner
        HistoricalTrade {
            timestamp: 1704096000000,
            token_symbol: "GIANT".to_string(),
            entry_price: 0.00002,
            exit_price: 0.00008,
            duration_minutes: 60,
            pnl_percent: 300.0,
            liquidity_sol: 50.0,
            volume_24h: 120000.0,
        },
        // Small loss
        HistoricalTrade {
            timestamp: 1704099600000,
            token_symbol: "FAIL".to_string(),
            entry_price: 0.00012,
            exit_price: 0.0001,
            duration_minutes: 40,
            pnl_percent: -16.67,
            liquidity_sol: 18.0,
            volume_24h: 25000.0,
        },
    ]
}
