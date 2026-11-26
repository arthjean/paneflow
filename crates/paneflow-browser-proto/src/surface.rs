use serde::{Deserialize, Serialize};

pub const FRAME_SLOTS: u32 = 3;

pub const BYTES_PER_PIXEL: u32 = 4;

pub const MAX_VIEW_EDGE: u32 = 8192;

pub const MIN_SCALE: f32 = 0.5;

pub const MAX_SCALE: f32 = 4.0;

pub const DEFAULT_VIEW_WIDTH: u32 = 1280;

pub const DEFAULT_VIEW_HEIGHT: u32 = 800;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl View {
    pub fn new(width: u32, height: u32, scale: f32) -> Self {
        Self {
            width: width.clamp(1, MAX_VIEW_EDGE),
            height: height.clamp(1, MAX_VIEW_EDGE),
            scale: if scale.is_finite() {
                scale.clamp(MIN_SCALE, MAX_SCALE)
            } else {
                1.0
            },
        }
    }

    pub fn pixel_size(&self) -> (u32, u32) {
        let scale = f64::from(self.scale);
        let edge = |logical: u32| {
            let pixels = (f64::from(logical) * scale).round();
            if pixels.is_finite() && pixels >= 1.0 {
                (pixels as u32).min(MAX_VIEW_EDGE * 4)
            } else {
                1
            }
        };
        (edge(self.width), edge(self.height))
    }
}

impl Default for View {
    fn default() -> Self {
        Self::new(DEFAULT_VIEW_WIDTH, DEFAULT_VIEW_HEIGHT, 1.0)
    }
}

pub fn frame_bytes(width: u32, height: u32) -> Option<usize> {
    let stride = usize::try_from(width)
        .ok()?
        .checked_mul(BYTES_PER_PIXEL as usize)?;
    stride.checked_mul(usize::try_from(height).ok()?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const CAPS_LOCK: u32 = 1;
    pub const SHIFT: u32 = 1 << 1;
    pub const CONTROL: u32 = 1 << 2;
    pub const ALT: u32 = 1 << 3;
    pub const LEFT_BUTTON: u32 = 1 << 4;
    pub const MIDDLE_BUTTON: u32 = 1 << 5;
    pub const RIGHT_BUTTON: u32 = 1 << 6;
    pub const COMMAND: u32 = 1 << 7;
    pub const IS_KEY_PAD: u32 = 1 << 9;
    pub const IS_REPEAT: u32 = 1 << 13;

    pub fn with(self, flag: u32, on: bool) -> Self {
        if on {
            Self(self.0 | flag)
        } else {
            Self(self.0 & !flag)
        }
    }

    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    RawDown,
    Up,
    Char,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum Input {
    MouseMove {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    MouseLeave {
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    MouseButton {
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
        clicks: u32,
        modifiers: Modifiers,
    },
    Wheel {
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: Modifiers,
    },
    Key {
        kind: KeyKind,
        windows_key_code: i32,
        native_key_code: i32,
        character: u16,
        unmodified_character: u16,
        modifiers: Modifiers,
    },
    ImeSetComposition {
        text: String,
        selection_start: u32,
        selection_end: u32,
    },
    ImeCommit {
        text: String,
    },
    ImeFinish,
    ImeCancel,
    Focus {
        focused: bool,
    },
    CaptureLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStep {
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cursor {
    Default,
    Pointer,
    Text,
    Wait,
    Progress,
    Crosshair,
    Help,
    Move,
    NotAllowed,
    Grab,
    Grabbing,
    ResizeColumn,
    ResizeRow,
    ResizeEast,
    ResizeWest,
    ResizeNorth,
    ResizeSouth,
    ResizeEastWest,
    ResizeNorthSouth,
    ResizeNorthEastSouthWest,
    ResizeNorthWestSouthEast,
    ZoomIn,
    ZoomOut,
    ContextMenu,
    Copy,
    Alias,
    NoDrop,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    Open,
    OpenMultiple,
    OpenFolder,
    Save,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn clamped_into(&self, width: i32, height: i32) -> Rect {
        let width_fit = self.width.min(width).max(0);
        let height_fit = self.height.min(height).max(0);
        Rect {
            x: self.x.clamp(0, (width - width_fit).max(0)),
            y: self.y.clamp(0, (height - height_fit).max(0)),
            width: width_fit,
            height: height_fit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_is_clamped_to_a_drawable_size_and_scale() {
        let view = View::new(0, 90_000, f32::NAN);
        assert_eq!(
            (view.width, view.height, view.scale),
            (1, MAX_VIEW_EDGE, 1.0)
        );
        assert_eq!(View::new(540, 738, 9.0).scale, MAX_SCALE);
        assert_eq!(View::new(540, 738, 2.0).pixel_size(), (1080, 1476));
        assert_eq!(View::new(541, 739, 1.5).pixel_size(), (812, 1109));
    }

    #[test]
    fn a_frame_size_overflow_has_no_byte_count() {
        assert_eq!(frame_bytes(540, 738), Some(540 * 738 * 4));
        assert_eq!(frame_bytes(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn a_popup_past_the_bottom_edge_is_moved_inside_the_view() {
        let popup = Rect {
            x: 400,
            y: 700,
            width: 200,
            height: 120,
        };
        assert_eq!(
            popup.clamped_into(540, 738),
            Rect {
                x: 340,
                y: 618,
                width: 200,
                height: 120
            }
        );
        let wider = Rect {
            x: -10,
            y: 0,
            width: 900,
            height: 10,
        };
        assert_eq!(wider.clamped_into(540, 738).width, 540);
        assert_eq!(wider.clamped_into(540, 738).x, 0);
    }

    #[test]
    fn modifier_flags_toggle_independently() {
        let modifiers = Modifiers::default()
            .with(Modifiers::SHIFT, true)
            .with(Modifiers::CONTROL, true)
            .with(Modifiers::SHIFT, false);
        assert!(modifiers.has(Modifiers::CONTROL));
        assert!(!modifiers.has(Modifiers::SHIFT));
    }
}
