use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::{GhosttyError, Result};

#[derive(Clone, Copy)]
pub(crate) struct Slot<K: Copy> {
    key: K,
    value: *mut c_void,
}

impl<K: Copy> Slot<K> {
    pub(crate) unsafe fn new<T>(key: K, destination: &mut T) -> Self {
        Self {
            key,
            value: (destination as *mut T).cast(),
        }
    }
}

pub(crate) type GetMultiFn<H, K> =
    unsafe extern "C" fn(H, usize, *const K, *mut *mut c_void, *mut usize) -> sys::GhosttyResult;

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
    let result = unsafe { call(handle, N, keys.as_ptr(), values.as_mut_ptr(), &mut written) };
    if result != sys::GhosttyResult_GHOSTTY_SUCCESS {
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
