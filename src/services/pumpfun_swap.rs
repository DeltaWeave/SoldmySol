use anyhow::{anyhow, Result};
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
const EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";
const FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";
const FEE_CONFIG_SEED: [u8; 32] = [
    1, 86, 224, 246, 147, 102, 90, 207, 68, 219, 21, 104, 191, 23, 91, 170,
    81, 137, 203, 151, 245, 210, 255, 59, 101, 93, 43, 182, 253, 109, 24, 176,
];
const BUY_INSTRUCTION_DISCRIMINATOR: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];
const SELL_INSTRUCTION_DISCRIMINATOR: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];

#[derive(Debug, Clone)]
struct BondingCurveState {
    virtual_token_reserves: u64,
    virtual_sol_reserves: u64,
    real_token_reserves: u64,
    real_sol_reserves: u64,
    token_total_supply: u64,
    complete: bool,
    creator: Pubkey,
    is_mayhem_mode: bool,
}

#[derive(Debug, Clone)]
pub struct BondingCurveSnapshot {
    pub real_sol_reserves: f64,
    pub virtual_sol_reserves: f64,
    pub real_token_reserves: u64,
    pub virtual_token_reserves: u64,
    pub is_complete: bool,
    pub creator: Pubkey,
}

pub struct PumpFunSwap {
    rpc_client: RpcClient,
}

impl PumpFunSwap {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_client: RpcClient::new(rpc_url.to_string()),
        }
    }

    /// Fetch bonding curve snapshot for validation and risk checks.
    pub fn get_curve_snapshot(&self, token_mint: &str) -> Result<BondingCurveSnapshot> {
        let token_mint_pubkey = Pubkey::from_str(token_mint)?;
        let bonding_curve = self.derive_bonding_curve(&token_mint_pubkey)?;
        let state = self.fetch_bonding_curve_state(&bonding_curve)?;

        Ok(BondingCurveSnapshot {
            real_sol_reserves: state.real_sol_reserves as f64 / 1_000_000_000.0,
            virtual_sol_reserves: state.virtual_sol_reserves as f64 / 1_000_000_000.0,
            real_token_reserves: state.real_token_reserves,
            virtual_token_reserves: state.virtual_token_reserves,
            is_complete: state.complete,
            creator: state.creator,
        })
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

        // Derive associated bonding curve token account (ATA for bonding curve PDA)
        let associated_bonding_curve = spl_associated_token_account::get_associated_token_address(
            &bonding_curve,
            &token_mint_pubkey,
        );

        // Derive user's associated token account
        let user_token_account = spl_associated_token_account::get_associated_token_address(
            &user_pubkey,
            &token_mint_pubkey,
        );

        let bonding_curve_state = self.fetch_bonding_curve_state(&bonding_curve)?;
        let creator_vault = self.derive_creator_vault(&bonding_curve_state.creator)?;
        let global_volume_accumulator = self.derive_global_volume_accumulator()?;
        let user_volume_accumulator = self.derive_user_volume_accumulator(&user_pubkey)?;
        let fee_config = self.derive_fee_config()?;

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
            &creator_vault,
            &global_volume_accumulator,
            &user_volume_accumulator,
            &fee_config,
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

    /// Execute a sell on Pump.fun bonding curve
    pub async fn sell_token(
        &self,
        token_mint: &str,
        amount_tokens: u64,
        keypair: &Keypair,
        slippage_bps: u16,
    ) -> Result<String> {
        let token_mint_pubkey = Pubkey::from_str(token_mint)?;
        let user_pubkey = keypair.pubkey();

        let bonding_curve = self.derive_bonding_curve(&token_mint_pubkey)?;
        let associated_bonding_curve = spl_associated_token_account::get_associated_token_address(
            &bonding_curve,
            &token_mint_pubkey,
        );
        let user_token_account = spl_associated_token_account::get_associated_token_address(
            &user_pubkey,
            &token_mint_pubkey,
        );

        let bonding_curve_state = self.fetch_bonding_curve_state(&bonding_curve)?;
        let creator_vault = self.derive_creator_vault(&bonding_curve_state.creator)?;
        let fee_config = self.derive_fee_config()?;

        let expected_output = self.estimate_sell_output(&bonding_curve_state, amount_tokens);
        let min_sol_output = expected_output
            .saturating_sub(expected_output.saturating_mul(slippage_bps as u64) / 10_000);

        info!(
            "🔻 Pump.fun Sell: {} tokens of {} (min SOL: {})",
            amount_tokens, token_mint, min_sol_output
        );

        let ix = self.build_sell_instruction(
            &token_mint_pubkey,
            &bonding_curve,
            &associated_bonding_curve,
            &user_pubkey,
            &user_token_account,
            &creator_vault,
            &fee_config,
            amount_tokens,
            min_sol_output,
        )?;

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[ix],
            Some(&user_pubkey),
            &[keypair],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!("✅ Pump.fun sell successful: {}", signature);

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

    fn derive_creator_vault(&self, creator: &Pubkey) -> Result<Pubkey> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"creator-vault", creator.as_ref()],
            &program_id,
        );
        Ok(pda)
    }

    fn derive_global_volume_accumulator(&self) -> Result<Pubkey> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"global_volume_accumulator"],
            &program_id,
        );
        Ok(pda)
    }

    fn derive_user_volume_accumulator(&self, user: &Pubkey) -> Result<Pubkey> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"user_volume_accumulator", user.as_ref()],
            &program_id,
        );
        Ok(pda)
    }

    fn derive_fee_config(&self) -> Result<Pubkey> {
        let fee_program = Pubkey::from_str(FEE_PROGRAM)?;
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"fee_config", &FEE_CONFIG_SEED],
            &fee_program,
        );
        Ok(pda)
    }

    fn fetch_bonding_curve_state(&self, bonding_curve: &Pubkey) -> Result<BondingCurveState> {
        let account_data = self.rpc_client.get_account_data(bonding_curve)?;
        Self::parse_bonding_curve_state(&account_data)
    }

    fn parse_bonding_curve_state(data: &[u8]) -> Result<BondingCurveState> {
        let expected_len = 8 + 8 * 5 + 1 + 32 + 1;
        if data.len() < expected_len {
            return Err(anyhow::anyhow!("Bonding curve account too small"));
        }

        let mut offset = 8; // discriminator
        let read_u64 = |data: &[u8], offset: &mut usize| -> u64 {
            let value = u64::from_le_bytes(data[*offset..*offset + 8].try_into().unwrap());
            *offset += 8;
            value
        };

        let virtual_token_reserves = read_u64(data, &mut offset);
        let virtual_sol_reserves = read_u64(data, &mut offset);
        let real_token_reserves = read_u64(data, &mut offset);
        let real_sol_reserves = read_u64(data, &mut offset);
        let token_total_supply = read_u64(data, &mut offset);
        let complete = data[offset] != 0;
        offset += 1;

        let creator = Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap());
        offset += 32;
        let is_mayhem_mode = data[offset] != 0;

        Ok(BondingCurveState {
            virtual_token_reserves,
            virtual_sol_reserves,
            real_token_reserves,
            real_sol_reserves,
            token_total_supply,
            complete,
            creator,
            is_mayhem_mode,
        })
    }

    fn estimate_sell_output(&self, state: &BondingCurveState, amount_tokens: u64) -> u64 {
        let amount_in = amount_tokens as u128;
        let virtual_token = state.virtual_token_reserves as u128;
        let virtual_sol = state.virtual_sol_reserves as u128;

        if amount_in == 0 || virtual_sol == 0 {
            return 0;
        }

        let output = (amount_in * virtual_sol) / (virtual_token + amount_in);
        output as u64
    }

    fn build_buy_instruction(
        &self,
        mint: &Pubkey,
        bonding_curve: &Pubkey,
        associated_bonding_curve: &Pubkey,
        user: &Pubkey,
        user_token_account: &Pubkey,
        creator_vault: &Pubkey,
        global_volume_accumulator: &Pubkey,
        user_volume_accumulator: &Pubkey,
        fee_config: &Pubkey,
        amount_sol: f64,
        slippage_bps: u16,
    ) -> Result<Instruction> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let global = Pubkey::from_str(GLOBAL_ACCOUNT)?;
        let fee_recipient = Pubkey::from_str(FEE_RECIPIENT)?;
        let system_program = Pubkey::from_str(SYSTEM_PROGRAM)?;
        let token_program = Pubkey::from_str(TOKEN_PROGRAM)?;
        let fee_program = Pubkey::from_str(FEE_PROGRAM)?;
        let event_authority = Pubkey::from_str(EVENT_AUTHORITY)?;

        // Convert SOL to lamports
        let amount_lamports = (amount_sol * 1_000_000_000.0) as u64;

        // Calculate max slippage (simplified - should calculate expected tokens)
        let max_sol_cost = amount_lamports + (amount_lamports * slippage_bps as u64 / 10000);

        // Build instruction data: discriminator + amount + max_sol_cost + track_volume
        let mut data = Vec::new();
        data.extend_from_slice(&BUY_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&amount_lamports.to_le_bytes());
        data.extend_from_slice(&max_sol_cost.to_le_bytes());
        data.push(0); // track_volume = false

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
                AccountMeta::new(*creator_vault, false),
                AccountMeta::new_readonly(event_authority, false),
                AccountMeta::new_readonly(program_id, false), // Program itself
                AccountMeta::new(*global_volume_accumulator, false),
                AccountMeta::new(*user_volume_accumulator, false),
                AccountMeta::new_readonly(*fee_config, false),
                AccountMeta::new_readonly(fee_program, false),
            ],
            data,
        })
    }

    fn build_sell_instruction(
        &self,
        mint: &Pubkey,
        bonding_curve: &Pubkey,
        associated_bonding_curve: &Pubkey,
        user: &Pubkey,
        user_token_account: &Pubkey,
        creator_vault: &Pubkey,
        fee_config: &Pubkey,
        amount_tokens: u64,
        min_sol_output: u64,
    ) -> Result<Instruction> {
        let program_id = Pubkey::from_str(PUMP_PROGRAM_ID)?;
        let global = Pubkey::from_str(GLOBAL_ACCOUNT)?;
        let fee_recipient = Pubkey::from_str(FEE_RECIPIENT)?;
        let system_program = Pubkey::from_str(SYSTEM_PROGRAM)?;
        let token_program = Pubkey::from_str(TOKEN_PROGRAM)?;
        let fee_program = Pubkey::from_str(FEE_PROGRAM)?;
        let event_authority = Pubkey::from_str(EVENT_AUTHORITY)?;

        let mut data = Vec::new();
        data.extend_from_slice(&SELL_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&amount_tokens.to_le_bytes());
        data.extend_from_slice(&min_sol_output.to_le_bytes());

        Ok(Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new_readonly(global, false),
                AccountMeta::new(fee_recipient, false),
                AccountMeta::new(*mint, false),
                AccountMeta::new(*bonding_curve, false),
                AccountMeta::new(*associated_bonding_curve, false),
                AccountMeta::new(*user_token_account, false),
                AccountMeta::new(*user, true),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new(*creator_vault, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(event_authority, false),
                AccountMeta::new_readonly(program_id, false),
                AccountMeta::new_readonly(*fee_config, false),
                AccountMeta::new_readonly(fee_program, false),
            ],
            data,
        })
    }
}
