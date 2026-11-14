use gpui::{AnyElement, IntoElement, Styled, img, px, svg};
use paneflow_config::schema::WorkspaceIcon;

use super::style::SIDEBAR_WORKSPACE_FOLDER_ICON_WIDTH;

const HIDPI_SCALE_FACTOR: f32 = 2.;
const GRID_GLYPH_SIZE: f32 = 16.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderInk {
    Muted,
    Text,
    Own,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FolderGlyphs {
    pub(crate) closed: &'static str,
    pub(crate) open: &'static str,
    pub(crate) size: f32,
    pub(crate) ink: FolderInk,
}

pub(crate) fn folder_glyphs(icon: WorkspaceIcon, scale_factor: f32) -> FolderGlyphs {
    match icon {
        WorkspaceIcon::Default => FolderGlyphs {
            closed: "icons/folder.svg",
            open: "icons/folder-open.svg",
            size: SIDEBAR_WORKSPACE_FOLDER_ICON_WIDTH,
            ink: FolderInk::Muted,
        },
        WorkspaceIcon::Standard => FolderGlyphs {
            closed: "icons/workspace/chevron-right.svg",
            open: "icons/workspace/chevron-down.svg",
            size: GRID_GLYPH_SIZE,
            ink: FolderInk::Muted,
        },
        WorkspaceIcon::Metal => FolderGlyphs {
            closed: "icons/workspace/metal-folder.svg",
            open: "icons/workspace/metal-folder-open.svg",
            size: GRID_GLYPH_SIZE,
            ink: FolderInk::Text,
        },
        WorkspaceIcon::Windows => FolderGlyphs {
            closed: "icons/workspace/windows-folder.svg",
            open: "icons/workspace/windows-folder-open.svg",
            size: GRID_GLYPH_SIZE,
            ink: FolderInk::Own,
        },
        WorkspaceIcon::Windows95 if scale_factor >= HIDPI_SCALE_FACTOR => FolderGlyphs {
            closed: "icons/workspace/windows95-folder-hidpi.svg",
            open: "icons/workspace/windows95-folder-open-hidpi.svg",
            size: GRID_GLYPH_SIZE,
            ink: FolderInk::Own,
        },
        WorkspaceIcon::Windows95 => FolderGlyphs {
            closed: "icons/workspace/windows95-folder.svg",
            open: "icons/workspace/windows95-folder-open.svg",
            size: GRID_GLYPH_SIZE,
            ink: FolderInk::Own,
        },
        WorkspaceIcon::Pixel => FolderGlyphs {
            closed: "icons/workspace/pixel-folder.svg",
            open: "icons/workspace/pixel-folder-open.svg",
            size: GRID_GLYPH_SIZE,
            ink: FolderInk::Muted,
        },
    }
}

impl FolderGlyphs {
    pub(crate) fn render(self, path: &'static str, ui: crate::theme::UiColors) -> AnyElement {
        let size = px(self.size);
        let tint = match self.ink {
            FolderInk::Own => return img(path).size(size).flex_none().into_any_element(),
            FolderInk::Muted => ui.muted,
            FolderInk::Text => ui.text,
        };
        svg()
            .size(size)
            .flex_none()
            .path(path)
            .text_color(tint)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALE_FACTORS: [f32; 4] = [1., 1.5, 2., 3.];

    fn asset(path: &str) -> String {
        let file =
            crate::assets::Assets::get(path).unwrap_or_else(|| panic!("{path} is not embedded"));
        String::from_utf8(file.data.into_owned()).expect("an SVG is UTF-8")
    }

    #[test]
    fn every_workspace_icon_draws_two_distinct_embedded_glyphs() {
        for &icon in WorkspaceIcon::ALL {
            for scale_factor in SCALE_FACTORS {
                let glyphs = folder_glyphs(icon, scale_factor);
                assert_ne!(glyphs.closed, glyphs.open, "{icon:?}");
                assert!(asset(glyphs.closed).contains("<svg"), "{icon:?}");
                assert!(asset(glyphs.open).contains("<svg"), "{icon:?}");
            }
        }
    }

    #[test]
    fn every_glyph_fits_the_folder_slot_on_whole_pixels() {
        for &icon in WorkspaceIcon::ALL {
            let glyphs = folder_glyphs(icon, 1.);
            assert!(glyphs.size <= super::super::SIDEBAR_FOLDER_SLOT_WIDTH);
            if icon != WorkspaceIcon::Default {
                let margin = super::super::SIDEBAR_FOLDER_SLOT_WIDTH - glyphs.size;
                assert_eq!(margin % 2., 0., "{icon:?} sits on a half pixel");
            }
        }
    }

    #[test]
    fn windows_95_draws_the_32_cell_original_only_where_a_cell_gets_a_device_pixel() {
        let coarse = folder_glyphs(WorkspaceIcon::Windows95, 1.5);
        let fine = folder_glyphs(WorkspaceIcon::Windows95, 2.);
        assert!(asset(coarse.closed).contains(r#"viewBox="0 0 16 16""#));
        assert!(asset(coarse.open).contains(r#"viewBox="0 0 16 16""#));
        assert!(asset(fine.closed).contains(r#"viewBox="0 0 32 32""#));
        assert!(asset(fine.open).contains(r#"viewBox="0 0 32 32""#));
        assert_eq!(coarse.size, fine.size);
    }

    #[test]
    fn a_grid_glyph_is_drawn_one_cell_per_pixel() {
        for icon in [WorkspaceIcon::Windows95, WorkspaceIcon::Pixel] {
            let glyphs = folder_glyphs(icon, 1.);
            let cells = format!(r#"viewBox="0 0 {0} {0}""#, glyphs.size);
            for path in [glyphs.closed, glyphs.open] {
                let svg = asset(path);
                assert!(svg.contains(&cells), "{path}");
                assert!(svg.contains(r#"shape-rendering="crispEdges""#), "{path}");
            }
        }
    }

    #[test]
    fn only_the_colored_glyphs_keep_their_own_ink() {
        for &icon in WorkspaceIcon::ALL {
            let own = folder_glyphs(icon, 1.).ink == FolderInk::Own;
            assert_eq!(
                own,
                matches!(icon, WorkspaceIcon::Windows | WorkspaceIcon::Windows95),
                "{icon:?}"
            );
        }
    }
}
