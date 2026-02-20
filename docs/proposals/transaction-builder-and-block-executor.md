# Proposal: TransactionBuilder Sticky to Network + BlockExecutor Adoption

## Context

We have fundamental flaws in several abstraction layers:
- **alloy**: `TransactionBuilder` can't express network-specific fields (Tempo's `fee_token`, `nonce_key`, 2D nonces, validity windows)
- **foundry primitives**: `FoundryTransactionRequest` is a 3-variant enum with lossy `AsRef<TransactionRequest>` delegation
- **anvil execution**: `EitherEvm` enum has panicking impls and unsafe transmutes
- **anvil executor**: `TransactionExecutor` creates a new EVM per transaction instead of using alloy-evm's `BlockExecutor`

This doc covers two related proposals: fixing TransactionBuilder upstream in alloy, and adopting BlockExecutor in anvil.

---

## Part 1: TransactionBuilder Sticky to Network

### Problem

alloy's `TransactionBuilder<N: Network>` trait has hardcoded Ethereum methods:

```rust
trait TransactionBuilder<N: Network> {
    fn nonce(&self) -> Option<u64>;        // flat u64 — can't express Tempo's 2D nonce
    fn set_nonce(&mut self, nonce: u64);
    fn gas_limit(&self) -> Option<u64>;
    fn value(&self) -> Option<U256>;
    // ... all standard Ethereum fields only
}
```

Extension traits (`TransactionBuilder4844`, `TransactionBuilder7702`) exist but are **decoupled from Network** — they're standalone traits with no connection to which network they apply to.

Our `FoundryTransactionRequest` works around this with an enum:

```rust
pub enum FoundryTransactionRequest {
    Ethereum(TransactionRequest),
    Op(WithOtherFields<TransactionRequest>),
    Tempo(Box<TempoTransactionRequest>),
}
```

The `TransactionBuilder<FoundryNetwork>` impl delegates every getter/setter through `self.as_ref() -> &TransactionRequest`, which means:
- **`fee_token`** — invisible through the trait
- **`nonce_key`** (2D nonce) — collapses to flat `u64`
- **`valid_before`/`valid_after`** — invisible
- **`tempo_authorization_list`** — invisible
- **`calls`** (multi-call batching) — only `first_call` survives via `input()`

Provider middleware (fillers like `NonceFiller`, `GasFiller`) operates through the trait, so it can never fill Tempo-specific fields.

### What Broke

The routing from `WithOtherFields<TransactionRequest>` to `FoundryTransactionRequest` is fragile runtime dispatch based on field name sniffing:

```rust
if tx.other.contains_key("feeToken") || tx.other.contains_key("nonceKey") {
    Self::Tempo(...)
} else if tx.transaction_type == Some(DEPOSIT_TX_TYPE_ID) {
    Self::Op(tx)
} else {
    Self::Ethereum(tx.into_inner())
}
```

No compile-time guarantees. Lossy conversion. `build_unsigned()` works because it bypasses the trait and calls `build_typed_tx()` directly, but everything else (fillers, middleware, generic provider code) sees the Ethereum projection.

### Proposed Fix

#### Step 1: Define `TempoTransactionBuilder` extension trait (in tempo-alloy)

Following the existing pattern of `TransactionBuilder4844`/`TransactionBuilder7702`:

```rust
pub trait TempoTransactionBuilder: Default + Sized + Send + Sync + 'static {
    fn fee_token(&self) -> Option<Address>;
    fn set_fee_token(&mut self, token: Address);
    fn with_fee_token(mut self, token: Address) -> Self {
        self.set_fee_token(token);
        self
    }

    fn nonce_key(&self) -> Option<U256>;
    fn set_nonce_key(&mut self, key: U256);
    fn with_nonce_key(mut self, key: U256) -> Self {
        self.set_nonce_key(key);
        self
    }

    fn valid_before(&self) -> Option<u64>;
    fn set_valid_before(&mut self, ts: u64);

    fn valid_after(&self) -> Option<u64>;
    fn set_valid_after(&mut self, ts: u64);

    fn key_id(&self) -> Option<Address>;
    fn set_key_id(&mut self, key_id: Address);
}
```

This is the least invasive change — no alloy upstream modification needed, follows the existing pattern.

#### Step 2: Alloy upstream PR — Associate extension constraints with Network

Currently `Network::TransactionRequest` only requires `TransactionBuilder<Self>`. Propose adding an associated type for network-specific builder extensions:

```rust
pub trait Network {
    type TxType: ...;
    type TxEnvelope: ...;
    type TransactionRequest: TransactionBuilder<Self>;

    // NEW: Network-specific builder constraint
    // Default to () for networks with no extensions
    type TransactionBuilderExt = ();
}
```

For Tempo:
```rust
impl Network for TempoNetwork {
    type TransactionRequest: TransactionBuilder<Self> + TempoTransactionBuilder;
    type TransactionBuilderExt = dyn TempoTransactionBuilder;
}
```

**Motivation for alloy to accept this**: OP-Stack has the same problem with deposit tx fields. This isn't Tempo-specific — it's any L2/alt-chain that adds fields beyond EIP-1559.

#### Step 3: Make generic code network-aware

Replace concrete `TempoNetwork` / `FoundryNetwork` references with generic `N: Network` bounds:

```rust
// Before (locked to concrete type):
pub async fn estimate_gas<P: Provider<TempoNetwork>>(
    tx: &mut WithOtherFields<TempoTransactionRequest>,
    provider: &P,
) -> Result<()>

// After (generic over network):
pub async fn estimate_gas<N: Network, P: Provider<N>>(
    tx: &mut N::TransactionRequest,
    provider: &P,
) -> Result<()>
```

Code that needs Tempo-specific fields adds the extension bound:

```rust
pub async fn fill_tempo_fields<N, P>(tx: &mut N::TransactionRequest, provider: &P)
where
    N: Network,
    N::TransactionRequest: TempoTransactionBuilder,
    P: Provider<N>,
{
    tx.set_fee_token(PATH_USD);
    tx.set_nonce_key(U256::ZERO);
}
```

This gives compile-time safety: you can't accidentally call `set_nonce_key` on an Ethereum `TransactionRequest`.

### Migration Path

1. **Now**: Define `TempoTransactionBuilder` trait in tempo-alloy. Implement for `TempoTransactionRequest`. No alloy upstream change needed.
2. **PR to alloy**: Propose `Network::TransactionBuilderExt` associated type with default. Write motivation showing OP-Stack + Tempo both need this.
3. **After alloy merge**: Update `FoundryTransactionRequest` to use trait-based dispatch instead of enum + field sniffing.
4. **Eventually**: Kill `FoundryNetwork` — it's a lie. Use `TempoNetwork` or `Ethereum` or `OpStack` directly, parameterized through generics.

---

## Part 2: BlockExecutor Adoption (foundry #12679)

### Problem

Anvil's `TransactionExecutor` (in `crates/anvil/src/eth/backend/executor.rs`) creates a fresh EVM per transaction:

```rust
// Current: per-tx EVM creation
for tx in pending_txs {
    let mut evm = new_evm_with_inspector(&mut *self.db, &env, &mut inspector);
    // inject precompiles...
    match evm.transact_commit(env.tx) { ... }
}
```

This goes through `EitherEvm` which has serious problems:
- **Panicking impls**: `components()`, `inspector()`, `system_call()` panic for Tempo variant
- **Unsafe transmute**: `precompiles()` uses `unsafe { std::mem::transmute(...) }` to handle type mismatches
- **Type mismatch**: Tempo's `TempoHaltReason` gets force-mapped to `OpHaltReason` via lossy error conversion

### Proposed Fix

Adopt alloy-evm's `BlockExecutor` pattern. The flow becomes:

```
create EVM factory (per network) → create BlockExecutor → executor.execute_block(txs)
```

#### Step 1: Fix EitherEvm (prerequisite)

Replace panicking impls and unsafe transmutes:

- **`components()` / `inspector()`**: Either implement properly for all variants, or change the trait bound to not require these (they're only used by cheats).
- **`precompiles()`**: Remove the `unsafe transmute`. Use a proper `PrecompilesMap` type that's generic over the context, or wrap in a safe accessor.
- **Error types**: Replace the `map_tempo_err_to_op` lossy conversion with a proper `FoundryHaltReason` enum:

```rust
pub enum FoundryHaltReason {
    Eth(HaltReason),
    Op(OpHaltReason),
    Tempo(TempoHaltReason),
}
```

#### Step 2: Per-network BlockExecutor impls

Instead of one `EitherEvm` that must satisfy all trait bounds, each network gets its own executor:

```rust
trait FoundryBlockExecutor {
    fn execute_transactions(
        &mut self,
        txs: Vec<Arc<PoolTransaction>>,
        env: &Env,
    ) -> Vec<TransactionExecutionOutcome>;
}

struct EthBlockExecutor<DB> { ... }
struct OpBlockExecutor<DB> { ... }
struct TempoBlockExecutor<DB> { ... }
```

The dispatch happens once at block level, not per-transaction:

```rust
match network {
    NetworkConfig::Ethereum => EthBlockExecutor::new(db).execute_transactions(txs, env),
    NetworkConfig::Op => OpBlockExecutor::new(db).execute_transactions(txs, env),
    NetworkConfig::Tempo => TempoBlockExecutor::new(db).execute_transactions(txs, env),
}
```

#### Step 3: Integrate anvil-specific hooks

The executor needs to support:
- Inspector injection (tracing, cheats)
- Precompile injection (cheat precompiles)
- Per-tx state (gas tracking, bloom filters, logs)
- Tempo-specific validation (`valid_before`/`valid_after` time windows, fee token balance checks)

This is where the alloy-evm `BlockExecutor` trait gets wrapped:

```rust
struct FoundryBlockExecutor<E: alloy_evm::BlockExecutor> {
    inner: E,
    cheats: Option<Cheatcodes>,
    tracer: Option<TracingInspector>,
}
```

### Why This Matters for Upstreaming

The goal is to upstream Tempo support into foundry proper. The current `EitherEvm` approach:
1. Requires every trait method to handle 3 variants (Eth/Op/Tempo)
2. Forces unsafe code to bridge type mismatches
3. Couples all three implementations at every call site

The BlockExecutor approach:
1. Each network is self-contained
2. Shared behavior lives in `FoundryBlockExecutor<E>` wrapper
3. Adding a new network means implementing one struct, not touching every match arm
4. Aligns with how reth handles execution (making upstream easier)

### Migration Path

1. **Now**: Clean up `EitherEvm` — kill panics, kill unsafe transmute, add `FoundryHaltReason`
2. **Next**: Implement per-network executor structs behind the `FoundryBlockExecutor` trait
3. **Then**: Wire into anvil's `TransactionExecutor`, replacing the per-tx EVM creation loop
4. **Finally**: Remove `EitherEvm` entirely once all callers use the executor pattern

---

## Dependency Order

```
TempoTransactionBuilder trait (tempo-alloy)     [no deps, start now]
    ↓
Alloy PR: Network::TransactionBuilderExt        [needs motivation doc]
    ↓
Cast rewrite to generic Network                  [mablr, in progress]
    ↓
EitherEvm cleanup (kill panics/unsafe)           [after cast, or parallel]
    ↓
FoundryBlockExecutor + per-network executors     [after EitherEvm]
    ↓
Kill FoundryNetwork, use real Network types      [after all above]
```
