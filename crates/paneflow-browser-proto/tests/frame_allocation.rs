#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use paneflow_browser_proto::limits::MAX_MESSAGE_BYTES;
use paneflow_browser_proto::{FrameError, WindowMessage, decode, read_message};

struct LargestAllocation;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}
static LARGEST: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for LargestAllocation {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING.with(Cell::get) {
            LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACKING.with(Cell::get) {
            LARGEST.fetch_max(new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: LargestAllocation = LargestAllocation;

#[test]
fn a_frame_over_the_limit_fails_to_decode_without_allocating_its_declared_length() {
    let declared = u32::MAX as usize;
    let mut framed = (declared as u32).to_le_bytes().to_vec();
    framed.extend_from_slice(&[b'{'; 64]);

    LARGEST.store(0, Ordering::Relaxed);
    TRACKING.with(|tracking| tracking.set(true));
    let streamed = read_message::<WindowMessage, _>(&mut framed.as_slice());
    let buffered = decode::<WindowMessage>(&framed);
    TRACKING.with(|tracking| tracking.set(false));

    assert!(matches!(
        streamed,
        Err(FrameError::TooLarge { declared: d, max }) if d == declared && max == MAX_MESSAGE_BYTES
    ));
    assert!(matches!(buffered, Err(FrameError::TooLarge { .. })));
    let largest = LARGEST.load(Ordering::Relaxed);
    assert!(
        largest < 1024,
        "decoding allocated {largest} bytes for a refused frame"
    );
}

#[test]
fn a_frame_just_over_the_limit_is_refused() {
    let declared = MAX_MESSAGE_BYTES + 1;
    let framed = (declared as u32).to_le_bytes();
    assert!(matches!(
        read_message::<WindowMessage, _>(&mut framed.as_slice()),
        Err(FrameError::TooLarge { declared: d, .. }) if d == declared
    ));
}
