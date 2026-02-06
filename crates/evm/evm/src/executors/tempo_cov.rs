use foundry_tempo_coverage::COVERAGE_MAP_SIZE;

use super::RawCallResult;

/// RAII guard that activates Tempo precompile coverage collection for the duration of an EVM call.
///
/// Allocates a thread-local scratch buffer, sets it as the active coverage map via
/// `foundry_tempo_coverage`, and on drop clears it. The collected hits can then be merged
/// into the `RawCallResult`'s `edge_coverage` via [`Self::merge_into`].
pub(super) struct TempoCoverageGuard;

thread_local! {
    static TEMPO_COV_BUFFER: std::cell::RefCell<Vec<u8>> =
        std::cell::RefCell::new(vec![0u8; COVERAGE_MAP_SIZE]);
}

impl TempoCoverageGuard {
    pub(super) fn new() -> Self {
        TEMPO_COV_BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();
            buf.fill(0);
            let ptr = buf.as_mut_ptr();
            let len = buf.len();
            foundry_tempo_coverage::set_coverage_map(ptr, len);
        });
        foundry_tempo_coverage::clear_cmp_operands();
        Self
    }

    /// Merge Tempo precompile coverage hits into the `RawCallResult`'s edge coverage.
    ///
    /// If the result already has an `edge_coverage` map (from `EdgeCovInspector`), Tempo precompile
    /// hits are added into it. If not, the Tempo precompile coverage buffer becomes the edge
    /// coverage.
    ///
    /// Also drains any comparison operands captured by trace-cmp callbacks and attaches them
    /// to the result for injection into the fuzz dictionary.
    pub(super) fn merge_into(result: &mut RawCallResult) {
        TEMPO_COV_BUFFER.with(|buf| {
            let buf = buf.borrow();
            let has_any_hit = buf.iter().any(|&b| b > 0);
            if !has_any_hit {
                return;
            }

            match &mut result.edge_coverage {
                Some(existing) => {
                    for (existing_slot, &native_hit) in existing.iter_mut().zip(buf.iter()) {
                        *existing_slot = existing_slot.saturating_add(native_hit);
                    }
                }
                None => {
                    result.edge_coverage = Some(buf.clone());
                }
            }
        });

        let cmp_values = foundry_tempo_coverage::drain_cmp_operands();
        if !cmp_values.is_empty() {
            result.tempo_cmp_values = Some(cmp_values);
        }
    }
}

impl Drop for TempoCoverageGuard {
    fn drop(&mut self) {
        foundry_tempo_coverage::clear_coverage_map();
    }
}
