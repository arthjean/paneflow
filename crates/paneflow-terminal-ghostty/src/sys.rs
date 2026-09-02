use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{GhosttyError, Result};

const MAX_DECODED_IMAGE_BYTES: usize = 320 * 1024 * 1024;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub type PngDecoder = fn(&[u8]) -> Option<DecodedImage>;

pub type SecureRandom = fn(&mut [u8]) -> bool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warning,
    Info,
    Debug,
}

pub type LogSink = fn(LogLevel, &str, &str);

static PNG_DECODER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static SECURE_RANDOM: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static LOG_SINK: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn store(slot: &AtomicPtr<c_void>, value: Option<usize>) {
    slot.store(
        value.map_or(std::ptr::null_mut(), |value| value as *mut c_void),
        Ordering::Release,
    );
}

fn load<T: Copy>(slot: &AtomicPtr<c_void>) -> Option<T> {
    let raw = slot.load(Ordering::Acquire);
    if raw.is_null() {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&raw) })
}

fn set_option(
    operation: &'static str,
    option: sys::GhosttySysOption,
    value: *const c_void,
) -> Result<()> {
    let result = unsafe { sys::ghostty_sys_set(option, value) };
    check(operation, result)
}

pub fn set_png_decoder(decoder: Option<PngDecoder>) -> Result<()> {
    store(&PNG_DECODER, decoder.map(|decoder| decoder as usize));
    let value = if decoder.is_some() {
        decode_png_trampoline as *const c_void
    } else {
        std::ptr::null()
    };
    set_option(
        "sys_set_decode_png",
        sys::GhosttySysOption_GHOSTTY_SYS_OPT_DECODE_PNG,
        value,
    )
}

pub fn set_secure_random(source: Option<SecureRandom>) -> Result<()> {
    store(&SECURE_RANDOM, source.map(|source| source as usize));
    let value = if source.is_some() {
        random_secure_trampoline as *const c_void
    } else {
        std::ptr::null()
    };
    set_option(
        "sys_set_random_secure",
        sys::GhosttySysOption_GHOSTTY_SYS_OPT_RANDOM_SECURE,
        value,
    )
}

pub fn set_log_sink(sink: Option<LogSink>) -> Result<()> {
    store(&LOG_SINK, sink.map(|sink| sink as usize));
    let value = if sink.is_some() {
        log_trampoline as *const c_void
    } else {
        std::ptr::null()
    };
    set_option("sys_set_log", sys::GhosttySysOption_GHOSTTY_SYS_OPT_LOG, value)
}

pub fn set_log_to_stderr() -> Result<()> {
    store(&LOG_SINK, None);
    set_option(
        "sys_set_log",
        sys::GhosttySysOption_GHOSTTY_SYS_OPT_LOG,
        sys::ghostty_sys_log_stderr as *const c_void,
    )
}

unsafe extern "C" fn decode_png_trampoline(
    _userdata: *mut c_void,
    allocator: *const sys::GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut sys::GhosttySysImage,
) -> bool {
    let Some(decoder) = load::<PngDecoder>(&PNG_DECODER) else {
        return false;
    };
    if data.is_null() || out.is_null() {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, data_len) };
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder(bytes)));
    let Ok(Some(image)) = decoded else {
        return false;
    };
    let expected = usize::try_from(image.width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(image.height).ok()?))
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(image.rgba.len()) || image.rgba.len() > MAX_DECODED_IMAGE_BYTES {
        return false;
    }

    let buffer = unsafe { sys::ghostty_alloc(allocator, image.rgba.len()) };
    if buffer.is_null() {
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(image.rgba.as_ptr(), buffer, image.rgba.len()) };
    unsafe {
        *out = sys::GhosttySysImage {
            width: image.width,
            height: image.height,
            data: buffer,
            data_len: image.rgba.len(),
        };
    }
    true
}

unsafe extern "C" fn random_secure_trampoline(
    _userdata: *mut c_void,
    buffer: *mut u8,
    len: usize,
) -> bool {
    let Some(source) = load::<SecureRandom>(&SECURE_RANDOM) else {
        return false;
    };
    if buffer.is_null() || len == 0 {
        return false;
    }
    let slice = unsafe { std::slice::from_raw_parts_mut(buffer, len) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source(slice))).unwrap_or(false)
}

unsafe extern "C" fn log_trampoline(
    _userdata: *mut c_void,
    level: sys::GhosttySysLogLevel,
    scope: *const u8,
    scope_len: usize,
    message: *const u8,
    message_len: usize,
) {
    let Some(sink) = load::<LogSink>(&LOG_SINK) else {
        return;
    };
    let level = match level {
        sys::GhosttySysLogLevel_GHOSTTY_SYS_LOG_LEVEL_ERROR => LogLevel::Error,
        sys::GhosttySysLogLevel_GHOSTTY_SYS_LOG_LEVEL_WARNING => LogLevel::Warning,
        sys::GhosttySysLogLevel_GHOSTTY_SYS_LOG_LEVEL_DEBUG => LogLevel::Debug,
        _ => LogLevel::Info,
    };
    let text = |pointer: *const u8, len: usize| -> &str {
        if pointer.is_null() || len == 0 {
            return "";
        }
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer, len) }).unwrap_or("")
    };
    let scope = text(scope, scope_len);
    let message = text(message, message_len);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(level, scope, message)));
}

pub unsafe fn alloc(len: usize) -> Result<*mut u8> {
    let pointer = unsafe { sys::ghostty_alloc(std::ptr::null(), len) };
    if pointer.is_null() {
        return Err(GhosttyError::AbiMismatch(format!(
            "ghostty_alloc returned null for {len} bytes"
        )));
    }
    Ok(pointer)
}

pub unsafe fn free(pointer: *mut u8, len: usize) {
    unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    static GLOBAL_HOOKS: Mutex<()> = Mutex::new(());

    fn exclusive() -> MutexGuard<'static, ()> {
        GLOBAL_HOOKS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fake_png(data: &[u8]) -> Option<DecodedImage> {
        if data.is_empty() {
            return None;
        }
        Some(DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        })
    }

    fn lying_png(_: &[u8]) -> Option<DecodedImage> {
        Some(DecodedImage {
            width: 2,
            height: 2,
            rgba: vec![0; 4],
        })
    }

    #[test]
    fn installing_and_clearing_the_hooks_round_trips() {
        let _guard = exclusive();
        set_png_decoder(Some(fake_png)).expect("decoder must install");
        assert!(load::<PngDecoder>(&PNG_DECODER).is_some());
        set_png_decoder(None).expect("decoder must clear");
        assert!(load::<PngDecoder>(&PNG_DECODER).is_none());

        set_log_to_stderr().expect("stderr log must install");
        set_log_sink(None).expect("log must clear");
        set_secure_random(None).expect("random must reset");
    }

    #[test]
    fn the_decode_trampoline_refuses_a_mismatched_pixel_count() {
        let _guard = exclusive();
        store(&PNG_DECODER, Some(lying_png as PngDecoder as usize));
        let data = [0u8; 4];
        let mut out = sys::GhosttySysImage {
            width: 0,
            height: 0,
            data: std::ptr::null_mut(),
            data_len: 0,
        };
        let accepted = unsafe {
            decode_png_trampoline(
                std::ptr::null_mut(),
                std::ptr::null(),
                data.as_ptr(),
                data.len(),
                &mut out,
            )
        };
        assert!(!accepted);
        assert!(out.data.is_null());
        store(&PNG_DECODER, None);
    }

    #[test]
    fn the_decode_trampoline_allocates_through_the_library() {
        let _guard = exclusive();
        store(&PNG_DECODER, Some(fake_png as PngDecoder as usize));
        let data = [0u8; 4];
        let mut out = sys::GhosttySysImage {
            width: 0,
            height: 0,
            data: std::ptr::null_mut(),
            data_len: 0,
        };
        let accepted = unsafe {
            decode_png_trampoline(
                std::ptr::null_mut(),
                std::ptr::null(),
                data.as_ptr(),
                data.len(),
                &mut out,
            )
        };
        assert!(accepted);
        assert_eq!((out.width, out.height, out.data_len), (1, 1, 4));
        assert!(!out.data.is_null());
        unsafe { free(out.data, out.data_len) };
        store(&PNG_DECODER, None);
    }

    #[test]
    fn library_allocations_round_trip() {
        let pointer = unsafe { alloc(64) }.expect("allocation must succeed");
        unsafe { std::ptr::write_bytes(pointer, 0, 64) };
        unsafe { free(pointer, 64) };
    }
}
