use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use std::str::FromStr;
use tracing::{info, warn};

/// Pump.fun program ID
const PUMP_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// Global Pump.fun accounts
const GLOBAL_ACCOUNT: &str = "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf";
const FEE_RECIPIENT: &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWvBzgzy44P9kegTLi";
const ASSOC_TOKEN_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const RENT_PROGRAM: &str = "SysvarRent111111111111111111111111111111111";
const EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";
const BUY_INSTRUCTION_DISCRIMINATOR: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];

pub struct PumpFunSwap {
    rpc_client: RpcClient,
}

impl PumpFunSwap {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_client: RpcClient::new(rpc_url.to_string()),
        }
    }

    /// Execute a buy on Pump.fun bonding curve
    ///
    /// Based on: https://github.com/pump-fun/pump-public-docs/blob/main/docs/PUMP_PROGRAM_README.md
    pub async fn buy_token(
        &self,
        token_mint: &str,
        amount_sol: f64,
        keypair: &Keypair,
        slippage_bps: u16,
    ) -> Result<String> {
        let token_mint_pubkey = Pubkey::from_str(token_mint)?;
        let user_pubkey = keypair.pubkey();

        // Derive bonding curve PDA
        let bonding_curve = self.derive_bonding_curve(&token_mint_pubkey)?;

        // Derive associated bonding curve (holds SOL/tokens)
        let associated_bonding_curve = self.derive_associated_bonding_curve(&token_mint_pubkey)?;

        // Derive user's associated token account
        let user_token_account = spl_associated_token_account::get_associated_token_address(
            &user_pubkey,
            &token_mint_pubkey,
        );

        info!(
            "🎯 Pump.fun Buy: {} SOL of {} (bonding curve: {})",
            amount_sol, token_mint, bonding_curve
        );

        // Build buy instruction
        let ix = self.build_buy_instruction(
            &token_mint_pubkey,
            &bonding_curve,
            &associated_bonding_curve,
            &user_pubkey,
            &user_token_account,
            amount_sol,
            slippage_bps,
        )?;

        // Create and send transaction
        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[ix],
            Some(&user_pubkey),
            &[keypair],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!("✅ Pump.fun buy successful: {}", signature);

        Ok(signature.to_string())
    }

    fn derive_bonding_curve(&self, mint: &Pubkey) -> Result<Pubkey> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"bonding-curve", mint.as_ref()],
            &program_id,
        );
        Ok(pda)
    }

    fn derive_associated_bonding_curve(&self, mint: &Pubkey) -> Result<Pubkey> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let bonding_curve = self.derive_bonding_curve(mint)?;
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"associated-bonding-curve", bonding_curve.as_ref()],
            &program_id,
        );
        Ok(pda)
    }

    fn build_buy_instruction(
        &self,
        mint: &Pubkey,
        bonding_curve: &Pubkey,
        associated_bonding_curve: &Pubkey,
        user: &Pubkey,
        user_token_account: &Pubkey,
        amount_sol: f64,
        slippage_bps: u16,
    ) -> Result<Instruction> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let global = Pubkey::from_str(GLOBAL_ACCOUNT)?;
        let fee_recipient = Pubkey::from_str(FEE_RECIPIENT)?;
        let system_program = Pubkey::from_str(SYSTEM_PROGRAM)?;
        let token_program = Pubkey::from_str(TOKEN_PROGRAM)?;
        let rent = Pubkey::from_str(RENT_PROGRAM)?;
        let event_authority = Pubkey::from_str(EVENT_AUTHORITY)?;

        // Convert SOL to lamports
        let amount_lamports = (amount_sol * 1_000_000_000.0) as u64;

        // Calculate max slippage (simplified - should calculate expected tokens)
        let max_sol_cost = amount_lamports + (amount_lamports * slippage_bps as u64 / 10000);

        // Build instruction data: discriminator + amount + max_sol_cost
        let mut data = Vec::new();
        data.extend_from_slice(&BUY_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&amount_lamports.to_le_bytes());
        data.extend_from_slice(&max_sol_cost.to_le_bytes());

        Ok(Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new_readonly(global, false),
                AccountMeta::new(fee_recipient, false),
                AccountMeta::new(*mint, false),
                AccountMeta::new(*bonding_curve, false),
                AccountMeta::new(*associated_bonding_curve, false),
                AccountMeta::new(*user_token_account, false),
                AccountMeta::new(*user, true), // Signer
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(rent, false),
                AccountMeta::new_readonly(event_authority, false),
                AccountMeta::new_readonly(program_id, false), // Program itself
            ],
            data,
        })
    }
}
