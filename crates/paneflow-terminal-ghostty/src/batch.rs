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
        return Err(GhosttyError::BatchRead {
            operation,
            detail: format!("key {failing} returned {result} ({written} of {N} written)"),
        });
    }
    if written != N {
        return Err(GhosttyError::BatchRead {
            operation,
            detail: format!("{written} of {N} values written"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn rejecting_get_multi(
        _: usize,
        _: usize,
        _: *const u32,
        _: *mut *mut c_void,
        written: *mut usize,
    ) -> sys::GhosttyResult {
        unsafe { *written = 1 };
        sys::GhosttyResult_GHOSTTY_NO_VALUE
    }

    #[test]
    fn a_failed_batch_read_is_not_reported_as_an_abi_mismatch() {
        let mut first = 0u32;
        let mut second = 0u32;
        let slots = unsafe { [Slot::new(7u32, &mut first), Slot::new(9u32, &mut second)] };
        let error = unsafe { get_multi("test_get_multi", 0usize, rejecting_get_multi, slots) }
            .expect_err("a rejected batch read must fail");

        let message = error.to_string();
        assert!(matches!(error, GhosttyError::BatchRead { .. }), "{error:?}");
        assert!(!message.contains("ABI"), "{message}");
        assert!(message.contains("test_get_multi"), "{message}");
        assert!(message.contains("key 9"), "{message}");
    }
}
