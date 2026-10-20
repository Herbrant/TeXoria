//! An allocator that counts the bytes in use, for the measuring examples (`examples/*-perf.rs`):
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: test_support::alloc::Counting = test_support::alloc::Counting;
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, keeping track of the bytes allocated and not yet freed.
pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
            grow(new_size);
        }
        new
    }
}

fn grow(size: usize) {
    let now = CURRENT.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

/// The bytes in use now.
pub fn current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

/// The most bytes in use at once since the last `reset_peak`.
pub fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Start measuring the peak from the bytes in use now.
pub fn reset_peak() {
    PEAK.store(current(), Ordering::Relaxed);
}

/// `bytes` in a readable unit.
pub fn human(bytes: usize) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / (1 << 10) as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_units() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(3 << 20), "3.0 MB");
    }
}
