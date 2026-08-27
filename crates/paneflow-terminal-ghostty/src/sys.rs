//! Process-global system hooks: image decoding, logging, and entropy.
//!
//! libghostty leaves a few operations to its embedder. Installing a PNG
//! decoder is what turns on PNG support in the Kitty Graphics Protocol;
//! installing a log sink surfaces the parser diagnostics that are otherwise
//! discarded. These are global and must be set before any terminal that
//! depends on them starts running.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{GhosttyError, Result};

/// Cap on a decoded image, matching the storage limit the constructor sets
/// for Kitty graphics. A decoder that reports more is refused rather than
/// handed to the library.
const MAX_DECODED_IMAGE_BYTES: usize = 320 * 1024 * 1024;

/// A decoded RGBA image.
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA pixels, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Decode PNG bytes into RGBA pixels, or return `None` on failure.
pub type PngDecoder = fn(&[u8]) -> Option<DecodedImage>;

/// Fill a buffer with cryptographically secure random bytes.
pub type SecureRandom = fn(&mut [u8]) -> bool;

/// Severity of a libghostty log message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    /// An error.
    Error,
    /// A warning.
    Warning,
    /// Informational.
    Info,
    /// Debug detail. Compiled out of release builds of the library.
    Debug,
}

/// Receive a log message: its level, its scope (empty when unscoped), and
/// its text.
pub type LogSink = fn(LogLevel, &str, &str);

// Function pointers are stored process-globally because that is the shape of
// the underlying API. `AtomicPtr` keeps installation and use race-free
// without pulling in a lock the callbacks would have to take on the render
// path.
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
    // SAFETY: the slot only ever holds a value written by `store` from the
    // matching `set_*` function, which transmutes exactly this `fn` type.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&raw) })
}

fn set_option(
    operation: &'static str,
    option: sys::GhosttySysOption,
    value: *const c_void,
) -> Result<()> {
    // SAFETY: `value` matches the input type documented for `option`: a
    // function pointer or null in every case this module uses.
    let result = unsafe { sys::ghostty_sys_set(option, value) };
    check(operation, result)
}

/// Install a PNG decoder, enabling PNG images in the Kitty Graphics
/// Protocol. Passing `None` disables PNG support.
///
/// This is process-global and should be called once at startup.
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

/// Override the source of secure random bytes. Passing `None` restores the
/// platform default, which is what every Paneflow target already has.
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

/// Route libghostty's internal logs to `sink`. Passing `None` discards them,
/// which is the default.
///
/// Release builds of the library never emit debug-level messages.
pub fn set_log_sink(sink: Option<LogSink>) -> Result<()> {
    store(&LOG_SINK, sink.map(|sink| sink as usize));
    let value = if sink.is_some() {
        log_trampoline as *const c_void
    } else {
        std::ptr::null()
    };
    set_option("sys_set_log", sys::GhosttySysOption_GHOSTTY_SYS_OPT_LOG, value)
}

/// Route libghostty's internal logs to stderr using the library's own
/// formatter, as `[level](scope): message`.
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
    // SAFETY: libghostty documents `data`/`data_len` as a borrowed slice
    // valid for the duration of this synchronous callback.
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

    // The pixel buffer must come from libghostty's allocator: the library
    // takes ownership and frees it with that same allocator.
    // SAFETY: `allocator` is the one libghostty handed this callback.
    let buffer = unsafe { sys::ghostty_alloc(allocator, image.rgba.len()) };
    if buffer.is_null() {
        return false;
    }
    // SAFETY: `buffer` is a fresh allocation of exactly `rgba.len()` bytes
    // and the source slice is live and non-overlapping.
    unsafe { std::ptr::copy_nonoverlapping(image.rgba.as_ptr(), buffer, image.rgba.len()) };
    // SAFETY: `out` is valid writable storage supplied by the library.
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
    // SAFETY: libghostty documents `buffer`/`len` as writable storage valid
    // for the duration of this synchronous callback.
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
        // An unknown level is still worth surfacing; treat it as info rather
        // than dropping the message.
        _ => LogLevel::Info,
    };
    // SAFETY: both pairs are borrowed slices valid for this synchronous
    // callback; a zero length may come with a null pointer.
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

/// Allocate `len` bytes from libghostty's default allocator.
///
/// Only useful for buffers the library will take ownership of. Anything the
/// caller keeps should use Rust's allocator instead.
///
/// # Safety
///
/// The returned pointer must be released with [`free`] using the same length,
/// or handed to a libghostty entry point that documents taking ownership.
pub unsafe fn alloc(len: usize) -> Result<*mut u8> {
    // SAFETY: a null allocator selects libghostty's default.
    let pointer = unsafe { sys::ghostty_alloc(std::ptr::null(), len) };
    if pointer.is_null() {
        return Err(GhosttyError::AbiMismatch(format!(
            "ghostty_alloc returned null for {len} bytes"
        )));
    }
    Ok(pointer)
}

/// Release a buffer obtained from [`alloc`].
///
/// # Safety
///
/// `pointer` and `len` must come from a matching [`alloc`] call on the
/// default allocator, and must not have been freed or given away.
pub unsafe fn free(pointer: *mut u8, len: usize) {
    // SAFETY: the caller guarantees the pointer and length pair.
    unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    /// These hooks are process-global, so the tests that install them have to
    /// run one at a time even though the harness runs tests in parallel.
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
        // Claims a 2x2 image but supplies one pixel.
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
        // SAFETY: the arguments mirror what libghostty passes: a live input
        // slice, its default allocator, and writable output storage.
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
        // SAFETY: as above.
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
        // SAFETY: the library would take ownership here; the test frees the
        // buffer with the same default allocator instead.
        unsafe { free(out.data, out.data_len) };
        store(&PNG_DECODER, None);
    }

    #[test]
    fn library_allocations_round_trip() {
        // SAFETY: the allocation is freed below with its exact length.
        let pointer = unsafe { alloc(64) }.expect("allocation must succeed");
        // SAFETY: `pointer` is a fresh 64-byte allocation.
        unsafe { std::ptr::write_bytes(pointer, 0, 64) };
        // SAFETY: the pointer and length match the allocation above.
        unsafe { free(pointer, 64) };
    }
}
