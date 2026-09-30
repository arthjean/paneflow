use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use super::builtin::{paneflow_dark, theme_by_name};
use super::model::{TerminalTheme, apply_surface_overrides};

static ACTIVE_THEME: Mutex<Option<TerminalTheme>> = Mutex::new(None);
static THEME_GENERATION: AtomicU64 = AtomicU64::new(0);

fn resolve_theme_name(name: Option<&str>) -> TerminalTheme {
    if let Some(name) = name {
        if let Some(theme) = theme_by_name(name) {
            return apply_surface_overrides(theme);
        }
        log::warn!("Unknown theme '{}', using default", name);
    }
    apply_surface_overrides(paneflow_dark())
}

fn install_theme(active: &mut Option<TerminalTheme>, theme: TerminalTheme) -> TerminalTheme {
    let changed = active.is_some_and(|current| current != theme);
    *active = Some(theme);
    let generation = if changed {
        THEME_GENERATION.fetch_add(1, Ordering::AcqRel) + 1
    } else {
        theme_generation()
    };
    super::palette::install_palette(&theme, generation);
    theme
}

pub fn set_active_theme(name: Option<&str>) {
    install_theme(&mut ACTIVE_THEME.lock(), resolve_theme_name(name));
}

#[cfg(test)]
pub fn invalidate_theme_cache() {
    *ACTIVE_THEME.lock() = None;
    THEME_GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn theme_generation() -> u64 {
    THEME_GENERATION.load(Ordering::Acquire)
}

pub fn active_theme() -> TerminalTheme {
    let mut active = ACTIVE_THEME.lock();
    match *active {
        Some(theme) => theme,
        None => install_theme(
            &mut active,
            resolve_theme_name(paneflow_config::loader::load_config().theme.as_deref()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static SERIAL_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn installing_the_same_theme_twice_bumps_the_generation_once() {
        let _g = SERIAL_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        set_active_theme(Some("Vercel Dark"));
        let before = theme_generation();
        set_active_theme(Some("Vercel Dark"));
        assert_eq!(theme_generation(), before);
        set_active_theme(Some("Claude Dark"));
        assert_eq!(theme_generation(), before + 1);
    }

    #[test]
    fn an_invalid_config_on_disk_never_changes_the_active_theme() {
        let _g = SERIAL_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        set_active_theme(Some("Claude Dark"));
        let before = theme_generation();
        let dir = tempfile::tempdir().expect("config dir");
        let path = dir.path().join("paneflow.json");
        std::fs::write(&path, "{ \"theme\": ").expect("invalid config");
        let loaded = paneflow_config::loader::load_config_from_path(&path);
        assert_eq!(loaded.theme, None, "an invalid file loads as defaults");
        assert!(active_theme() == resolve_theme_name(Some("Claude Dark")));
        assert_eq!(theme_generation(), before);
    }

    #[test]
    fn the_retired_paneflow_light_alias_resolves_through_the_case_insensitive_name() {
        assert!(
            resolve_theme_name(Some("PaneFlow Light"))
                == apply_surface_overrides(crate::theme::paneflow_light())
        );
        assert!(
            resolve_theme_name(Some("No Such Theme")) == apply_surface_overrides(paneflow_dark())
        );
    }
}
