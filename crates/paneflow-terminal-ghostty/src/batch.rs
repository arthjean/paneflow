//! Batched multi-key reads over libghostty's `*_get_multi` entry points.
//!
//! Every libghostty handle that exposes a keyed `get` also exposes a
//! `get_multi` with the same shape: an array of keys and a parallel array of
//! destination pointers, filled in one call. One round trip instead of one
//! per field matters on the snapshot path, which runs per cell per frame.
//!
//! The slot count is a const parameter so the key and pointer arrays live on
//! the stack: a heap allocation per cell would cost more than the FFI calls
//! this saves.
//!
//! On error the library reports how many values it managed to write, so a
//! failure names the key that rejected rather than leaving the batch opaque.

use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::{GhosttyError, Result};

/// One key and the storage its value lands in.
#[derive(Clone, Copy)]
pub(crate) struct Slot<K: Copy> {
    key: K,
    value: *mut c_void,
}

impl<K: Copy> Slot<K> {
    /// Bind `key` to `destination`.
    ///
    /// # Safety
    ///
    /// `destination` must have exactly the output type libghostty documents
    /// for `key`, and must stay live until the batch call returns.
    pub(crate) unsafe fn new<T>(key: K, destination: &mut T) -> Self {
        Self {
            key,
            value: (destination as *mut T).cast(),
        }
    }
}

/// The `get_multi` shape shared by every keyed libghostty handle.
pub(crate) type GetMultiFn<H, K> =
    unsafe extern "C" fn(H, usize, *const K, *mut *mut c_void, *mut usize) -> sys::GhosttyResult;

/// Run one batched read.
///
/// # Safety
///
/// `handle` must be live, `call` must be the `get_multi` belonging to that
/// handle's key type, and every slot must satisfy [`Slot::new`]'s contract.
pub(crate) unsafe fn get_multi<H: Copy, K: Copy + std::fmt::Debug, const N: usize>(
    operation: &'static str,
    handle: H,
    call: GetMultiFn<H, K>,
    slots: [Slot<K>; N],
) -> Result<()> {
    if N == 0 {
        return Ok(());
    }
    let keys = slots.map(|slot| slot.key);
    let mut values = slots.map(|slot| slot.value);
    let mut written = 0usize;
    // SAFETY: the two arrays hold exactly `N` entries each, the caller
    // guarantees the handle and destination types, and `written` is valid
    // writable storage.
    let result = unsafe { call(handle, N, keys.as_ptr(), values.as_mut_ptr(), &mut written) };
    if result != sys::GhosttyResult_GHOSTTY_SUCCESS {
        // `written` is the index of the key that failed, which is far more
        // useful than the bare result code.
        let failing = keys
            .get(written)
            .map_or_else(|| "unknown".to_owned(), |key| format!("{key:?}"));
        return Err(GhosttyError::AbiMismatch(format!(
            "{operation} failed at key {failing} (result {result}, {written} of {N} written)"
        )));
    }
    if written != N {
        return Err(GhosttyError::AbiMismatch(format!(
            "{operation} wrote {written} of {N} values"
        )));
    }
    Ok(())
}
