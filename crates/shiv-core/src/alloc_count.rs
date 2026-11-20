//! A counting allocator for the per-flow allocation ceilings
//! (`tests/alloc_ceilings.rs`, docs/proof-plan.md "One allocation profile
//! per flow").
//!
//! The counting is **total bytes handed out, not live bytes**: when frees
//! happen is up to the caller and varies between runs, while totals are
//! stable for a fixed flow.
//!
//! An integration-test binary that wants its flows measured declares
//!
//! ```rust,ignore
//! #[global_allocator]
//! static ALLOCATOR: CountingAlloc = CountingAlloc;
//! ```
//!
//! and every allocation that binary makes — shiv-core's included — passes
//! the counter. Nothing outside the test suite uses this module.
//!
//! The author-facing rule when a ceiling test fails: the change made a
//! user flow allocate more bytes. Remove the allocation, or raise the
//! ceiling with a written reason saying what the bytes are and why the
//! flow needs them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// A [`GlobalAlloc`] that delegates to [`System`] and counts every byte it
/// hands out into [`allocated_bytes`]. Freed bytes are not subtracted.
/// `alloc_zeroed` and `realloc` keep the trait's defaults, which route
/// through [`GlobalAlloc::alloc`], so they are counted too.
pub struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Total bytes [`CountingAlloc`] has handed out since the last
/// [`reset_allocated_bytes`].
pub fn allocated_bytes() -> u64 {
    ALLOCATED_BYTES.load(Ordering::Relaxed) as u64
}

/// Zero the byte counter.
pub fn reset_allocated_bytes() {
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
}
