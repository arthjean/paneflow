use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::callbacks::with_state;
use crate::osc7::working_directory_from_ghostty;
use crate::{
    BackendEvent, ColorScheme, OscTerminator, ProgramStatusKind, ProgramStatusReport,
    ProgramStatusState, ProgressReport, ProgressState, PromptKind, SemanticPromptKind,
    UnknownSequenceKind,
};

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
    unsafe { with_state(userdata, |state| state.push(BackendEvent::Bell)) };
}

pub(crate) unsafe extern "C" fn render_hold(
    terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    held: bool,
) {
    unsafe {
        with_state(userdata, |state| {
            if held {
                state.begin_render_hold(terminal);
            } else {
                state.end_render_hold();
            }
        });
    }
}

pub(crate) unsafe extern "C" fn title_changed(
    terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
) {
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

unsafe fn store_clipboard_write(
    state: &crate::callbacks::CallbackState,
    request: &sys::GhosttyClipboardWrite,
) -> sys::GhosttyClipboardWriteResult {
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
    unsafe {
        with_state(userdata, |state| {
            let Some(request) = notification.as_ref() else {
                return;
            };
            if request.size < size_of::<sys::GhosttyTerminalDesktopNotification>() {
                return;
            }
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
    unsafe {
        with_state(userdata, |state| {
            let Some(report) = sequence.as_ref() else {
                return;
            };
            let (kind, content, truncated) = match report.tag {
                sys::GhosttyTerminalUnknownSequenceTag_GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_APC => {
                    let apc = report.value.apc;
                    (UnknownSequenceKind::Apc, apc.content, apc.truncated)
                }
                sys::GhosttyTerminalUnknownSequenceTag_GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_OSC => {
                    let osc = report.value.osc;
                    let Some(terminator) = osc_terminator(osc.terminator) else {
                        return;
                    };
                    (
                        UnknownSequenceKind::Osc(terminator),
                        osc.content,
                        osc.truncated,
                    )
                }
                _ => return,
            };
            let Some(raw) = borrowed_bytes(content, MAX_CALLBACK_BYTES) else {
                return;
            };
            state.push(BackendEvent::UnknownSequence {
                kind,
                content: escape_content(raw),
                truncated,
            });
        });
    }
}

pub(crate) unsafe extern "C" fn clipboard_read(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    read: *const sys::GhosttyClipboardRead,
) {
    unsafe {
        with_state(userdata, |_| {
            let Some(request) = read.as_ref() else {
                return;
            };
            if request.size < size_of::<sys::GhosttyClipboardRead>() {
                return;
            }
            let Some(answer) = request.reply else {
                return;
            };
            answer(read, &denied_read());
        });
    }
}

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

fn osc_terminator(raw: sys::GhosttyOscTerminator) -> Option<OscTerminator> {
    match raw {
        sys::GhosttyOscTerminator_GHOSTTY_OSC_TERMINATOR_ST => Some(OscTerminator::St),
        sys::GhosttyOscTerminator_GHOSTTY_OSC_TERMINATOR_BEL => Some(OscTerminator::Bel),
        _ => None,
    }
}

unsafe fn borrowed_bytes(text: sys::GhosttyString, limit: usize) -> Option<&'static [u8]> {
    if text.len == 0 {
        return Some(&[]);
    }
    if text.ptr.is_null() || text.len > limit {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(text.ptr, text.len) })
}

unsafe fn borrowed_text(text: sys::GhosttyString, limit: usize) -> Option<String> {
    let bytes = unsafe { borrowed_bytes(text, limit) }?;
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

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
    unsafe {
        with_state(userdata, |_| {
            if let Some(out) = out.as_mut() {
                *out = std::mem::zeroed();
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

pub(crate) unsafe extern "C" fn program_status(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const sys::GhosttyTerminalProgramStatus,
) {
    unsafe {
        with_state(userdata, |state| {
            if let Some(report) = program_status_report(report) {
                state.push(BackendEvent::ProgramStatus(report));
            }
        });
    }
}

unsafe fn program_status_report(
    report: *const sys::GhosttyTerminalProgramStatus,
) -> Option<ProgramStatusReport> {
    let size = unsafe { report.cast::<usize>().as_ref() }.copied()?;
    if size < size_of::<sys::GhosttyTerminalProgramStatus>() {
        return None;
    }
    let report = unsafe { &*report };
    let state = program_status_state(report.state)?;
    Some(ProgramStatusReport {
        state,
        kind: program_status_kind(report.kind),
        progress: u8::try_from(report.progress).ok().filter(|&p| p <= 100),
        id: unsafe { borrowed_text(report.id, MAX_CALLBACK_BYTES) }?,
        app: unsafe { borrowed_text(report.app, MAX_CALLBACK_BYTES) }?,
        title: unsafe { borrowed_text(report.title, MAX_CALLBACK_BYTES) }?,
        message: unsafe { borrowed_text(report.message, MAX_CALLBACK_BYTES) }?,
    })
}

fn program_status_state(state: sys::GhosttyProgramStatusState) -> Option<ProgramStatusState> {
    match state {
        sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_IDLE => {
            Some(ProgramStatusState::Idle)
        }
        sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_WORKING => {
            Some(ProgramStatusState::Working)
        }
        sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_DONE => {
            Some(ProgramStatusState::Done)
        }
        sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED => {
            Some(ProgramStatusState::Blocked)
        }
        sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_ERROR => {
            Some(ProgramStatusState::Error)
        }
        sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_CLEAR => {
            Some(ProgramStatusState::Clear)
        }
        _ => None,
    }
}

fn program_status_kind(kind: sys::GhosttyProgramStatusKind) -> Option<ProgramStatusKind> {
    match kind {
        sys::GhosttyProgramStatusKind_GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION => {
            Some(ProgramStatusKind::Permission)
        }
        sys::GhosttyProgramStatusKind_GHOSTTY_PROGRAM_STATUS_KIND_QUESTION => {
            Some(ProgramStatusKind::Question)
        }
        sys::GhosttyProgramStatusKind_GHOSTTY_PROGRAM_STATUS_KIND_AUTH => {
            Some(ProgramStatusKind::Auth)
        }
        _ => None,
    }
}

pub(crate) unsafe extern "C" fn semantic_prompt(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    event: *const sys::GhosttyTerminalSemanticPrompt,
) {
    unsafe {
        with_state(userdata, |state| {
            if let Some(event) = semantic_prompt_event(event) {
                state.push(event);
            }
        });
    }
}

unsafe fn semantic_prompt_event(
    event: *const sys::GhosttyTerminalSemanticPrompt,
) -> Option<BackendEvent> {
    let size = unsafe { event.cast::<usize>().as_ref() }.copied()?;
    if size < size_of::<sys::GhosttyTerminalSemanticPrompt>() {
        return None;
    }
    let event = unsafe { &*event };
    let kind = match event.kind {
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_START => {
            SemanticPromptKind::PromptStart
        }
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_INPUT_START => {
            SemanticPromptKind::InputStart
        }
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_OUTPUT_START => {
            SemanticPromptKind::OutputStart
        }
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_COMMAND_END => {
            SemanticPromptKind::CommandEnd
        }
        _ => return None,
    };
    let prompt_kind = match event.prompt_kind {
        sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_RIGHT => {
            PromptKind::Right
        }
        sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_CONTINUATION => {
            PromptKind::Continuation
        }
        sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_SECONDARY => {
            PromptKind::Secondary
        }
        _ => PromptKind::Primary,
    };
    Some(BackendEvent::SemanticPrompt {
        kind,
        prompt_kind,
        exit_code: event.has_exit_code.then_some(event.exit_code),
    })
}

pub(crate) unsafe extern "C" fn reset(_: sys::GhosttyTerminal, userdata: *mut c_void) {
    unsafe { with_state(userdata, |state| state.push(BackendEvent::Reset)) };
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

        let state = unsafe {
            CallbackState::new(
                WindowSize::new(80, 24, 8, 16).unwrap(),
                ColorScheme::Dark,
                std::ptr::null(),
            )
        }
        .unwrap();
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

    fn status_report(state: sys::GhosttyProgramStatusState) -> sys::GhosttyTerminalProgramStatus {
        let empty = sys::GhosttyString {
            ptr: std::ptr::null(),
            len: 0,
        };
        sys::GhosttyTerminalProgramStatus {
            size: size_of::<sys::GhosttyTerminalProgramStatus>(),
            state,
            kind: sys::GhosttyProgramStatusKind_GHOSTTY_PROGRAM_STATUS_KIND_NONE,
            progress: -1,
            id: empty,
            app: empty,
            title: empty,
            message: empty,
        }
    }

    #[test]
    fn program_status_ignores_unknown_states_and_short_structs() {
        let idle = sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_IDLE;
        let unknown = sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_CLEAR + 1;
        assert!(unsafe { program_status_report(&status_report(unknown)) }.is_none());

        let mut short = status_report(idle);
        short.size -= 1;
        assert!(unsafe { program_status_report(&short) }.is_none());

        assert!(unsafe { program_status_report(std::ptr::null()) }.is_none());

        let report = unsafe { program_status_report(&status_report(idle)) }.unwrap();
        assert_eq!(report.state, ProgramStatusState::Idle);
        assert_eq!(report.progress, None);
        assert_eq!(report.kind, None);
    }

    #[test]
    fn program_status_maps_out_of_range_progress_and_unknown_kind_to_none() {
        let mut report =
            status_report(sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED);
        report.kind = sys::GhosttyProgramStatusKind_GHOSTTY_PROGRAM_STATUS_KIND_AUTH + 1;
        report.progress = 101;
        let copied = unsafe { program_status_report(&report) }.unwrap();
        assert_eq!(copied.kind, None);
        assert_eq!(copied.progress, None);

        report.kind = sys::GhosttyProgramStatusKind_GHOSTTY_PROGRAM_STATUS_KIND_QUESTION;
        report.progress = 0;
        let copied = unsafe { program_status_report(&report) }.unwrap();
        assert_eq!(copied.kind, Some(ProgramStatusKind::Question));
        assert_eq!(copied.progress, Some(0));
    }

    #[test]
    fn program_status_drops_a_report_with_a_string_over_the_callback_limit() {
        let oversized = vec![b'a'; MAX_CALLBACK_BYTES + 1];
        let mut report =
            status_report(sys::GhosttyProgramStatusState_GHOSTTY_PROGRAM_STATUS_STATE_DONE);
        report.message = sys::GhosttyString {
            ptr: oversized.as_ptr(),
            len: oversized.len(),
        };
        assert!(unsafe { program_status_report(&report) }.is_none());
    }

    #[test]
    fn semantic_prompt_ignores_unknown_kinds_and_short_structs() {
        let empty = sys::GhosttyString {
            ptr: std::ptr::null(),
            len: 0,
        };
        let mut event = sys::GhosttyTerminalSemanticPrompt {
            size: size_of::<sys::GhosttyTerminalSemanticPrompt>(),
            kind: sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_INVALID,
            prompt_kind: sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY,
            has_exit_code: false,
            exit_code: 0,
            command: empty,
            error: empty,
        };
        assert!(unsafe { semantic_prompt_event(&event) }.is_none());

        event.kind = sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_COMMAND_END;
        event.has_exit_code = true;
        event.exit_code = -1;
        assert_eq!(
            unsafe { semantic_prompt_event(&event) },
            Some(BackendEvent::SemanticPrompt {
                kind: SemanticPromptKind::CommandEnd,
                prompt_kind: PromptKind::Primary,
                exit_code: Some(-1),
            })
        );

        event.size -= 1;
        assert!(unsafe { semantic_prompt_event(&event) }.is_none());
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
            let state = unsafe {
                CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), scheme, std::ptr::null())
            }
            .unwrap();
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
