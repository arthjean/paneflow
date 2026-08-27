//! Bridges between Rust closures and libghostty's byte streams.
//!
//! `GhosttyWriter` and `GhosttyReader` are the two callback structs libghostty
//! uses whenever an operation streams bytes instead of materializing them.
//! Both are called synchronously from inside the operation they are handed to,
//! so a borrowed closure is enough unless the owning handle outlives the call,
//! as the snapshot decoder's reader does.

use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

/// Feed one chunk into the Rust sink behind a `GhosttyWriter`.
unsafe extern "C" fn write_trampoline<F: FnMut(&[u8]) -> bool>(
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) -> bool {
    if userdata.is_null() || data.is_null() {
        return false;
    }
    // SAFETY: `userdata` is the `&mut F` [`writer`] captured, and libghostty
    // calls this synchronously while that borrow lives.
    let sink = unsafe { &mut *userdata.cast::<F>() };
    // SAFETY: libghostty documents `data`/`len` as a borrowed slice valid for
    // the duration of the callback.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(bytes))).unwrap_or(false)
}

/// Wrap `sink` in a `GhosttyWriter`, which returns `false` to abort output.
///
/// The writer borrows `sink` through a raw pointer, so it must only be handed
/// to a call that finishes before the borrow ends.
pub(crate) fn writer<F: FnMut(&[u8]) -> bool>(sink: &mut F) -> sys::GhosttyWriter {
    sys::GhosttyWriter {
        write: Some(write_trampoline::<F>),
        userdata: (sink as *mut F).cast::<c_void>(),
    }
}

/// Fill one buffer from the Rust source behind a `GhosttyReader`.
unsafe extern "C" fn read_trampoline<F: FnMut(&mut [u8]) -> Option<usize>>(
    userdata: *mut c_void,
    buffer: *mut u8,
    capacity: usize,
    out_read: *mut usize,
) -> bool {
    if userdata.is_null() || buffer.is_null() || out_read.is_null() {
        return false;
    }
    // SAFETY: `userdata` is the `&mut F` [`reader`] captured, and the caller
    // keeps it alive for as long as libghostty may call back into it.
    let source = unsafe { &mut *userdata.cast::<F>() };
    // SAFETY: libghostty documents `buffer`/`capacity` as writable for the
    // duration of the callback.
    let destination = unsafe { std::slice::from_raw_parts_mut(buffer, capacity) };
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source(destination)));
    // A source that reports more than it was offered would leave libghostty
    // reading uninitialized bytes, so it is treated as an I/O failure.
    match read {
        Ok(Some(read)) if read <= capacity => {
            // SAFETY: `out_read` is libghostty's own `size_t` out-parameter.
            unsafe { *out_read = read };
            true
        }
        _ => false,
    }
}

/// Wrap `source` in a `GhosttyReader`.
///
/// The closure returns how many bytes it wrote, `Some(0)` for end of input, or
/// `None` to report an I/O error. The reader borrows `source` through a raw
/// pointer, so `source` must outlive every call libghostty makes through it.
pub(crate) fn reader<F: FnMut(&mut [u8]) -> Option<usize>>(source: &mut F) -> sys::GhosttyReader {
    sys::GhosttyReader {
        read: Some(read_trampoline::<F>),
        userdata: (source as *mut F).cast::<c_void>(),
    }
}
