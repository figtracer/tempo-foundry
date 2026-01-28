//! Tempo precompile and fee token initialization for Anvil.
//!
//! When running in Tempo mode, Anvil needs to set up Tempo-specific precompiles
//! and fee tokens (PathUSD, AlphaUSD, BetaUSD, ThetaUSD) to enable proper
//! transaction validation.
//!
//! This module provides a storage provider adapter for Anvil's `Db` trait and
//! uses the shared initialization logic from `foundry-evm-core`.

use alloy_primitives::{Address, U256, address};
use foundry_evm::core::tempo::initialize_tempo_genesis;
use revm::state::{AccountInfo, Bytecode};
use std::collections::HashMap;
use tempo_chainspec::hardfork::TempoHardfork;
use tempo_precompiles::{
    error::TempoPrecompileError,
    storage::{PrecompileStorageProvider, StorageCtx},
    tip20::{ITIP20, TIP20Token},
};

use super::db::Db;

/// Sender address used for genesis initialization.
const SENDER: Address = address!("0x1804c8AB1F12E6bbf3894d4083f33e07309d1f38");
/// Admin address used for genesis initialization.
const ADMIN: Address = address!("0x5615dEB798BB3E4dFa0139dFa1b3D433Cc23b72f");

/// PathUSD token address
const PATH_USD: Address = address!("20C0000000000000000000000000000000000000");
/// AlphaUSD token address
const ALPHA_USD: Address = address!("20C0000000000000000000000000000000000001");
/// BetaUSD token address
const BETA_USD: Address = address!("20C0000000000000000000000000000000000002");
/// ThetaUSD token address
const THETA_USD: Address = address!("20C0000000000000000000000000000000000003");

/// Storage provider adapter for Anvil's Db to work with Tempo precompiles.
pub struct AnvilStorageProvider<'a> {
    db: &'a mut dyn Db,
    chain_id: u64,
    timestamp: U256,
    gas_used: u64,
    gas_refunded: i64,
    transient: HashMap<(Address, U256), U256>,
    hardfork: TempoHardfork,
}

impl<'a> AnvilStorageProvider<'a> {
    pub fn new(
        db: &'a mut dyn Db,
        chain_id: u64,
        timestamp: U256,
        hardfork: TempoHardfork,
    ) -> Self {
        Self {
            db,
            chain_id,
            timestamp,
            gas_used: 0,
            gas_refunded: 0,
            transient: HashMap::new(),
            hardfork,
        }
    }
}

impl PrecompileStorageProvider for AnvilStorageProvider<'_> {
    fn spec(&self) -> TempoHardfork {
        self.hardfork
    }

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn timestamp(&self) -> U256 {
        self.timestamp
    }

    fn set_code(&mut self, address: Address, code: Bytecode) -> Result<(), TempoPrecompileError> {
        self.db.insert_account(
            address,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                nonce: 1,
                ..Default::default()
            },
        );
        Ok(())
    }

    fn with_account_info(
        &mut self,
        address: Address,
        f: &mut dyn FnMut(&AccountInfo),
    ) -> Result<(), TempoPrecompileError> {
        use revm::DatabaseRef;
        if let Some(info) =
            self.db.basic_ref(address).map_err(|e| TempoPrecompileError::Fatal(e.to_string()))?
        {
            f(&info);
            Ok(())
        } else {
            Err(TempoPrecompileError::Fatal(format!("account '{address}' not found")))
        }
    }

    fn sstore(
        &mut self,
        address: Address,
        key: U256,
        value: U256,
    ) -> Result<(), TempoPrecompileError> {
        use alloy_primitives::B256;
        self.db
            .set_storage_at(address, B256::from(key), B256::from(value))
            .map_err(|e| TempoPrecompileError::Fatal(e.to_string()))
    }

    fn sload(&mut self, address: Address, key: U256) -> Result<U256, TempoPrecompileError> {
        revm::Database::storage(self.db, address, key)
            .map_err(|e| TempoPrecompileError::Fatal(e.to_string()))
    }

    fn tstore(
        &mut self,
        address: Address,
        key: U256,
        value: U256,
    ) -> Result<(), TempoPrecompileError> {
        self.transient.insert((address, key), value);
        Ok(())
    }

    fn tload(&mut self, address: Address, key: U256) -> Result<U256, TempoPrecompileError> {
        Ok(self.transient.get(&(address, key)).copied().unwrap_or(U256::ZERO))
    }

    fn emit_event(
        &mut self,
        _address: Address,
        _event: alloy_primitives::LogData,
    ) -> Result<(), TempoPrecompileError> {
        Ok(())
    }

    fn deduct_gas(&mut self, gas: u64) -> Result<(), TempoPrecompileError> {
        self.gas_used = self.gas_used.saturating_add(gas);
        Ok(())
    }

    fn gas_used(&self) -> u64 {
        self.gas_used
    }

    fn gas_refunded(&self) -> i64 {
        self.gas_refunded
    }

    fn refund_gas(&mut self, gas: i64) {
        self.gas_refunded = self.gas_refunded.saturating_add(gas);
    }

    fn beneficiary(&self) -> Address {
        Address::ZERO
    }

    fn is_static(&self) -> bool {
        false
    }
}

/// Initialize Tempo precompiles and fee tokens for Anvil.
///
/// This sets up the same precompiles and tokens as Tempo's genesis, enabling
/// proper fee token validation for transactions.
///
/// Additionally, mints fee tokens to the provided test accounts so they can
/// send transactions in Tempo mode.
pub fn initialize_tempo_precompiles(
    db: &mut dyn Db,
    chain_id: u64,
    timestamp: u64,
    test_accounts: &[Address],
) -> Result<(), TempoPrecompileError> {
    let hardfork = TempoHardfork::default();
    let timestamp = U256::from(timestamp);

    let mut storage = AnvilStorageProvider::new(db, chain_id, timestamp, hardfork);

    // Initialize base Tempo genesis (precompiles and tokens)
    initialize_tempo_genesis(&mut storage, ADMIN, SENDER)?;

    // Mint fee tokens to test accounts
    // u64::MAX per account - safe since u128::MAX can hold ~18 quintillion u64::MAX values
    let mint_amount = U256::from(u64::MAX);
    let tokens = [PATH_USD, ALPHA_USD, BETA_USD, THETA_USD];

    StorageCtx::enter(&mut storage, || -> Result<(), TempoPrecompileError> {
        for &token_address in &tokens {
            let mut token = TIP20Token::from_address(token_address)?;
            for &account in test_accounts {
                token.mint(ADMIN, ITIP20::mintCall { to: account, amount: mint_amount })?;
            }
        }
        Ok(())
    })?;

    Ok(())
}
