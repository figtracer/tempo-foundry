//! Transaction environment conversions for Tempo.
//!
//! This module provides newtype wrappers to bridge Tempo's transaction types
//! with existing Foundry/OP-stack infrastructure.

use alloy_evm::{FromRecoveredTx, IntoTxEnv};
use alloy_primitives::{Address, Bytes};
use op_revm::OpTransaction;
use revm::context::TxEnv;
use std::ops::{Deref, DerefMut};
use tempo_revm::TempoTxEnv;

use crate::FoundryTxEnvelope;

/// A newtype wrapper around `TempoTxEnv` that implements conversions needed
/// for compatibility with `EitherEvm`.
///
/// This wrapper allows `TempoTxEnv` to be used in contexts that expect
/// `OpTransaction<TxEnv>`, bridging the gap between Tempo and OP-stack types.
#[derive(Clone, Debug, Default)]
pub struct FoundryTempoTxEnv {
    /// The inner Tempo transaction environment.
    pub inner: TempoTxEnv,
    /// The RLP-encoded transaction bytes for OP-stack L1 fee calculation.
    /// This is only set when running in Optimism mode.
    pub enveloped_tx: Option<Bytes>,
}

impl FoundryTempoTxEnv {
    /// Creates a new `FoundryTempoTxEnv` from a `TempoTxEnv`.
    pub fn new(tx: TempoTxEnv) -> Self {
        Self { inner: tx, enveloped_tx: None }
    }

    /// Creates a new `FoundryTempoTxEnv` with enveloped tx bytes for OP-stack.
    pub fn with_enveloped_tx(tx: TempoTxEnv, enveloped_tx: Option<Bytes>) -> Self {
        Self { inner: tx, enveloped_tx }
    }

    /// Consumes the wrapper and returns the inner `TempoTxEnv`.
    pub fn into_inner(self) -> TempoTxEnv {
        self.inner
    }
}

impl From<TempoTxEnv> for FoundryTempoTxEnv {
    fn from(tx: TempoTxEnv) -> Self {
        Self { inner: tx, enveloped_tx: None }
    }
}

impl From<FoundryTempoTxEnv> for TempoTxEnv {
    fn from(wrapper: FoundryTempoTxEnv) -> Self {
        wrapper.inner
    }
}

impl Deref for FoundryTempoTxEnv {
    type Target = TempoTxEnv;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for FoundryTempoTxEnv {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Implementation of `IntoTxEnv<OpTransaction<TxEnv>>` for `FoundryTempoTxEnv`.
///
/// This is a bridge implementation that allows `TempoTxEnv` (via the wrapper) to be
/// used with `EitherEvm` which expects `OpTransaction<TxEnv>` as its transaction type.
///
/// Note: This conversion loses Tempo-specific fields (fee_token, tempo_tx_env, etc.)
/// and should only be used until the anvil codebase is fully migrated to use
/// Tempo-native EVM types.
impl IntoTxEnv<OpTransaction<TxEnv>> for FoundryTempoTxEnv {
    fn into_tx_env(self) -> OpTransaction<TxEnv> {
        OpTransaction {
            base: self.inner.inner,
            enveloped_tx: self.enveloped_tx,
            ..Default::default()
        }
    }
}

/// Implementation of `FromRecoveredTx<FoundryTxEnvelope>` for `FoundryTempoTxEnv`.
///
/// This allows creating a `FoundryTempoTxEnv` from a recovered `FoundryTxEnvelope`,
/// which is needed for transaction execution in anvil.
impl FromRecoveredTx<FoundryTxEnvelope> for FoundryTempoTxEnv {
    fn from_recovered_tx(tx: &FoundryTxEnvelope, caller: Address) -> Self {
        match tx {
            // Handle Tempo transactions natively using TempoTxEnv's FromRecoveredTx impl
            FoundryTxEnvelope::Tempo(aa_signed) => Self {
                inner: TempoTxEnv::from_recovered_tx(aa_signed, caller),
                enveloped_tx: None,
            },
            // For all other transaction types, convert through OpTransaction<TxEnv>
            _ => {
                let op_tx: OpTransaction<TxEnv> = FromRecoveredTx::from_recovered_tx(tx, caller);
                Self {
                    inner: TempoTxEnv { inner: op_tx.base, ..Default::default() },
                    enveloped_tx: op_tx.enveloped_tx,
                }
            }
        }
    }
}
