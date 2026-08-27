//! Standalone OSC (Operating System Command) parsing.
//!
//! The terminal parses its own OSC sequences and reports them through
//! callbacks. This parser is for OSC bytes that never reach a terminal:
//! classifying a sequence captured from a log, a hook payload, or an agent's
//! raw output without instantiating a screen for it.

use std::ffi::CStr;
use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::handles::{OwnedHandle, create};
use crate::{GhosttyError, Result};

/// BEL, the classic OSC terminator.
pub const OSC_TERMINATOR_BEL: u8 = 0x07;
/// The `ST` half of `ESC \`, the standards-conformant OSC terminator.
pub const OSC_TERMINATOR_ST: u8 = 0x5c;

/// Cap on a single OSC payload fed through [`OscParser::parse`]. libghostty
/// enforces its own per-command budgets; this only bounds what Paneflow will
/// hand it in one call.
const MAX_OSC_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The kind of OSC command a payload turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscCommandType {
    /// The payload did not parse as a known command.
    Invalid,
    /// OSC 0 / OSC 2: set the window title.
    ChangeWindowTitle,
    /// OSC 1: set the window icon name.
    ChangeWindowIcon,
    /// OSC 133: shell integration prompt marks.
    SemanticPrompt,
    /// OSC 4 / 10 / 11 / 12 and friends: query or set a color.
    ColorOperation,
    /// OSC 8 start: open a hyperlink.
    HyperlinkStart,
    /// OSC 8 end: close a hyperlink.
    HyperlinkEnd,
    /// OSC 52: clipboard contents.
    ClipboardContents,
    /// OSC 7: report the working directory.
    ReportPwd,
    /// OSC 22: set the mouse cursor shape.
    MouseShape,
    /// OSC 9 / OSC 777: desktop notification.
    ShowDesktopNotification,
    /// OSC 99: the Kitty desktop notification protocol.
    KittyDesktopNotification,
    /// OSC 5522: the Kitty clipboard protocol.
    KittyClipboardProtocol,
    /// OSC 21: the Kitty color protocol.
    KittyColorProtocol,
    /// The Kitty drag-and-drop protocol.
    KittyDndProtocol,
    /// The Kitty text sizing protocol.
    KittyTextSizing,
    /// A signal delivered through a context command.
    ContextSignal,
    /// ConEmu: change the tab title.
    ConemuChangeTabTitle,
    /// ConEmu: a comment, which has no effect.
    ConemuComment,
    /// ConEmu: a GUI macro.
    ConemuGuimacro,
    /// ConEmu: output an environment variable.
    ConemuOutputEnvironmentVariable,
    /// ConEmu: OSC 9;4 progress report.
    ConemuProgressReport,
    /// ConEmu: run a process.
    ConemuRunProcess,
    /// ConEmu: show a message box.
    ConemuShowMessageBox,
    /// ConEmu: sleep.
    ConemuSleep,
    /// ConEmu: wait for input.
    ConemuWaitInput,
    /// ConEmu: xterm emulation control.
    ConemuXtermEmulation,
}

/// A parsed OSC command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OscCommand {
    /// What kind of command this is.
    pub kind: OscCommandType,
    /// The requested window title, for [`OscCommandType::ChangeWindowTitle`].
    ///
    /// libghostty exposes typed payload extraction for this command only; the
    /// others are currently reported by kind alone.
    pub window_title: Option<String>,
}

/// An incremental OSC parser.
pub struct OscParser {
    handle: OwnedHandle<sys::GhosttyOscParser>,
}

impl OscParser {
    /// Create a parser on libghostty's default allocator.
    pub fn new() -> Result<Self> {
        // SAFETY: the null allocator selects libghostty's default, and
        // `ghostty_osc_free` is the matching destructor for the handle
        // `ghostty_osc_new` produces.
        let handle = unsafe {
            create(
                "osc_new",
                std::ptr::null(),
                sys::ghostty_osc_new,
                sys::ghostty_osc_free,
            )?
        };
        Ok(Self { handle })
    }

    /// Discard any partially accumulated sequence.
    pub fn reset(&mut self) {
        // SAFETY: the handle is live for as long as `self` is.
        unsafe { sys::ghostty_osc_reset(self.handle.raw()) };
    }

    /// Feed one payload byte, excluding the `ESC ]` introducer and the
    /// terminator.
    pub fn feed_byte(&mut self, byte: u8) {
        // SAFETY: the handle is live for as long as `self` is.
        unsafe { sys::ghostty_osc_next(self.handle.raw(), byte) };
    }

    /// Feed a run of payload bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    /// Terminate the sequence and read back the command.
    ///
    /// `terminator` should be [`OSC_TERMINATOR_BEL`] or
    /// [`OSC_TERMINATOR_ST`]; libghostty keeps it so a reply can mirror the
    /// form the program used.
    pub fn end(&mut self, terminator: u8) -> Result<OscCommand> {
        // SAFETY: the handle is live for as long as `self` is.
        let command = unsafe { sys::ghostty_osc_end(self.handle.raw(), terminator) };
        // SAFETY: `ghostty_osc_command_type` accepts a null command and the
        // handle stays valid until the next parser call, which the borrow of
        // `self` prevents until this function returns.
        let kind = command_type(unsafe { sys::ghostty_osc_command_type(command) })?;
        let window_title = if kind == OscCommandType::ChangeWindowTitle {
            window_title(command)?
        } else {
            None
        };
        Ok(OscCommand { kind, window_title })
    }

    /// Parse one complete payload: reset, feed, terminate.
    pub fn parse(&mut self, payload: &[u8], terminator: u8) -> Result<OscCommand> {
        if payload.len() > MAX_OSC_PAYLOAD_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "OSC payload",
                limit: MAX_OSC_PAYLOAD_BYTES,
            });
        }
        self.reset();
        self.feed(payload);
        self.end(terminator)
    }
}

fn window_title(command: sys::GhosttyOscCommand) -> Result<Option<String>> {
    let mut pointer: *const std::ffi::c_char = std::ptr::null();
    // SAFETY: the data kind writes a `const char *`, which is what `pointer`
    // provides, and the command handle is live for this call.
    let extracted = unsafe {
        sys::ghostty_osc_command_data(
            command,
            sys::GhosttyOscCommandData_GHOSTTY_OSC_DATA_CHANGE_WINDOW_TITLE_STR,
            (&raw mut pointer).cast::<c_void>(),
        )
    };
    if !extracted || pointer.is_null() {
        return Ok(None);
    }
    // SAFETY: libghostty documents this as a null-terminated string owned by
    // the parser and valid until the next call on it. It is copied here.
    let title = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|_| GhosttyError::InvalidUtf8("OSC window title"))?
        .to_owned();
    Ok(Some(title))
}

fn command_type(value: sys::GhosttyOscCommandType) -> Result<OscCommandType> {
    use sys as s;
    Ok(match value {
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_INVALID => OscCommandType::Invalid,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CHANGE_WINDOW_TITLE => {
            OscCommandType::ChangeWindowTitle
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CHANGE_WINDOW_ICON => {
            OscCommandType::ChangeWindowIcon
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_SEMANTIC_PROMPT => {
            OscCommandType::SemanticPrompt
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_COLOR_OPERATION => {
            OscCommandType::ColorOperation
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_HYPERLINK_START => {
            OscCommandType::HyperlinkStart
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_HYPERLINK_END => OscCommandType::HyperlinkEnd,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CLIPBOARD_CONTENTS => {
            OscCommandType::ClipboardContents
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_REPORT_PWD => OscCommandType::ReportPwd,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_MOUSE_SHAPE => OscCommandType::MouseShape,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_SHOW_DESKTOP_NOTIFICATION => {
            OscCommandType::ShowDesktopNotification
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_KITTY_DESKTOP_NOTIFICATION => {
            OscCommandType::KittyDesktopNotification
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_KITTY_CLIPBOARD_PROTOCOL => {
            OscCommandType::KittyClipboardProtocol
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_KITTY_COLOR_PROTOCOL => {
            OscCommandType::KittyColorProtocol
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_KITTY_DND_PROTOCOL => {
            OscCommandType::KittyDndProtocol
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_KITTY_TEXT_SIZING => {
            OscCommandType::KittyTextSizing
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONTEXT_SIGNAL => OscCommandType::ContextSignal,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_CHANGE_TAB_TITLE => {
            OscCommandType::ConemuChangeTabTitle
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_COMMENT => OscCommandType::ConemuComment,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_GUIMACRO => {
            OscCommandType::ConemuGuimacro
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_OUTPUT_ENVIRONMENT_VARIABLE => {
            OscCommandType::ConemuOutputEnvironmentVariable
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_PROGRESS_REPORT => {
            OscCommandType::ConemuProgressReport
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_RUN_PROCESS => {
            OscCommandType::ConemuRunProcess
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_SHOW_MESSAGE_BOX => {
            OscCommandType::ConemuShowMessageBox
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_SLEEP => OscCommandType::ConemuSleep,
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_WAIT_INPUT => {
            OscCommandType::ConemuWaitInput
        }
        s::GhosttyOscCommandType_GHOSTTY_OSC_COMMAND_CONEMU_XTERM_EMULATION => {
            OscCommandType::ConemuXtermEmulation
        }
        other => {
            return Err(GhosttyError::AbiMismatch(format!(
                "unknown Ghostty OSC command type {other}"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(payload: &[u8]) -> OscCommand {
        let mut parser = OscParser::new().expect("parser must initialize");
        parser
            .parse(payload, OSC_TERMINATOR_BEL)
            .expect("payload must parse")
    }

    #[test]
    fn a_title_sequence_yields_its_string() {
        let command = parse(b"0;paneflow");
        assert_eq!(command.kind, OscCommandType::ChangeWindowTitle);
        assert_eq!(command.window_title.as_deref(), Some("paneflow"));

        assert_eq!(parse(b"2;other").window_title.as_deref(), Some("other"));
    }

    #[test]
    fn known_command_families_are_classified() {
        assert_eq!(parse(b"7;file:///tmp").kind, OscCommandType::ReportPwd);
        assert_eq!(parse(b"133;A").kind, OscCommandType::SemanticPrompt);
        assert_eq!(
            parse(b"52;c;cGFuZWZsb3c=").kind,
            OscCommandType::ClipboardContents
        );
        assert_eq!(parse(b"9;hello").kind, OscCommandType::ShowDesktopNotification);
        assert_eq!(
            parse(b"9;4;1;50").kind,
            OscCommandType::ConemuProgressReport
        );
        assert_eq!(parse(b"22;pointer").kind, OscCommandType::MouseShape);
    }

    #[test]
    fn an_unknown_payload_is_invalid_rather_than_an_error() {
        assert_eq!(parse(b"65535;nonsense").kind, OscCommandType::Invalid);
        assert_eq!(parse(b"").kind, OscCommandType::Invalid);
    }

    #[test]
    fn feeding_byte_by_byte_matches_a_single_payload() {
        let mut parser = OscParser::new().expect("parser must initialize");
        parser.reset();
        for byte in b"0;incremental" {
            parser.feed_byte(*byte);
        }
        let command = parser.end(OSC_TERMINATOR_ST).expect("payload must parse");
        assert_eq!(command.kind, OscCommandType::ChangeWindowTitle);
        assert_eq!(command.window_title.as_deref(), Some("incremental"));
    }

    #[test]
    fn reset_discards_a_partial_sequence() {
        let mut parser = OscParser::new().expect("parser must initialize");
        parser.feed(b"0;discarded");
        parser.reset();
        parser.feed(b"2;kept");
        let command = parser.end(OSC_TERMINATOR_BEL).expect("payload must parse");
        assert_eq!(command.window_title.as_deref(), Some("kept"));
    }
}
