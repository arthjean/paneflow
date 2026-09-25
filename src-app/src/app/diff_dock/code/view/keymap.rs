use super::*;

pub(crate) const CODE_KEY_CONTEXT: &str = "CodeEditor";

actions!(
    paneflow_code_editor,
    [
        CeLeft,
        CeRight,
        CeUp,
        CeDown,
        CeSelectLeft,
        CeSelectRight,
        CeSelectUp,
        CeSelectDown,
        CeWordLeft,
        CeWordRight,
        CeSelectWordLeft,
        CeSelectWordRight,
        CeHome,
        CeEnd,
        CeSelectHome,
        CeSelectEnd,
        CePageUp,
        CePageDown,
        CeSelectPageUp,
        CeSelectPageDown,
        CeDocStart,
        CeDocEnd,
        CeSelectDocStart,
        CeSelectDocEnd,
        CeSelectAll,
        CeBackspace,
        CeDelete,
        CeNewline,
        CeUndo,
        CeRedo,
        CeCopy,
        CeCut,
        CePaste,
        CeIndent,
        CeOutdent,
        CeSave,
        CeEscape,
    ]
);

pub(crate) fn register_keybindings(cx: &mut App) {
    let ctx = Some(CODE_KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", CeLeft, ctx),
        KeyBinding::new("right", CeRight, ctx),
        KeyBinding::new("up", CeUp, ctx),
        KeyBinding::new("down", CeDown, ctx),
        KeyBinding::new("shift-left", CeSelectLeft, ctx),
        KeyBinding::new("shift-right", CeSelectRight, ctx),
        KeyBinding::new("shift-up", CeSelectUp, ctx),
        KeyBinding::new("shift-down", CeSelectDown, ctx),
        KeyBinding::new("home", CeHome, ctx),
        KeyBinding::new("end", CeEnd, ctx),
        KeyBinding::new("shift-home", CeSelectHome, ctx),
        KeyBinding::new("shift-end", CeSelectEnd, ctx),
        KeyBinding::new("pageup", CePageUp, ctx),
        KeyBinding::new("pagedown", CePageDown, ctx),
        KeyBinding::new("shift-pageup", CeSelectPageUp, ctx),
        KeyBinding::new("shift-pagedown", CeSelectPageDown, ctx),
        KeyBinding::new("secondary-a", CeSelectAll, ctx),
        KeyBinding::new("backspace", CeBackspace, ctx),
        KeyBinding::new("delete", CeDelete, ctx),
        KeyBinding::new("enter", CeNewline, ctx),
        KeyBinding::new("tab", CeIndent, ctx),
        KeyBinding::new("shift-tab", CeOutdent, ctx),
        KeyBinding::new("secondary-z", CeUndo, ctx),
        KeyBinding::new("secondary-shift-z", CeRedo, ctx),
        KeyBinding::new("secondary-c", CeCopy, ctx),
        KeyBinding::new("secondary-x", CeCut, ctx),
        KeyBinding::new("secondary-v", CePaste, ctx),
        KeyBinding::new("secondary-s", CeSave, ctx),
        KeyBinding::new("escape", CeEscape, ctx),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("alt-left", CeWordLeft, ctx),
        KeyBinding::new("alt-right", CeWordRight, ctx),
        KeyBinding::new("alt-shift-left", CeSelectWordLeft, ctx),
        KeyBinding::new("alt-shift-right", CeSelectWordRight, ctx),
        KeyBinding::new("cmd-up", CeDocStart, ctx),
        KeyBinding::new("cmd-down", CeDocEnd, ctx),
        KeyBinding::new("cmd-shift-up", CeSelectDocStart, ctx),
        KeyBinding::new("cmd-shift-down", CeSelectDocEnd, ctx),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-left", CeWordLeft, ctx),
        KeyBinding::new("ctrl-right", CeWordRight, ctx),
        KeyBinding::new("ctrl-shift-left", CeSelectWordLeft, ctx),
        KeyBinding::new("ctrl-shift-right", CeSelectWordRight, ctx),
        KeyBinding::new("ctrl-home", CeDocStart, ctx),
        KeyBinding::new("ctrl-end", CeDocEnd, ctx),
        KeyBinding::new("ctrl-shift-home", CeSelectDocStart, ctx),
        KeyBinding::new("ctrl-shift-end", CeSelectDocEnd, ctx),
        KeyBinding::new("ctrl-y", CeRedo, ctx),
    ]);
}
