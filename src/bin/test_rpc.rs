use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use std::time::Instant;

fn main() {
    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║             Solana Trading Bot - RPC Tester                   ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // Load environment
    dotenv::dotenv().ok();
    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

    println!("Testing RPC: {}\n", rpc_url);

    let client = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::confirmed());

    // Test 1: Get Version
    print!("1. Getting Solana version... ");
    let start = Instant::now();
    match client.get_version() {
        Ok(version) => {
            let elapsed = start.elapsed();
            println!("✅ OK ({:.0}ms)", elapsed.as_millis());
            println!("   Solana Version: {}", version.solana_core);
        }
        Err(e) => {
            println!("❌ FAILED");
            println!("   Error: {}", e);
            return;
        }
    }

    // Test 2: Get Slot
    print!("\n2. Getting current slot... ");
    let start = Instant::now();
    match client.get_slot() {
        Ok(slot) => {
            let elapsed = start.elapsed();
            println!("✅ OK ({:.0}ms)", elapsed.as_millis());
            println!("   Current Slot: {}", slot);
        }
        Err(e) => {
            println!("❌ FAILED");
            println!("   Error: {}", e);
        }
    }

    // Test 3: Get Recent Blockhash
    print!("\n3. Getting recent blockhash... ");
    let start = Instant::now();
    match client.get_latest_blockhash() {
        Ok(blockhash) => {
            let elapsed = start.elapsed();
            println!("✅ OK ({:.0}ms)", elapsed.as_millis());
            println!("   Blockhash: {}", blockhash);
        }
        Err(e) => {
            println!("❌ FAILED");
            println!("   Error: {}", e);
        }
    }

    // Test 4: Latency Test (5 requests)
    print!("\n4. Testing latency (5 requests)... ");
    let mut latencies = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        if client.get_slot().is_ok() {
            latencies.push(start.elapsed().as_millis());
        }
    }

    if !latencies.is_empty() {
        let avg_latency = latencies.iter().sum::<u128>() / latencies.len() as u128;
        let max_latency = latencies.iter().max().unwrap();
        println!("✅ OK");
        println!("   Average Latency: {}ms", avg_latency);
        println!("   Max Latency: {}ms", max_latency);

        if avg_latency > 500 {
            println!("\n⚠️  WARNING: High latency detected!");
            println!("   Consider using a private RPC (Helius, QuickNode, Triton)");
            println!("   for faster execution and better trading performance.");
        }
    } else {
        println!("❌ FAILED");
    }

    // Test 5: Check if using public RPC
    println!("\n5. RPC Analysis:");
    if rpc_url.contains("api.mainnet-beta.solana.com") {
        println!("   ⚠️  Using PUBLIC RPC");
        println!("   Risk Level: HIGH (MEV vulnerability)");
        println!("   Recommendation: Upgrade to private RPC");
        println!("\n   📝 Quick Setup for Helius (Free Tier):");
        println!("      1. Visit: https://helius.dev");
        println!("      2. Sign up and get API key");
        println!("      3. Update SOLANA_RPC_URL in .env:");
        println!("         SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY");
        println!("      4. Set USE_PRIVATE_RPC=true");
    } else {
        println!("   ✅ Using PRIVATE RPC");
        println!("   Risk Level: LOW");
        println!("   Status: Production ready");
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║                    RPC Test Complete                          ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");
}
