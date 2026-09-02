#![allow(deprecated, unexpected_cfgs)]

use cocoa::{
    appkit::{
        NSAppearance, NSAppearanceNameVibrantDark, NSAppearanceNameVibrantLight, NSView,
        NSViewHeightSizable, NSViewWidthSizable, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    },
    base::{NO, YES, id, nil},
};
use gpui::WindowBackgroundAppearance;
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;

thread_local! {
    static SIDEBAR_MATERIAL: RefCell<Option<SidebarMaterial>> = const { RefCell::new(None) };
}

struct SidebarMaterial {
    effect_view: id,
    is_light: bool,
    is_enabled: bool,
}

pub(crate) fn apply_subtle_sidebar_material(
    window: &gpui::Window,
    is_light: bool,
    is_enabled: bool,
) {
    match try_apply_subtle_sidebar_material(window, is_light, is_enabled) {
        Ok(effect_view) => {
            SIDEBAR_MATERIAL.with(|slot| {
                *slot.borrow_mut() = Some(SidebarMaterial {
                    effect_view,
                    is_light,
                    is_enabled,
                });
            });
        }
        Err(error) => {
            log::warn!("Could not install the native macOS sidebar material: {error}");
            window.set_background_appearance(WindowBackgroundAppearance::Blurred);
        }
    }
}

pub(crate) fn sync_subtle_sidebar_material(is_light: bool, is_enabled: bool) {
    SIDEBAR_MATERIAL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(material) = slot.as_mut() else {
            return;
        };
        if material.is_light != is_light {
            set_material_appearance(material.effect_view, is_light);
            material.is_light = is_light;
        }
        if material.is_enabled != is_enabled {
            set_material_enabled(material.effect_view, is_enabled);
            material.is_enabled = is_enabled;
        }
    });
}

fn try_apply_subtle_sidebar_material(
    window: &gpui::Window,
    is_light: bool,
    is_enabled: bool,
) -> Result<id, &'static str> {
    let window_handle = HasWindowHandle::window_handle(window)
        .map_err(|_| "GPUI did not expose an AppKit window handle")?;
    let RawWindowHandle::AppKit(handle) = window_handle.as_raw() else {
        return Err("GPUI returned a non-AppKit window handle on macOS");
    };

    let native_view = handle.ns_view.as_ptr() as id;

    unsafe {
        let content_view: id = msg_send![native_view, superview];
        if content_view == nil {
            return Err("GPUI's native view is not attached to an NSWindow");
        }

        let frame = NSView::bounds(content_view);
        let effect_view = NSVisualEffectView::initWithFrame_(NSVisualEffectView::alloc(nil), frame);
        if effect_view == nil {
            return Err("AppKit could not create NSVisualEffectView");
        }

        NSView::setAutoresizingMask_(effect_view, NSViewWidthSizable | NSViewHeightSizable);
        NSVisualEffectView::setMaterial_(effect_view, NSVisualEffectMaterial::Sidebar);
        NSVisualEffectView::setBlendingMode_(effect_view, NSVisualEffectBlendingMode::BehindWindow);
        NSVisualEffectView::setState_(effect_view, NSVisualEffectState::FollowsWindowActiveState);
        set_material_appearance(effect_view, is_light);
        set_material_enabled(effect_view, is_enabled);

        let _: () = msg_send![
            content_view,
            addSubview: effect_view
            positioned: NSWindowOrderingMode::NSWindowBelow
            relativeTo: native_view
        ];
        let _: () = msg_send![effect_view, release];

        Ok(effect_view)
    }
}

fn set_material_enabled(effect_view: id, is_enabled: bool) {
    unsafe {
        let hidden = if is_enabled { NO } else { YES };
        let _: () = msg_send![effect_view, setHidden: hidden];
    }
}

fn set_material_appearance(effect_view: id, is_light: bool) {
    unsafe {
        let name = if is_light {
            NSAppearanceNameVibrantLight
        } else {
            NSAppearanceNameVibrantDark
        };
        NSView::setAppearance(effect_view, NSAppearance(name));
    }
}
