mod apply;
mod chord_query;
mod defaults;
mod display;
mod registry;

pub use apply::{
    LuaKeymap, LuaKeymapWarnings, RunLuaAction, apply_keybindings, host_os,
    keyboard_layout_changed, keystrokes_conflict, parse_keystroke,
};
pub use chord_query::{ChordFit, ChordPattern};
pub use display::{
    ShortcutEntry, default_keys, effective_shortcuts, format_keystroke, is_bare_modifier,
};
pub use registry::ShortcutGroup;

pub fn action_for_name(name: &str) -> Option<Box<dyn gpui::Action>> {
    registry::action_from_name(name)
}

pub fn builtin_actions() -> Vec<paneflow_api::methods::config::BuiltinAction> {
    registry::builtin_actions()
        .map(
            |(name, description, group)| paneflow_api::methods::config::BuiltinAction {
                name: name.to_string(),
                description: description.to_string(),
                group: group.to_string(),
            },
        )
        .collect()
}

pub fn is_builtin_action(name: &str) -> bool {
    registry::action_from_name(name).is_some()
}
