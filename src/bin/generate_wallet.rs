use solana_sdk::signature::{Keypair, Signer};
use bs58;

fn main() {
    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║           Solana Trading Bot - Wallet Generator               ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // Generate a new random keypair
    let keypair = Keypair::new();

    // Get public key
    let pubkey = keypair.pubkey();

    // Get private key as base58 string
    let private_key_bytes = keypair.to_bytes();
    let private_key_base58 = bs58::encode(private_key_bytes).into_string();

    println!("✅ New Wallet Generated!\n");
    println!("Public Key (Wallet Address):");
    println!("  {}\n", pubkey);
    println!("Private Key (Base58 - KEEP THIS SECRET!):");
    println!("  {}\n", private_key_base58);

    println!("⚠️  IMPORTANT SECURITY NOTES:");
    println!("  1. NEVER share your private key with anyone");
    println!("  2. Store it securely (password manager, hardware wallet)");
    println!("  3. This private key controls all funds in this wallet");
    println!("  4. If you lose it, you lose access to your funds forever\n");

    println!("📝 Next Steps:");
    println!("  1. Copy the Private Key above");
    println!("  2. Update PRIVATE_KEY in rust-bot/.env");
    println!("  3. Fund this wallet with SOL for trading");
    println!("  4. Keep a backup of your private key in a safe place\n");

    // Save to a keyfile as well
    let keyfile_path = "wallet.json";
    match std::fs::write(keyfile_path, format!("{:?}", private_key_bytes.to_vec())) {
        Ok(_) => println!("💾 Wallet also saved to: {}", keyfile_path),
        Err(e) => eprintln!("⚠️  Could not save wallet file: {}", e),
    }

    println!();
}
