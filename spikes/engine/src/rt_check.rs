// SPDX-License-Identifier: GPL-3.0-or-later
//! Detects heap allocation on real-time threads.
//!
//! A thread marks itself real-time with [`RtGuard`]. Any allocation,
//! reallocation or free while the guard is alive is counted for that
//! thread. Tests assert the count stays at zero.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // Const-initialised and without Drop, so touching these never
    // allocates and is safe inside the allocator.
    static IN_RT: Cell<bool> = const { Cell::new(false) };
    static VIOLATIONS: Cell<usize> = const { Cell::new(0) };
}

pub struct CheckedAlloc;

impl CheckedAlloc {
    #[inline]
    fn note() {
        let _ = IN_RT.try_with(|rt| {
            if rt.get() {
                let _ = VIOLATIONS.try_with(|v| v.set(v.get() + 1));
            }
        });
    }
}

unsafe impl GlobalAlloc for CheckedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::note();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::note();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CheckedAlloc = CheckedAlloc;

/// Marks the current thread as real-time until dropped.
pub struct RtGuard(());

impl RtGuard {
    pub fn enter() -> Self {
        IN_RT.with(|rt| rt.set(true));
        RtGuard(())
    }
}

impl Drop for RtGuard {
    fn drop(&mut self) {
        IN_RT.with(|rt| rt.set(false));
    }
}

/// Allocator calls counted on this thread while it was marked real-time.
pub fn violations() -> usize {
    VIOLATIONS.with(|v| v.get())
}
