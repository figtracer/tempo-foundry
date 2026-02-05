//! Tempo precompile coverage feedback for coverage-guided fuzzing.
//!
//! Provides SanitizerCoverage callbacks and a thread-local coverage map pointer
//! that can be set by the fuzzing executor to collect edge coverage from Tempo precompile code.

use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

pub const COVERAGE_MAP_SIZE: usize = 65536;

// Use atomics instead of thread-locals so that sancov callbacks are safe to
// call during static initialization (before thread-locals are available).
// Coverage is single-threaded per fuzz run, so relaxed ordering is fine.
static COVERAGE_MAP_PTR: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static COVERAGE_MAP_LEN: AtomicUsize = AtomicUsize::new(0);

pub fn set_coverage_map(ptr: *mut u8, len: usize) {
    COVERAGE_MAP_PTR.store(ptr, Ordering::Release);
    COVERAGE_MAP_LEN.store(len, Ordering::Release);
}

pub fn clear_coverage_map() {
    COVERAGE_MAP_PTR.store(std::ptr::null_mut(), Ordering::Release);
    COVERAGE_MAP_LEN.store(0, Ordering::Release);
}

pub fn is_active() -> bool {
    !COVERAGE_MAP_PTR.load(Ordering::Relaxed).is_null()
}

pub struct CoverageMapGuard;

impl CoverageMapGuard {
    pub fn new(ptr: *mut u8, len: usize) -> Self {
        set_coverage_map(ptr, len);
        Self
    }
}

impl Drop for CoverageMapGuard {
    fn drop(&mut self) {
        clear_coverage_map();
    }
}

#[inline(always)]
pub fn record_hit(guard_id: u32) {
    let ptr = COVERAGE_MAP_PTR.load(Ordering::Relaxed);
    if ptr.is_null() {
        return;
    }
    let len = COVERAGE_MAP_LEN.load(Ordering::Relaxed);
    if len == 0 {
        return;
    }
    let idx = guard_id as usize % len;
    unsafe {
        let slot = ptr.add(idx);
        *slot = (*slot).wrapping_add(1);
    }
}

static GUARD_COUNTER: AtomicU32 = AtomicU32::new(1);

/// # Safety
///
/// Called by the LLVM SanitizerCoverage runtime at startup. `[start, stop)` must be a valid
/// range of mutable `u32` guard slots allocated by the compiler for the current DSO.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __sanitizer_cov_trace_pc_guard_init(mut start: *mut u32, stop: *mut u32) {
    while start < stop {
        let id = GUARD_COUNTER.fetch_add(1, Ordering::Relaxed);
        unsafe {
            *start = id;
            start = start.add(1);
        }
    }
}

/// # Safety
///
/// Called by the LLVM SanitizerCoverage runtime at every instrumented CFG edge.
/// `guard` must point to a valid `u32` guard slot initialized by `__sanitizer_cov_trace_pc_guard_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __sanitizer_cov_trace_pc_guard(guard: *mut u32) {
    let id = unsafe { *guard };
    if id == 0 {
        return;
    }
    record_hit(id);
}
