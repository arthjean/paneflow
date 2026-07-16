use paneflow_libghostty_sys as sys;

use crate::{GhosttyError, Result};

pub(crate) struct OwnedHandle<T: Copy> {
    raw: T,
    free: unsafe extern "C" fn(T),
}

impl<T: Copy> OwnedHandle<T> {
    /// Take unique ownership of a raw libghostty handle.
    ///
    /// # Safety
    ///
    /// `raw` must be a live, non-null handle owned by the caller, and `free`
    /// must be the exact destructor for that handle type and allocator. No
    /// other owner may use or free `raw` after this call.
    pub(crate) unsafe fn from_raw(raw: T, free: unsafe extern "C" fn(T)) -> Self {
        Self { raw, free }
    }

    pub(crate) fn raw(&self) -> T {
        self.raw
    }
}

impl<T: Copy> Drop for OwnedHandle<T> {
    fn drop(&mut self) {
        // SAFETY: constructors only create this owner after libghostty returns
        // a non-null handle. `raw` is private and Drop runs exactly once.
        unsafe { (self.free)(self.raw) };
    }
}

/// Create and uniquely own a libghostty handle using its default allocator.
///
/// # Safety
///
/// `create` must initialize an out-parameter of exactly type `T` and allocate
/// the resulting handle with libghostty's default allocator. `free` must be
/// the exact matching destructor for every non-null handle produced by
/// `create`, and the caller must not retain another owner of that handle.
pub(crate) unsafe fn create<T: Copy + Default + PartialEq>(
    operation: &'static str,
    create: unsafe extern "C" fn(*const sys::GhosttyAllocator, *mut T) -> sys::GhosttyResult,
    free: unsafe extern "C" fn(T),
) -> Result<OwnedHandle<T>> {
    let mut raw = T::default();
    // SAFETY: `raw` is valid writable storage, and a null allocator selects
    // libghostty's default allocator for both construction and destruction.
    let result = unsafe { create(std::ptr::null(), &mut raw) };
    check(operation, result)?;
    if raw == T::default() {
        return Err(GhosttyError::AbiMismatch(format!(
            "{operation} returned a null handle"
        )));
    }
    Ok(OwnedHandle { raw, free })
}

pub(crate) fn check(operation: &'static str, result: sys::GhosttyResult) -> Result<()> {
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(GhosttyError::Ffi {
            operation,
            code: result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static DROPS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    unsafe extern "C" fn record_drop(raw: *mut usize) {
        let value = unsafe { *raw };
        DROPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(value);
        unsafe { drop(Box::from_raw(raw)) };
    }

    fn fake_handle(value: usize) -> OwnedHandle<*mut usize> {
        OwnedHandle {
            raw: Box::into_raw(Box::new(value)),
            free: record_drop,
        }
    }

    unsafe extern "C" fn create_null(
        _: *const sys::GhosttyAllocator,
        out: *mut *mut usize,
    ) -> sys::GhosttyResult {
        unsafe { *out = std::ptr::null_mut() };
        sys::GhosttyResult_GHOSTTY_SUCCESS
    }

    #[test]
    fn handles_free_exactly_once_in_reverse_initialization_order() {
        DROPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let result: Result<()> = (|| {
            let _terminal = fake_handle(1);
            let _render_state = fake_handle(2);
            let _row_iterator = fake_handle(3);
            Err(GhosttyError::AbiMismatch("forced partial init".into()))
        })();

        assert!(result.is_err());
        assert_eq!(
            *DROPS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [3, 2, 1]
        );
    }

    #[test]
    fn successful_constructor_rejects_a_null_handle() {
        // SAFETY: `create_null` has the expected `*mut usize` out-parameter,
        // and any non-null handle it could return would come from `Box` and
        // therefore have the matching `record_drop` destructor.
        let result = unsafe { create("fake_new", create_null, record_drop) };
        assert!(matches!(result, Err(GhosttyError::AbiMismatch(_))));
    }
}
