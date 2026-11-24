use std::borrow::Cow;
use std::cell::Cell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use gpui::{
    AnyElement, AppContext, Bounds, ClickEvent, Context, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollEvent, SharedString,
    Styled, Window, canvas, div, img, prelude::FluentBuilder, px, svg,
};

use super::large_text::PreviewEvent;
use super::{NoPreview, OpenedFile, format_size, lowered_extension};
use crate::ui_primitives::AccessibleControlExt;

pub(crate) const IMAGE_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub(crate) const IMAGE_MAX_SIDE: u32 = 16_384;
pub(crate) const IMAGE_MAX_PIXELS: u64 = 32_000_000;
pub(crate) const ZOOM_STEP: f32 = 1.25;
pub(crate) const ZOOM_MIN: f32 = 0.05;
pub(crate) const ZOOM_MAX: f32 = 8.0;
pub(crate) const FIT_MARGIN: f32 = 16.0;
pub(crate) const LOADING_IMAGE: &str = "Loading image…";
pub(crate) const UNABLE_TO_LOAD: &str = "Unable to load image.";
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const SVG_RASTER_MAX_SIDE: f32 = 4096.0;
const SVG_RASTER_SCALE: f32 = 2.0;
const TOOLBAR_HEIGHT: f32 = 32.0;
const WHEEL_LINE_PX: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageFormatKind {
    Raster,
    Svg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageProbe {
    pub(crate) format: ImageFormatKind,
    pub(crate) size: u64,
    pub(crate) dimensions: Option<(u32, u32)>,
}

pub(crate) fn caps_refusal(width: u32, height: u32, size: u64) -> Option<NoPreview> {
    if width > IMAGE_MAX_SIDE || height > IMAGE_MAX_SIDE {
        return Some(NoPreview::new(
            Some(size),
            format!(
                "This image is {width} × {height} pixels, past the 16,384-pixel limit per side."
            ),
        ));
    }
    if u64::from(width) * u64::from(height) > IMAGE_MAX_PIXELS {
        return Some(NoPreview::new(
            Some(size),
            format!("This image is {width} × {height} pixels, past the 32-megapixel limit."),
        ));
    }
    None
}

pub(crate) fn probe(path: &Path, size: u64) -> OpenedFile {
    let ext = lowered_extension(path);
    if super::UNSUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return OpenedFile::NoPreview(NoPreview::new(
            Some(size),
            format!("{} images can't be previewed.", ext.to_ascii_uppercase()),
        ));
    }
    if size > IMAGE_MAX_BYTES {
        return OpenedFile::NoPreview(NoPreview::new(
            Some(size),
            format!(
                "This image is larger than {}, the preview limit.",
                format_size(IMAGE_MAX_BYTES)
            ),
        ));
    }
    if ext == "svg" {
        return OpenedFile::Image(ImageProbe {
            format: ImageFormatKind::Svg,
            size,
            dimensions: None,
        });
    }
    let dimensions = ::image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.into_dimensions().ok());
    if let Some((width, height)) = dimensions
        && let Some(refusal) = caps_refusal(width, height, size)
    {
        return OpenedFile::NoPreview(refusal);
    }
    OpenedFile::Image(ImageProbe {
        format: ImageFormatKind::Raster,
        size,
        dimensions,
    })
}

#[derive(Clone)]
pub(crate) struct DecodedImage {
    pub(crate) image: Arc<RenderImage>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ImageFailure {
    Unreadable,
    Refused(NoPreview),
}

pub(crate) fn decode(path: &Path, probe: ImageProbe) -> Result<DecodedImage, ImageFailure> {
    match probe.format {
        ImageFormatKind::Raster => decode_raster(path, probe.size),
        ImageFormatKind::Svg => decode_svg(path, probe.size),
    }
}

fn limits() -> ::image::Limits {
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(IMAGE_MAX_SIDE);
    limits.max_image_height = Some(IMAGE_MAX_SIDE);
    limits.max_alloc = Some(IMAGE_MAX_PIXELS * 4 * 2);
    limits
}

fn decode_raster(path: &Path, size: u64) -> Result<DecodedImage, ImageFailure> {
    let open = || {
        ::image::ImageReader::open(path)
            .and_then(|reader| reader.with_guessed_format())
            .map_err(|_| ImageFailure::Unreadable)
    };
    let (width, height) = open()?
        .into_dimensions()
        .map_err(|_| ImageFailure::Unreadable)?;
    if let Some(refusal) = caps_refusal(width, height, size) {
        return Err(ImageFailure::Refused(refusal));
    }
    let mut reader = open()?;
    reader.limits(limits());
    let decoded = reader.decode().map_err(|_| ImageFailure::Unreadable)?;
    Ok(to_render_image(decoded.into_rgba8()))
}

fn to_render_image(mut rgba: ::image::RgbaImage) -> DecodedImage {
    let (width, height) = rgba.dimensions();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    DecodedImage {
        image: Arc::new(RenderImage::new([::image::Frame::new(rgba)])),
        width,
        height,
    }
}

fn svg_fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = resvg::usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone()
}

pub(crate) fn svg_options() -> resvg::usvg::Options<'static> {
    let mut options = resvg::usvg::Options {
        resources_dir: None,
        fontdb: svg_fontdb(),
        ..Default::default()
    };
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options
}

fn decode_svg(path: &Path, size: u64) -> Result<DecodedImage, ImageFailure> {
    let data =
        super::read_head(path, IMAGE_MAX_BYTES as usize).map_err(|_| ImageFailure::Unreadable)?;
    rasterize_svg(&data, size)
}

fn svg_source(data: &[u8], size: u64) -> Result<Cow<'_, [u8]>, ImageFailure> {
    if !data.starts_with(&GZIP_MAGIC) {
        return Ok(Cow::Borrowed(data));
    }
    let mut inflated = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(IMAGE_MAX_BYTES + 1)
        .read_to_end(&mut inflated)
        .map_err(|_| ImageFailure::Unreadable)?;
    if inflated.len() as u64 > IMAGE_MAX_BYTES {
        return Err(ImageFailure::Refused(NoPreview::new(
            Some(size),
            format!(
                "This compressed SVG expands past {}, the preview limit.",
                format_size(IMAGE_MAX_BYTES)
            ),
        )));
    }
    if inflated.starts_with(&GZIP_MAGIC) {
        return Err(ImageFailure::Unreadable);
    }
    Ok(Cow::Owned(inflated))
}

pub(crate) fn rasterize_svg(data: &[u8], size: u64) -> Result<DecodedImage, ImageFailure> {
    let source = svg_source(data, size)?;
    let tree = resvg::usvg::Tree::from_data(&source, &svg_options())
        .map_err(|_| ImageFailure::Unreadable)?;
    let intrinsic = tree.size();
    let (width, height) = (
        intrinsic.width().ceil().max(1.0),
        intrinsic.height().ceil().max(1.0),
    );
    if let Some(refusal) = caps_refusal(width as u32, height as u32, size) {
        return Err(ImageFailure::Refused(refusal));
    }
    let scale = SVG_RASTER_SCALE
        .min(SVG_RASTER_MAX_SIDE / width.max(height))
        .max(0.01);
    let raster_w = (width * scale).ceil().max(1.0) as u32;
    let raster_h = (height * scale).ceil().max(1.0) as u32;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(raster_w, raster_h).ok_or(ImageFailure::Unreadable)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = Vec::with_capacity(raster_w as usize * raster_h as usize * 4);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    let buffer =
        ::image::RgbaImage::from_raw(raster_w, raster_h, rgba).ok_or(ImageFailure::Unreadable)?;
    let mut decoded = to_render_image(buffer);
    decoded.width = width as u32;
    decoded.height = height as u32;
    Ok(decoded)
}

pub(crate) fn fit_scale(width: u32, height: u32, area: (f32, f32)) -> f32 {
    if width == 0 || height == 0 {
        return 1.0;
    }
    let room_w = (area.0 - 2.0 * FIT_MARGIN).max(1.0);
    let room_h = (area.1 - 2.0 * FIT_MARGIN).max(1.0);
    (room_w / width as f32)
        .min(room_h / height as f32)
        .clamp(ZOOM_MIN, 1.0)
}

pub(crate) fn step_zoom(current: f32, zoom_in: bool) -> f32 {
    let next = if zoom_in {
        current * ZOOM_STEP
    } else {
        current / ZOOM_STEP
    };
    next.clamp(ZOOM_MIN, ZOOM_MAX)
}

pub(crate) fn zoom_label(scale: f32) -> String {
    format!("{}%", (scale * 100.0).round() as i64)
}

struct ToolbarButton {
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    enabled: bool,
}

enum ImageState {
    Loading,
    Ready(DecodedImage),
    Failed,
}

pub(crate) struct ImageView {
    path: PathBuf,
    probe: ImageProbe,
    focus: FocusHandle,
    state: ImageState,
    zoom: Option<f32>,
    scroll: Point<f32>,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<(Point<Pixels>, Point<f32>)>,
    generation: u64,
}

impl EventEmitter<PreviewEvent> for ImageView {}

impl ImageView {
    pub(crate) fn new(
        path: PathBuf,
        probe: ImageProbe,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|view: &mut Self, cx| {
            if let ImageState::Ready(decoded) = &view.state {
                cx.drop_image(decoded.image.clone(), None);
            }
        })
        .detach();
        let mut view = Self {
            path,
            probe,
            focus,
            state: ImageState::Loading,
            zoom: None,
            scroll: Point::default(),
            viewport: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            generation: 0,
        };
        view.start_decode(cx);
        view
    }

    pub(crate) fn reload(&mut self, probe: ImageProbe, cx: &mut Context<Self>) {
        self.probe = probe;
        self.start_decode(cx);
    }

    fn start_decode(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let path = self.path.clone();
        let probe = self.probe;
        let work = cx.background_spawn(async move { decode(&path, probe) });
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |view: &mut Self, cx| {
                if view.generation != generation {
                    return;
                }
                let previous = std::mem::replace(&mut view.state, ImageState::Loading);
                if let ImageState::Ready(old) = previous {
                    cx.drop_image(old.image, None);
                }
                view.state = match outcome {
                    Ok(decoded) => ImageState::Ready(decoded),
                    Err(ImageFailure::Unreadable) => ImageState::Failed,
                    Err(ImageFailure::Refused(screen)) => {
                        cx.emit(PreviewEvent::Refused(screen));
                        ImageState::Failed
                    }
                };
                view.clamp_scroll();
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self.state, ImageState::Loading)
    }

    #[cfg(test)]
    pub(crate) fn has_failed(&self) -> bool {
        matches!(self.state, ImageState::Failed)
    }

    pub(crate) fn dimensions(&self) -> Option<(u32, u32)> {
        match &self.state {
            ImageState::Ready(decoded) => Some((decoded.width, decoded.height)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn zoom_mode(&self) -> Option<f32> {
        self.zoom
    }

    fn area(&self) -> (f32, f32) {
        let size = self.viewport.get().size;
        (f32::from(size.width), f32::from(size.height))
    }

    pub(crate) fn scale(&self) -> f32 {
        let Some((width, height)) = self.dimensions() else {
            return 1.0;
        };
        self.zoom
            .unwrap_or_else(|| fit_scale(width, height, self.area()))
    }

    fn display_size(&self) -> (f32, f32) {
        let scale = self.scale();
        self.dimensions().map_or((0.0, 0.0), |(width, height)| {
            (width as f32 * scale, height as f32 * scale)
        })
    }

    fn clamp_scroll(&mut self) {
        let (area_w, area_h) = self.area();
        let (shown_w, shown_h) = self.display_size();
        self.scroll.x = self.scroll.x.clamp(0.0, (shown_w - area_w).max(0.0));
        self.scroll.y = self.scroll.y.clamp(0.0, (shown_h - area_h).max(0.0));
    }

    pub(crate) fn set_zoom(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        let before = self.scale();
        let (area_w, area_h) = self.area();
        let centre = Point {
            x: self.scroll.x + area_w / 2.0,
            y: self.scroll.y + area_h / 2.0,
        };
        self.zoom = zoom.map(|zoom| zoom.clamp(ZOOM_MIN, ZOOM_MAX));
        let ratio = self.scale() / before.max(f32::EPSILON);
        self.scroll = Point {
            x: centre.x * ratio - area_w / 2.0,
            y: centre.y * ratio - area_h / 2.0,
        };
        self.clamp_scroll();
        cx.notify();
    }

    pub(crate) fn zoom_by_step(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        let next = step_zoom(self.scale(), zoom_in);
        self.set_zoom(Some(next), cx);
    }

    pub(crate) fn pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.scroll.x += dx;
        self.scroll.y += dy;
        self.clamp_scroll();
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn scroll_offset(&self) -> Point<f32> {
        self.scroll
    }

    #[cfg(test)]
    pub(crate) fn set_viewport_for_test(&mut self, size: gpui::Size<Pixels>) {
        self.viewport.set(Bounds {
            origin: Point::default(),
            size,
        });
    }

    fn toolbar_button(
        &self,
        button: ToolbarButton,
        ui: crate::theme::UiColors,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ToolbarButton {
            id,
            label,
            icon,
            enabled,
        } = button;
        let hover = crate::app::constants::sidebar_tab_hover_background();
        let button = div().id(id);
        #[cfg(test)]
        let button = button.debug_selector(|| id.to_string());
        button
            .flex_none()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .accessible_control(gpui::accesskit::Role::Button, label)
            .when(enabled, |button| {
                button
                    .hover(move |style| style.bg(hover))
                    .on_activate(cx.listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
            })
            .child(svg().size(px(14.)).path(icon).text_color(if enabled {
                ui.text
            } else {
                ui.muted.opacity(0.5)
            }))
            .into_any_element()
    }

    fn render_toolbar(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let ready = self.dimensions().is_some();
        let scale = self.scale();
        let lead: SharedString = match (&self.state, self.dimensions()) {
            (ImageState::Loading, _) => LOADING_IMAGE.into(),
            (_, Some((width, height))) => format!("{width} × {height}").into(),
            _ => SharedString::default(),
        };
        let fit = self.zoom.is_none();
        div()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .border_b_1()
            .border_color(ui.border)
            .text_size(crate::ui_primitives::BODY)
            .text_color(ui.muted)
            .child(
                div()
                    .id("image-toolbar-size")
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(lead),
            )
            .child(self.toolbar_button(
                ToolbarButton {
                    id: "image-zoom-out",
                    label: "Zoom out",
                    icon: "icons/palette/zoom-out.svg",
                    enabled: ready && scale > ZOOM_MIN,
                },
                ui,
                |this, cx| this.zoom_by_step(false, cx),
                cx,
            ))
            .child(
                div()
                    .id("image-zoom-level")
                    .flex_none()
                    .w(px(44.))
                    .flex()
                    .justify_center()
                    .text_color(ui.text)
                    .child(if ready {
                        SharedString::from(zoom_label(scale))
                    } else {
                        SharedString::default()
                    }),
            )
            .child(self.toolbar_button(
                ToolbarButton {
                    id: "image-zoom-in",
                    label: "Zoom in",
                    icon: "icons/palette/zoom-in.svg",
                    enabled: ready && scale < ZOOM_MAX,
                },
                ui,
                |this, cx| this.zoom_by_step(true, cx),
                cx,
            ))
            .child(self.toolbar_button(
                ToolbarButton {
                    id: "image-zoom-fit",
                    label: "Fit",
                    icon: "icons/palette/maximize-screen.svg",
                    enabled: ready && !fit,
                },
                ui,
                |this, cx| this.set_zoom(None, cx),
                cx,
            ))
            .child(self.toolbar_button(
                ToolbarButton {
                    id: "image-zoom-actual",
                    label: "Actual size",
                    icon: "icons/dock/actual-size.svg",
                    enabled: ready && self.zoom != Some(1.0),
                },
                ui,
                |this, cx| this.set_zoom(Some(1.0), cx),
                cx,
            ))
            .into_any_element()
    }
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        self.clamp_scroll();
        let toolbar = self.render_toolbar(ui, cx);
        let viewport = self.viewport.clone();
        let (area_w, area_h) = self.area();
        if area_w <= 0.0 {
            window.request_animation_frame();
        }
        let content: AnyElement = match &self.state {
            ImageState::Loading => div().into_any_element(),
            ImageState::Failed => crate::ui_primitives::panel_empty_state(
                ui,
                Some("icons/dock/alert-02.svg"),
                None,
                UNABLE_TO_LOAD,
                false,
            )
            .into_any_element(),
            ImageState::Ready(decoded) => {
                let (shown_w, shown_h) = self.display_size();
                let left = if shown_w <= area_w {
                    (area_w - shown_w) / 2.0
                } else {
                    -self.scroll.x
                };
                let top = if shown_h <= area_h {
                    (area_h - shown_h) / 2.0
                } else {
                    -self.scroll.y
                };
                img(decoded.image.clone())
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(shown_w))
                    .h(px(shown_h))
                    .into_any_element()
            }
        };
        let grabbing = self.drag.is_some();
        div().size_full().flex().flex_col().child(toolbar).child(
            div()
                .id("image-canvas")
                .track_focus(&self.focus)
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .cursor(if grabbing {
                    gpui::CursorStyle::ClosedHand
                } else {
                    gpui::CursorStyle::OpenHand
                })
                .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                    let (dx, dy) = match event.delta {
                        ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                        ScrollDelta::Lines(delta) => {
                            (delta.x * WHEEL_LINE_PX, delta.y * WHEEL_LINE_PX)
                        }
                    };
                    this.pan_by(-dx, -dy, cx);
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.drag = Some((event.position, this.scroll));
                        cx.notify();
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if let Some((start, scroll)) = this.drag {
                        let delta = event.position - start;
                        this.scroll = Point {
                            x: scroll.x - f32::from(delta.x),
                            y: scroll.y - f32::from(delta.y),
                        };
                        this.clamp_scroll();
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.drag = None;
                        cx.notify();
                    }),
                )
                .child(
                    canvas(move |bounds, _, _| viewport.set(bounds), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
                .child(content),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn write_png(dir: &tempfile::TempDir, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.path().join(name);
        ::image::RgbaImage::from_pixel(width, height, ::image::Rgba([200, 10, 20, 255]))
            .save(&path)
            .expect("save png");
        path
    }

    pub(crate) fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"IHDR");
        chunk.extend_from_slice(&width.to_be_bytes());
        chunk.extend_from_slice(&height.to_be_bytes());
        chunk.extend_from_slice(&[8, 6, 0, 0, 0]);
        let crc_of = |bytes: &[u8]| {
            let mut crc = flate2::Crc::new();
            crc.update(bytes);
            crc.sum().to_be_bytes()
        };
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\x0d".to_vec();
        header.extend_from_slice(&chunk);
        header.extend_from_slice(&crc_of(&chunk));
        header.extend_from_slice(&[0, 0, 0, 0]);
        header.extend_from_slice(b"IDAT");
        header.extend_from_slice(&crc_of(b"IDAT"));
        header
    }

    fn probe_of(path: &Path) -> OpenedFile {
        let size = std::fs::metadata(path).expect("meta").len();
        probe(path, size)
    }

    #[test]
    fn a_png_decodes_to_bgra_pixels_at_its_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_png(&dir, "red.png", 3, 2);
        let OpenedFile::Image(probe) = probe_of(&path) else {
            panic!("a png is an image");
        };
        assert_eq!(probe.dimensions, Some((3, 2)));
        let decoded = decode(&path, probe).expect("decode");
        assert_eq!((decoded.width, decoded.height), (3, 2));
        let frame = decoded.image.as_bytes(0).expect("frame");
        assert_eq!(&frame[..4], &[20, 10, 200, 255]);
    }

    #[test]
    fn every_listed_format_decodes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pixels = ::image::RgbaImage::from_pixel(4, 4, ::image::Rgba([1, 2, 3, 255]));
        for ext in ["png", "jpg", "gif", "webp", "bmp", "ico", "tiff"] {
            let path = dir.path().join(format!("sample.{ext}"));
            let saved = if ext == "jpg" {
                ::image::DynamicImage::ImageRgba8(pixels.clone())
                    .to_rgb8()
                    .save(&path)
            } else {
                pixels.save(&path)
            };
            saved.unwrap_or_else(|err| panic!("save {ext}: {err}"));
            let OpenedFile::Image(probe) = probe_of(&path) else {
                panic!("{ext} is an image");
            };
            let decoded = decode(&path, probe).unwrap_or_else(|err| panic!("{ext}: {err:?}"));
            assert_eq!((decoded.width, decoded.height), (4, 4), "{ext}");
        }
    }

    #[test]
    fn an_image_past_the_caps_is_refused_from_its_header_alone() {
        assert!(caps_refusal(IMAGE_MAX_SIDE, 10, 1).is_none());
        assert!(caps_refusal(IMAGE_MAX_SIDE + 1, 10, 1).is_some());
        assert!(caps_refusal(10, IMAGE_MAX_SIDE + 1, 1).is_some());
        let refusal = caps_refusal(8000, 5000, 1).expect("40 megapixels");
        assert!(refusal.reason.expect("reason").contains("32-megapixel"));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wide.png");
        std::fs::write(&path, png_header(20_000, 10)).expect("write header");
        let OpenedFile::NoPreview(screen) = probe_of(&path) else {
            panic!("a 20,000-pixel side is refused before decoding");
        };
        assert!(screen.reason.expect("reason").contains("20000 × 10"));
    }

    #[test]
    fn a_file_past_fifty_megabytes_is_refused_without_reading() {
        let OpenedFile::NoPreview(screen) =
            probe(Path::new("/nonexistent/huge.png"), IMAGE_MAX_BYTES + 1)
        else {
            panic!("refused by size");
        };
        assert!(screen.reason.expect("reason").contains("50 MB"));
    }

    #[test]
    fn avif_and_heic_land_on_the_no_preview_screen_and_garbage_fails_to_load() {
        for name in ["photo.avif", "photo.HEIC"] {
            assert!(matches!(
                probe(Path::new(name), 10),
                OpenedFile::NoPreview(_)
            ));
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nnot really").expect("write");
        let OpenedFile::Image(probe) = probe_of(&path) else {
            panic!("a broken png still opens the viewer");
        };
        assert_eq!(decode(&path, probe).err(), Some(ImageFailure::Unreadable));
    }

    #[test]
    fn an_svg_renders_without_loading_external_resources() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secret = write_png(&dir, "secret.png", 8, 8);
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' \
             width='20' height='10'><script>alert(1)</script>\
             <image width='20' height='10' xlink:href='{}'/></svg>",
            secret.display()
        );
        let decoded = rasterize_svg(svg.as_bytes(), svg.len() as u64).expect("svg");
        assert_eq!((decoded.width, decoded.height), (20, 10));
        let frame = decoded.image.as_bytes(0).expect("frame");
        assert!(
            frame.chunks(4).all(|pixel| pixel[3] == 0),
            "the referenced file was not drawn"
        );

        let drawn = rasterize_svg(
            b"<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'>\
              <rect width='4' height='4' fill='#ff0000'/></svg>",
            1,
        )
        .expect("svg");
        let frame = drawn.image.as_bytes(0).expect("frame");
        assert_eq!(&frame[..4], &[0, 0, 255, 255]);
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut encoder, bytes).expect("gzip");
        encoder.finish().expect("gzip")
    }

    #[test]
    fn a_compressed_svg_renders_and_one_that_expands_past_the_limit_is_refused() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'>\
                    <rect width='4' height='4' fill='#00ff00'/></svg>";
        let drawn = rasterize_svg(&gzip(svg), 1).expect("compressed svg");
        assert_eq!((drawn.width, drawn.height), (4, 4));

        let bomb = gzip(&vec![b' '; IMAGE_MAX_BYTES as usize + 1]);
        assert!(bomb.len() < 1024 * 1024, "{} bytes", bomb.len());
        let Err(ImageFailure::Refused(screen)) = rasterize_svg(&bomb, bomb.len() as u64) else {
            panic!("an svg that inflates past the limit is refused");
        };
        assert!(screen.reason.expect("reason").contains("50 MB"));

        let nested = gzip(&gzip(svg));
        assert_eq!(
            rasterize_svg(&nested, 1).err(),
            Some(ImageFailure::Unreadable),
            "a doubly compressed svg is not handed to the parser"
        );
    }

    #[test]
    fn fit_never_passes_actual_size_and_keeps_a_sixteen_pixel_margin() {
        assert_eq!(fit_scale(100, 100, (800.0, 600.0)), 1.0);
        let scale = fit_scale(2000, 1000, (432.0, 600.0));
        assert!((scale - 0.2).abs() < 1e-6, "{scale}");
        assert_eq!(step_zoom(1.0, true), 1.25);
        assert_eq!(step_zoom(1.0, false), 0.8);
        assert_eq!(step_zoom(7.0, true), ZOOM_MAX);
        assert_eq!(step_zoom(0.051, false), ZOOM_MIN);
        assert_eq!(zoom_label(1.25), "125%");
    }
}
