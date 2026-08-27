use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::callbacks::with_state;
use crate::osc7::working_directory_from_ghostty;
use crate::{BackendEvent, ColorScheme, ProgressReport, ProgressState};

const MAX_CALLBACK_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 4096;
pub(crate) const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;
const EMPTY_RESPONSE: &[u8] = b"";

pub(crate) unsafe extern "C" fn write_pty(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            if len > MAX_CALLBACK_BYTES || (len > 0 && data.is_null()) {
                state.push(BackendEvent::InputDropped { bytes: len });
                return;
            }
            let bytes = if len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data, len).to_vec()
            };
            state.push(BackendEvent::WritePty(bytes));
        });
    }
}

pub(crate) unsafe extern "C" fn bell(_: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe { with_state(userdata, |state| state.push(BackendEvent::Bell)) };
}

pub(crate) unsafe extern "C" fn title_changed(
    terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let mut title = sys::GhosttyString {
                ptr: std::ptr::null(),
                len: 0,
            };
            let result = sys::ghostty_terminal_get(
                terminal,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE,
                (&mut title as *mut sys::GhosttyString).cast(),
            );
            if result != sys::GhosttyResult_GHOSTTY_SUCCESS
                || title.len > MAX_METADATA_BYTES
                || (title.len > 0 && title.ptr.is_null())
            {
                return;
            }
            let bytes = if title.len == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(title.ptr, title.len)
            };
            state.push(BackendEvent::Title(
                String::from_utf8_lossy(bytes).into_owned(),
            ));
        });
    }
}

pub(crate) unsafe extern "C" fn pwd_changed(
    terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let mut pwd = sys::GhosttyString {
                ptr: std::ptr::null(),
                len: 0,
            };
            let result = sys::ghostty_terminal_get(
                terminal,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD,
                (&raw mut pwd).cast(),
            );
            if result != sys::GhosttyResult_GHOSTTY_SUCCESS
                || pwd.len == 0
                || pwd.len > MAX_METADATA_BYTES
                || pwd.ptr.is_null()
            {
                return;
            }
            // The terminal stores the bytes the shell emitted verbatim, so
            // OSC 9 and OSC 1337 deliver a bare path here. Paneflow only
            // trusts the OSC 7 `file://` form, which carries the host.
            let bytes = std::slice::from_raw_parts(pwd.ptr, pwd.len);
            let Ok(raw) = std::str::from_utf8(bytes) else {
                return;
            };
            let Some(cwd) = working_directory_from_ghostty(raw) else {
                return;
            };
            state.push_working_directory(cwd);
        });
    }
}

pub(crate) unsafe extern "C" fn clipboard_write(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let Some(request) = write.as_ref() else {
                return;
            };
            if request.size < size_of::<sys::GhosttyClipboardWrite>() {
                return;
            }
            let result = store_clipboard_write(state, request);
            let reply = sys::GhosttyClipboardWriteReply {
                size: size_of::<sys::GhosttyClipboardWriteReply>(),
                result,
                remember: false,
            };
            if let Some(answer) = request.reply {
                answer(write, &reply);
            }
        });
    }
}

/// Store the text representation of an atomic clipboard write.
///
/// # Safety
///
/// `request` must be a live clipboard write request whose reported `size`
/// covers `contents` and `contents_len`, borrowed for the callback duration.
unsafe fn store_clipboard_write(
    state: &crate::callbacks::CallbackState,
    request: &sys::GhosttyClipboardWrite,
) -> sys::GhosttyClipboardWriteResult {
    // A zero-length contents array clears the destination, which Paneflow
    // stores as an empty selection just like an empty OSC 52 payload.
    if request.contents_len == 0 {
        state.push(BackendEvent::ClipboardStore(String::new()));
        return sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    }
    if request.contents.is_null() {
        return sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }
    let contents = unsafe { std::slice::from_raw_parts(request.contents, request.contents_len) };
    for content in contents {
        if !unsafe { is_text_mime(content.mime) } {
            continue;
        }
        if content.data.len > MAX_CLIPBOARD_BYTES {
            return sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
        }
        let data = if content.data.len == 0 {
            &[][..]
        } else if content.data.ptr.is_null() {
            return sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
        } else {
            unsafe { std::slice::from_raw_parts(content.data.ptr, content.data.len) }
        };
        let Ok(text) = std::str::from_utf8(data) else {
            return sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
        };
        state.push(BackendEvent::ClipboardStore(text.to_owned()));
        return sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    }
    sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED
}

/// # Safety
///
/// `mime` must be a borrowed string valid for the callback duration.
unsafe fn is_text_mime(mime: sys::GhosttyString) -> bool {
    if mime.len == 0 || mime.ptr.is_null() {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(mime.ptr, mime.len) };
    bytes.starts_with(b"text/")
}

pub(crate) unsafe extern "C" fn desktop_notification(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const sys::GhosttyTerminalDesktopNotification,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let Some(request) = notification.as_ref() else {
                return;
            };
            if request.size < size_of::<sys::GhosttyTerminalDesktopNotification>() {
                return;
            }
            // OSC 9 carries a body and no title, so an empty title is normal
            // rather than a malformed request.
            let (Some(title), Some(body)) = (
                borrowed_text(request.title, MAX_METADATA_BYTES),
                borrowed_text(request.body, MAX_METADATA_BYTES),
            ) else {
                return;
            };
            if body.is_empty() {
                return;
            }
            state.push(BackendEvent::DesktopNotification { title, body });
        });
    }
}

pub(crate) unsafe extern "C" fn unknown_sequence(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    sequence: *const sys::GhosttyTerminalUnknownSequence,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let Some(report) = sequence.as_ref() else {
                return;
            };
            // APC is the only tag libghostty reports today, and the value is a
            // union: reading the wrong arm for a tag added later would be
            // reading the wrong type.
            if report.tag != sys::GhosttyTerminalUnknownSequenceTag_GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_APC
            {
                return;
            }
            let apc = report.value.apc;
            let Some(raw) = borrowed_bytes(apc.content, MAX_CALLBACK_BYTES) else {
                return;
            };
            state.push(BackendEvent::UnknownSequence {
                content: escape_content(raw),
                truncated: apc.truncated,
            });
        });
    }
}

pub(crate) unsafe extern "C" fn clipboard_read(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    read: *const sys::GhosttyClipboardRead,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let Some(request) = read.as_ref() else {
                return;
            };
            if request.size < size_of::<sys::GhosttyClipboardRead>() {
                return;
            }
            let Some(answer) = request.reply else {
                return;
            };
            // Denied is the default: a program that can read the clipboard can
            // exfiltrate whatever the user last copied anywhere on the system.
            let Some(text) = state.readable_clipboard() else {
                answer(read, &denied_read());
                return;
            };
            let content = sys::GhosttyClipboardContent {
                mime: string_of(TEXT_MIME),
                data: string_of(text.as_bytes()),
            };
            let available = string_of(TEXT_MIME);
            let reply = sys::GhosttyClipboardReadReply {
                size: size_of::<sys::GhosttyClipboardReadReply>(),
                result: sys::GhosttyClipboardReadResult_GHOSTTY_CLIPBOARD_READ_RESULT_SUCCESS,
                contents: &content,
                contents_len: 1,
                available: &available,
                available_len: 1,
                // Remembering would let one grant answer every later read from
                // the same program, which is the opposite of a per-read gate.
                remember: false,
            };
            answer(read, &reply);
        });
    }
}

/// The single representation Paneflow answers a clipboard read with.
const TEXT_MIME: &[u8] = b"text/plain;charset=utf-8";

fn denied_read() -> sys::GhosttyClipboardReadReply {
    sys::GhosttyClipboardReadReply {
        size: size_of::<sys::GhosttyClipboardReadReply>(),
        result: sys::GhosttyClipboardReadResult_GHOSTTY_CLIPBOARD_READ_RESULT_DENIED,
        contents: std::ptr::null(),
        contents_len: 0,
        available: std::ptr::null(),
        available_len: 0,
        remember: false,
    }
}

fn string_of(bytes: &[u8]) -> sys::GhosttyString {
    sys::GhosttyString {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

/// Borrow a libghostty string as bytes, rejecting anything over `limit`.
///
/// # Safety
///
/// `text` must be borrowed for at least the duration of the callback.
unsafe fn borrowed_bytes(text: sys::GhosttyString, limit: usize) -> Option<&'static [u8]> {
    if text.len == 0 {
        return Some(&[]);
    }
    if text.ptr.is_null() || text.len > limit {
        return None;
    }
    // SAFETY: the caller guarantees the borrow, and the returned slice is
    // consumed before the callback returns.
    Some(unsafe { std::slice::from_raw_parts(text.ptr, text.len) })
}

/// # Safety
///
/// `text` must be borrowed for at least the duration of the callback.
unsafe fn borrowed_text(text: sys::GhosttyString, limit: usize) -> Option<String> {
    // SAFETY: forwarded from this function's own contract.
    let bytes = unsafe { borrowed_bytes(text, limit) }?;
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

/// Render a captured sequence for a log line.
///
/// The payload is attacker-controlled, so control characters are escaped
/// rather than passed through to a terminal or a log viewer, and invalid
/// UTF-8 becomes the replacement character instead of being dropped.
fn escape_content(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                format!("\\x{:02x}", character as u32).chars().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

pub(crate) unsafe extern "C" fn enquiry(
    _: sys::GhosttyTerminal,
    _: *mut c_void,
) -> sys::GhosttyString {
    sys::GhosttyString {
        ptr: EMPTY_RESPONSE.as_ptr(),
        len: EMPTY_RESPONSE.len(),
    }
}

pub(crate) unsafe extern "C" fn xtversion(
    _: sys::GhosttyTerminal,
    _: *mut c_void,
) -> sys::GhosttyString {
    let version = sys::GHOSTTY_XTVERSION.as_bytes();
    sys::GhosttyString {
        ptr: version.as_ptr(),
        len: version.len(),
    }
}

pub(crate) unsafe extern "C" fn size(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttySizeReportSize,
) -> bool {
    let mut filled = false;
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            if let Some(out) = out.as_mut() {
                let value = state.size();
                *out = sys::GhosttySizeReportSize {
                    rows: value.rows,
                    columns: value.cols,
                    cell_width: value.cell_width,
                    cell_height: value.cell_height,
                };
                filled = true;
            }
        });
    }
    filled
}

pub(crate) unsafe extern "C" fn color_scheme(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttyColorScheme,
) -> bool {
    let mut filled = false;
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            if let Some(out) = out.as_mut() {
                *out = match state.color_scheme() {
                    ColorScheme::Light => sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_LIGHT,
                    ColorScheme::Dark => sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK,
                };
                filled = true;
            }
        });
    }
    filled
}

pub(crate) unsafe extern "C" fn device_attributes(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttyDeviceAttributes,
) -> bool {
    let mut filled = false;
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |_| {
            if let Some(out) = out.as_mut() {
                *out = std::mem::zeroed();
                // Match Ghostty's native VT220 profile. Paneflow supports ANSI
                // color and write-only OSC 52 clipboard storage.
                out.primary.conformance_level = sys::GHOSTTY_DA_CONFORMANCE_VT220 as u16;
                out.primary.features[0] = sys::GHOSTTY_DA_FEATURE_ANSI_COLOR as u16;
                out.primary.features[1] = sys::GHOSTTY_DA_FEATURE_CLIPBOARD as u16;
                out.primary.num_features = 2;
                out.secondary.device_type = sys::GHOSTTY_DA_DEVICE_TYPE_VT220 as u16;
                out.secondary.firmware_version = 10;
                out.secondary.rom_cartridge = 0;
                filled = true;
            }
        });
    }
    filled
}

pub(crate) unsafe extern "C" fn progress_report(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const sys::GhosttyTerminalProgressReport,
) {
    // SAFETY: libghostty supplies the userdata pointer registered by
    // `callbacks::install`; the boxed state outlives the terminal callback.
    unsafe {
        with_state(userdata, |state| {
            let Some(report) = report.as_ref() else {
                return;
            };
            if report.size < size_of::<sys::GhosttyTerminalProgressReport>() {
                return;
            }
            let Some(state_kind) = progress_state(report.state) else {
                return;
            };
            state.push(BackendEvent::Progress(ProgressReport {
                state: state_kind,
                percent: u8::try_from(report.progress).ok().filter(|&p| p <= 100),
            }));
        });
    }
}

/// Map a libghostty progress state onto the neutral mirror.
///
/// An unrecognized state means the pinned library grew a variant Paneflow does
/// not model yet, so the report is dropped rather than guessed at.
fn progress_state(state: sys::GhosttyTerminalProgressState) -> Option<ProgressState> {
    match state {
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE => {
            Some(ProgressState::Remove)
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_SET => {
            Some(ProgressState::Set)
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR => {
            Some(ProgressState::Error)
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE => {
            Some(ProgressState::Indeterminate)
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE => {
            Some(ProgressState::Pause)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WindowSize;
    use crate::callbacks::CallbackState;

    #[test]
    fn identity_callbacks_match_ghostty_native_profile() {
        let version = unsafe { xtversion(std::ptr::null_mut(), std::ptr::null_mut()) };
        let version = unsafe { std::slice::from_raw_parts(version.ptr, version.len) };
        assert_eq!(version, sys::GHOSTTY_XTVERSION.as_bytes());

        let enquiry = unsafe { enquiry(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(enquiry.len, 0);

        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), ColorScheme::Dark);
        let mut attributes = unsafe { std::mem::zeroed::<sys::GhosttyDeviceAttributes>() };
        assert!(unsafe {
            device_attributes(
                std::ptr::null_mut(),
                (&state as *const CallbackState).cast_mut().cast(),
                &mut attributes,
            )
        });
        assert_eq!(
            attributes.primary.conformance_level,
            sys::GHOSTTY_DA_CONFORMANCE_VT220 as u16
        );
        assert_eq!(
            &attributes.primary.features[..attributes.primary.num_features],
            &[
                sys::GHOSTTY_DA_FEATURE_ANSI_COLOR as u16,
                sys::GHOSTTY_DA_FEATURE_CLIPBOARD as u16,
            ]
        );
        assert_eq!(
            attributes.secondary.device_type,
            sys::GHOSTTY_DA_DEVICE_TYPE_VT220 as u16
        );
        assert_eq!(attributes.secondary.firmware_version, 10);
        assert_eq!(attributes.secondary.rom_cartridge, 0);
    }

    #[test]
    fn color_scheme_callback_uses_the_configured_appearance() {
        for (scheme, expected) in [
            (
                ColorScheme::Light,
                sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_LIGHT,
            ),
            (
                ColorScheme::Dark,
                sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK,
            ),
        ] {
            let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), scheme);
            let mut actual = sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK;
            assert!(unsafe {
                color_scheme(
                    std::ptr::null_mut(),
                    (&state as *const CallbackState).cast_mut().cast(),
                    &mut actual,
                )
            });
            assert_eq!(actual, expected);
        }
    }
}
