use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{Keypair, Signer},
};
use bs58;

fn main() {
    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║          Solana Trading Bot - Balance Checker                 ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // Load environment
    dotenv::dotenv().ok();

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .expect("SOLANA_RPC_URL not set in .env");
    let private_key_str = std::env::var("PRIVATE_KEY")
        .expect("PRIVATE_KEY not set in .env");

    // Decode private key
    let private_key_bytes = bs58::decode(&private_key_str)
        .into_vec()
        .expect("Invalid private key format");
    let keypair = Keypair::from_bytes(&private_key_bytes)
        .expect("Failed to create keypair from private key");

    let pubkey = keypair.pubkey();

    println!("Wallet Address: {}", pubkey);
    println!("RPC Endpoint: {}\n", if rpc_url.contains("helius") { "Helius (Private)" } else { "Public" });

    // Connect to RPC
    let client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());

    // Get balance
    print!("Checking balance... ");
    match client.get_balance(&pubkey) {
        Ok(lamports) => {
            let sol_balance = lamports as f64 / 1_000_000_000.0;
            println!("✅\n");
            println!("╔════════════════════════════════════════════════════════════════╗");
            println!("║  Balance: {:.9} SOL ({} lamports)", sol_balance, lamports);
            println!("╚════════════════════════════════════════════════════════════════╝\n");

            // Check if ready for trading
            if sol_balance >= 1.0 {
                println!("✅ Wallet is funded and ready for trading!");
                println!("   - Trading Capital: ~{:.4} SOL", sol_balance - 0.01);
                println!("   - Reserved for Fees: ~0.01 SOL\n");

                println!("🚀 Ready to deploy bot!");
                println!("   Run: cargo run --release\n");
            } else if sol_balance > 0.0 {
                println!("⚠️  Wallet has funds but less than 1 SOL");
                println!("   Current: {:.9} SOL", sol_balance);
                println!("   Needed: 1.00 SOL minimum\n");
                println!("📝 To fund your wallet:");
                println!("   1. Send SOL to: {}", pubkey);
                println!("   2. Use a CEX (Coinbase, Binance) to withdraw SOL");
                println!("   3. Or use Phantom/Solflare wallet to send\n");
            } else {
                println!("❌ Wallet has ZERO balance!");
                println!("\n📝 To fund your wallet:");
                println!("   1. Copy this address: {}", pubkey);
                println!("   2. Send at least 1.0 SOL from:");
                println!("      - Centralized exchange (Coinbase, Binance, etc.)");
                println!("      - Another Solana wallet (Phantom, Solflare)");
                println!("      - DEX or on-ramp service\n");
                println!("   ⚠️  IMPORTANT: Send on Solana network only!");
                println!("       Do NOT send from Ethereum or other chains.\n");
            }

            // Show network fees info
            println!("💡 Network Fee Info:");
            println!("   - Transaction fee: ~0.000005 SOL");
            println!("   - Priority fee (configured): 0.00005 SOL");
            println!("   - Estimated per-trade cost: ~0.0001 SOL");
            println!("   - With 1 SOL, you can make ~10,000 trades\n");

        }
        Err(e) => {
            println!("❌ FAILED\n");
            println!("Error: {}", e);
            println!("\nPossible issues:");
            println!("  1. RPC endpoint not responding");
            println!("  2. Network connectivity issues");
            println!("  3. Invalid wallet configuration\n");
        }
    }
}
