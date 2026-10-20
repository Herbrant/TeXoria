//! Big allocations straight from the system, on macOS.
//!
//! macOS's allocator keeps the large blocks it is given back, without bound and without a way
//! to make it let them go (`malloc_zone_pressure_relief` frees nothing): after zooming the PDF
//! in and out, the page bitmaps it rasterized (up to `pdf_view`'s cap, about 190 MB each) stay
//! in the app's footprint, gigabytes of it. Blocks this big are mapped and unmapped here
//! instead, so that they go back to the system as soon as they are freed.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::{c_int, c_void};

/// From this size on, a block is mapped: page bitmaps, PDFs, big files' text. A system call
/// costs little next to filling that much memory.
const BIG: usize = 4 << 20;
/// Mappings are aligned on pages (16 KB on Apple silicon, 4 KB on Intel).
const MAP_ALIGN: usize = 4096;

pub struct Allocator;

fn mapped(layout: Layout) -> bool {
    layout.size() >= BIG && layout.align() <= MAP_ALIGN
}

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        off: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
}

const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const MAP_PRIVATE: c_int = 0x2;
const MAP_ANON: c_int = 0x1000;

/// `size` bytes of new, zeroed pages, or null.
fn map(size: usize) -> *mut u8 {
    // SAFETY: an anonymous private mapping anywhere: no memory of ours is affected.
    let p = unsafe {
        mmap(
            std::ptr::null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        )
    };
    // MAP_FAILED
    if p as isize == -1 {
        std::ptr::null_mut()
    } else {
        p.cast()
    }
}

// SAFETY: blocks are mapped or not by their layout alone, which the caller gives back
// unchanged on `dealloc` and `realloc`: each goes back to where it came from.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if mapped(layout) {
            map(layout.size())
        } else {
            // SAFETY: the caller's guarantees, passed on.
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if mapped(layout) {
            // New pages are zeroed.
            map(layout.size())
        } else {
            // SAFETY: the caller's guarantees, passed on.
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if mapped(layout) {
            // SAFETY: mapped by `map` with this size.
            unsafe { munmap(ptr.cast(), layout.size()) };
        } else {
            // SAFETY: the caller's guarantees, passed on.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller guarantees the new size is valid for the alignment.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if !mapped(layout) && !mapped(new_layout) {
            // SAFETY: the caller's guarantees, passed on.
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        // SAFETY: a valid layout, as above.
        let new = unsafe { self.alloc(new_layout) };
        if !new.is_null() {
            // SAFETY: both blocks hold at least the smaller size, and they are distinct.
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    /// The process's footprint, as Activity Monitor shows it.
    fn footprint() -> u64 {
        #[repr(C)]
        struct RusageInfoV0 {
            uuid: [u8; 16],
            user_time: u64,
            system_time: u64,
            pkg_idle_wkups: u64,
            interrupt_wkups: u64,
            pageins: u64,
            wired_size: u64,
            resident_size: u64,
            phys_footprint: u64,
            proc_start_abstime: u64,
            proc_exit_abstime: u64,
        }
        unsafe extern "C" {
            fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
        }
        let mut info = std::mem::MaybeUninit::<RusageInfoV0>::zeroed();
        // SAFETY: RUSAGE_INFO_V0 (0) fills exactly this structure.
        let r =
            unsafe { proc_pid_rusage(std::process::id() as c_int, 0, info.as_mut_ptr().cast()) };
        assert_eq!(r, 0);
        // SAFETY: filled above.
        unsafe { info.assume_init() }.phys_footprint
    }

    #[test]
    fn big_blocks_go_back_to_the_system() {
        let mb = 1 << 20;
        let blocks: Vec<Vec<u8>> = (0..10).map(|i| vec![i as u8 + 1; 40 * mb]).collect();
        assert!(
            blocks
                .iter()
                .enumerate()
                .all(|(i, b)| b[40 * mb - 1] == i as u8 + 1)
        );
        let peak = footprint();
        drop(blocks);
        // 400 MB not kept by the allocator (other tests run meanwhile: leave them some room).
        let after = footprint();
        assert!(
            peak.saturating_sub(after) > 250 * mb as u64,
            "{peak} -> {after}"
        );
    }

    #[test]
    fn blocks_grow_and_shrink_across_the_threshold() {
        for (from, to) in [
            (BIG / 2, BIG * 2),
            (BIG * 2, BIG / 2),
            (BIG, BIG * 3),
            (BIG * 3, BIG),
        ] {
            let mut v: Vec<u8> = (0..from).map(|i| i as u8).collect();
            v.resize(to, 7);
            v.shrink_to_fit();
            assert_eq!(v.len(), to);
            let kept = from.min(to);
            assert!(v[..kept].iter().enumerate().all(|(i, b)| *b == i as u8));
            assert!(v[kept..].iter().all(|b| *b == 7));
        }
        // Zeroed, mapped or not.
        for size in [BIG - 1, BIG, BIG * 2] {
            assert!(vec![0u8; size].iter().all(|b| *b == 0));
        }
        // Over-aligned blocks are left to the system allocator.
        let layout = Layout::from_size_align(BIG, MAP_ALIGN * 4).unwrap();
        assert!(!mapped(layout));
        // SAFETY: a valid layout, freed with it.
        unsafe {
            let p = Allocator.alloc(layout);
            assert!(!p.is_null() && (p as usize).is_multiple_of(MAP_ALIGN * 4));
            Allocator.dealloc(p, layout);
        }
    }
}
