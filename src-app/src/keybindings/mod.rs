mod apply;
mod defaults;
mod display;
mod registry;

pub use apply::{apply_keybindings, keystrokes_conflict};
pub use display::{
    ShortcutEntry, default_keys, effective_shortcuts, format_keystroke, is_bare_modifier,
    keystroke_caps,
};
pub use registry::{ShortcutGroup, action_is_global};

pub fn action_for_name(name: &str) -> Option<Box<dyn gpui::Action>> {
    registry::action_from_name(name)
}
