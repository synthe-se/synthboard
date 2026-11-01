//! Wipes every block of native memory as it is freed. Memory tagging turns off
//! the zeroing `hardened_malloc` would do, and text passes through too many short
//! copies to chase one by one, so the allocator is the one place that sees them
//! all. It wraps the system allocator: `hardened_malloc` still does the real work.

use std::alloc::{GlobalAlloc, Layout};
use std::hint::black_box;

/// An allocator that zeroes what it frees. Growing a block goes through the
/// default `realloc`, which frees the old block here too.
#[derive(Debug)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct Wiping<A>(pub A);

// SAFETY: every method forwards to the inner allocator with its own arguments.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Wiping<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is passed on unchanged.
        unsafe { self.0.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is passed on unchanged.
        unsafe { self.0.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        // SAFETY: the block is live and `layout.size()` bytes long until the
        // inner allocator frees it. `black_box` keeps the compiler from
        // removing a store to memory it knows is about to be freed.
        unsafe {
            block.write_bytes(0, layout.size());
            self.0.dealloc(black_box(block), layout);
        }
    }
}

#[cfg(target_os = "android")]
#[global_allocator]
static ALLOCATOR: Wiping<std::alloc::System> = Wiping(std::alloc::System);

// ONNX Runtime is C++ and allocates behind the Rust allocator, so its blocks, which
// hold the gesture, are wiped where every one of them ends: `free`. The Gradle task
// links with `--wrap=free`, which sends the calls of this library here. The Rust
// allocator above stays too: it knows exact sizes, and works without that flag.
#[cfg(target_os = "android")]
mod native {
    use std::ffi::c_void;
    use std::hint::black_box;

    unsafe extern "C" {
        fn __real_free(block: *mut c_void);
        fn malloc_usable_size(block: *const c_void) -> usize;
    }

    /// Zeroes a block, then frees it.
    ///
    /// # Safety
    /// The contract of `free`: the linker sends its calls here.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_free(block: *mut c_void) {
        if !block.is_null() {
            // SAFETY: a live block from `malloc` has this many usable bytes.
            unsafe { block.cast::<u8>().write_bytes(0, malloc_usable_size(block)) };
        }
        // SAFETY: the caller's contract is passed on unchanged.
        unsafe { __real_free(black_box(block)) };
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::System;
    use std::cell::Cell;

    use super::*;

    /// Records what a block holds when it reaches the system allocator.
    struct Witness(Cell<u8>);

    // SAFETY: forwards to the system allocator, and reads a block still live.
    unsafe impl GlobalAlloc for Witness {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: passed on unchanged.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
            // SAFETY: the block is still live here.
            unsafe {
                self.0.set(block.read());
                System.dealloc(block, layout);
            }
        }
    }

    #[test]
    fn a_freed_block_holds_nothing() {
        let allocator = Wiping(Witness(Cell::new(1)));
        let layout = Layout::new::<u64>();
        // SAFETY: the block comes from this allocator with this layout.
        unsafe {
            let block = allocator.alloc(layout);
            block.write(42);
            allocator.dealloc(block, layout);
        }
        assert_eq!(allocator.0.0.get(), 0);
    }
}
