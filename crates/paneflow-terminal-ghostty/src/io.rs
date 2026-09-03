use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

unsafe extern "C" fn write_trampoline<F: FnMut(&[u8]) -> bool>(
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) -> bool {
    if userdata.is_null() || data.is_null() {
        return false;
    }
    let sink = unsafe { &mut *userdata.cast::<F>() };
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(bytes))).unwrap_or(false)
}

pub(crate) fn writer<F: FnMut(&[u8]) -> bool>(sink: &mut F) -> sys::GhosttyWriter {
    sys::GhosttyWriter {
        write: Some(write_trampoline::<F>),
        userdata: (sink as *mut F).cast::<c_void>(),
    }
}

unsafe extern "C" fn read_trampoline<F: FnMut(&mut [u8]) -> Option<usize>>(
    userdata: *mut c_void,
    buffer: *mut u8,
    capacity: usize,
    out_read: *mut usize,
) -> bool {
    if userdata.is_null() || buffer.is_null() || out_read.is_null() {
        return false;
    }
    let source = unsafe { &mut *userdata.cast::<F>() };
    let destination = unsafe { std::slice::from_raw_parts_mut(buffer, capacity) };
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source(destination)));
    match read {
        Ok(Some(read)) if read <= capacity => {
            unsafe { *out_read = read };
            true
        }
        _ => false,
    }
}

pub(crate) fn reader<F: FnMut(&mut [u8]) -> Option<usize>>(source: &mut F) -> sys::GhosttyReader {
    sys::GhosttyReader {
        read: Some(read_trampoline::<F>),
        userdata: (source as *mut F).cast::<c_void>(),
    }
}
