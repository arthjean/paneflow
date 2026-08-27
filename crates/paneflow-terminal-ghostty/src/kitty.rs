//! Read the Kitty graphics protocol's image storage.
//!
//! A program transmits an image, then places it on the grid one or more
//! times. libghostty stores the decoded pixels and the placements; drawing
//! them is the embedder's job. This module is the read side of that contract:
//! iterate the placements, resolve each one's image, and ask libghostty for
//! the geometry a renderer needs.
//!
//! Everything here borrows from the terminal and is invalidated by any
//! mutating terminal call, which is why every handle holds a shared borrow of
//! the [`DisplayTerminal`]: writing to the terminal needs `&mut`, so the
//! borrow checker enforces libghostty's rule.
//!
//! The protocol is off by default. [`DisplayTerminal::enable_kitty_graphics`]
//! is the opt-in, and it deliberately leaves the file, temporary-file, and
//! shared-memory transmission media disabled, so only payloads the program
//! sends inline are ever stored.

use std::ffi::c_void;
use std::marker::PhantomData;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::handles::{OwnedHandle, check, create};
use crate::selection::empty_selection;
use crate::{GhosttyError, Result, SelectionRange};

/// Which placements an iterator yields, by the protocol's z-index bands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlacementLayer {
    /// Every placement, in storage order.
    #[default]
    All,
    /// Below the cell background.
    BelowBackground,
    /// Above the background, below the text.
    BelowText,
    /// Above the text.
    AboveText,
}

impl PlacementLayer {
    fn raw(self) -> sys::GhosttyKittyPlacementLayer {
        match self {
            Self::All => sys::GhosttyKittyPlacementLayer_GHOSTTY_KITTY_PLACEMENT_LAYER_ALL,
            Self::BelowBackground => {
                sys::GhosttyKittyPlacementLayer_GHOSTTY_KITTY_PLACEMENT_LAYER_BELOW_BG
            }
            Self::BelowText => {
                sys::GhosttyKittyPlacementLayer_GHOSTTY_KITTY_PLACEMENT_LAYER_BELOW_TEXT
            }
            Self::AboveText => {
                sys::GhosttyKittyPlacementLayer_GHOSTTY_KITTY_PLACEMENT_LAYER_ABOVE_TEXT
            }
        }
    }
}

/// The pixel layout of a stored image.
///
/// [`Self::Png`] never comes back from storage: libghostty decodes PNG
/// payloads to [`Self::Rgba`] before storing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Three bytes per pixel.
    Rgb,
    /// Four bytes per pixel.
    Rgba,
    /// A PNG payload, only ever seen in transit.
    Png,
    /// Two bytes per pixel.
    GrayAlpha,
    /// One byte per pixel.
    Gray,
}

impl ImageFormat {
    fn from_raw(raw: sys::GhosttyKittyImageFormat) -> Result<Self> {
        match raw {
            sys::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_RGB => Ok(Self::Rgb),
            sys::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => Ok(Self::Rgba),
            sys::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_PNG => Ok(Self::Png),
            sys::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => {
                Ok(Self::GrayAlpha)
            }
            sys::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => Ok(Self::Gray),
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown kitty image format {other}"
            ))),
        }
    }

    /// Bytes one pixel occupies, or `None` for a format that is not a plain
    /// pixel array.
    #[must_use]
    pub fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgb => Some(3),
            Self::Rgba => Some(4),
            Self::GrayAlpha => Some(2),
            Self::Gray => Some(1),
            Self::Png => None,
        }
    }
}

/// How a stored image's payload is compressed.
///
/// Always [`Self::None`] in storage: deflate payloads are inflated on
/// transmission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageCompression {
    /// Raw pixels.
    None,
    /// A zlib deflate stream, only ever seen in transit.
    ZlibDeflate,
}

impl ImageCompression {
    fn from_raw(raw: sys::GhosttyKittyImageCompression) -> Result<Self> {
        match raw {
            sys::GhosttyKittyImageCompression_GHOSTTY_KITTY_IMAGE_COMPRESSION_NONE => Ok(Self::None),
            sys::GhosttyKittyImageCompression_GHOSTTY_KITTY_IMAGE_COMPRESSION_ZLIB_DEFLATE => {
                Ok(Self::ZlibDeflate)
            }
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown kitty image compression {other}"
            ))),
        }
    }
}

/// A stored image's metadata, read in one batched call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    /// Protocol image ID, the key placements refer to.
    pub id: u32,
    /// Protocol image number, the client's own alias for the ID.
    pub number: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel layout of [`KittyImage::pixels`].
    pub format: ImageFormat,
    /// Payload compression, always [`ImageCompression::None`] in storage.
    pub compression: ImageCompression,
    /// Payload length in bytes. For an image whose pixels have not arrived
    /// yet, this is the length reserved against the storage budget.
    pub len: usize,
    /// Stamp that changes on every add or replace of this image ID.
    ///
    /// A texture cache must key staleness on this: retransmitting the same
    /// ID with the same dimensions changes the pixels and nothing else.
    pub generation: u64,
}

/// One placement's protocol fields, read in one batched call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The image this places.
    pub image_id: u32,
    /// The placement's own ID, unique within the image.
    pub placement_id: u32,
    /// A Unicode-placeholder placement, positioned by cell content rather
    /// than by a pin, so it has no grid rectangle of its own.
    pub virtual_placement: bool,
    /// Pixel offset from the left edge of the anchor cell.
    pub x_offset: u32,
    /// Pixel offset from the top edge of the anchor cell.
    pub y_offset: u32,
    /// Source rectangle origin, before clamping. See
    /// [`PlacementCursor::source_rect`] for the resolved one.
    pub source_x: u32,
    /// Source rectangle origin, before clamping.
    pub source_y: u32,
    /// Source width, where zero means the full image.
    pub source_width: u32,
    /// Source height, where zero means the full image.
    pub source_height: u32,
    /// Columns the program asked the placement to span, or zero.
    pub columns: u32,
    /// Rows the program asked the placement to span, or zero.
    pub rows: u32,
    /// Z-index, which decides the layer and the draw order within it.
    pub z: i32,
}

/// A source rectangle resolved against the image bounds, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Everything a renderer needs to draw one placement, in one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementRenderInfo {
    /// Rendered width in pixels.
    pub pixel_width: u32,
    /// Rendered height in pixels.
    pub pixel_height: u32,
    /// Grid columns the placement covers.
    pub grid_cols: u32,
    /// Grid rows the placement covers.
    pub grid_rows: u32,
    /// Viewport-relative top-left cell, or `None` when the placement is fully
    /// scrolled out or virtual. The row goes negative when the top of the
    /// image has scrolled above the viewport, and clipping is the renderer's
    /// job.
    pub viewport: Option<(i32, i32)>,
    /// Source rectangle to sample.
    pub source: SourceRect,
}

impl DisplayTerminal {
    /// Turn on the Kitty graphics protocol.
    ///
    /// `storage_bytes` budgets decoded pixels for the active screen, and
    /// `command_bytes` caps what one protocol command may buffer while it
    /// arrives. Both are off by default, which is what makes a terminal
    /// ignore image transmissions entirely.
    ///
    /// This does not enable the file, temporary-file, or shared-memory
    /// transmission media: those let a program name a path for the terminal
    /// to read, and they stay off. Inline payloads still work, and PNG
    /// payloads additionally need a decoder installed with
    /// [`crate::set_png_decoder`].
    pub fn enable_kitty_graphics(&mut self, storage_bytes: u64, command_bytes: usize) -> Result<()> {
        // SAFETY: the terminal handle is live and each option's documented
        // input type is the one being passed.
        unsafe {
            self.set_option(
                "terminal_set_kitty_image_storage_limit",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&raw const storage_bytes).cast::<c_void>(),
            )?;
            self.set_option(
                "terminal_set_apc_max_bytes_kitty",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY,
                (&raw const command_bytes).cast::<c_void>(),
            )
        }
    }

    /// Turn the Kitty graphics protocol back off, dropping stored images.
    pub fn disable_kitty_graphics(&mut self) -> Result<()> {
        self.enable_kitty_graphics(0, 0)
    }

    /// # Safety
    ///
    /// `value` must point to a value of the option's documented input type
    /// and stay live for the duration of the call.
    unsafe fn set_option(
        &mut self,
        operation: &'static str,
        option: sys::GhosttyTerminalOption,
        value: *const c_void,
    ) -> Result<()> {
        // SAFETY: the terminal handle is live and the caller guarantees the
        // value's type.
        let result = unsafe { sys::ghostty_terminal_set(self.terminal.raw(), option, value) };
        check(operation, result)
    }

    /// The active screen's image storage.
    ///
    /// `None` when the protocol is disabled or the build has no Kitty
    /// graphics support. The storage belongs to the active screen, so
    /// switching to the alternate screen shows a different one.
    pub fn kitty_graphics(&self) -> Result<Option<KittyGraphics<'_>>> {
        let mut raw: sys::GhosttyKittyGraphics = std::ptr::null_mut();
        // SAFETY: the terminal handle is live and `raw` has the
        // `GhosttyKittyGraphics *` output type terminal.h documents.
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
                (&raw mut raw).cast::<c_void>(),
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("terminal_get_kitty_graphics", result)?;
        Ok((!raw.is_null()).then_some(KittyGraphics {
            raw,
            terminal: self,
        }))
    }
}

/// The Kitty image storage of one screen, borrowed from its terminal.
pub struct KittyGraphics<'terminal> {
    raw: sys::GhosttyKittyGraphics,
    terminal: &'terminal DisplayTerminal,
}

impl<'terminal> KittyGraphics<'terminal> {
    /// Stamp of the last change to the stored images and placements.
    ///
    /// Unchanged since the last frame means the placement set and every
    /// image are byte-identical, so a renderer can skip iterating entirely.
    /// Placement *geometry* still moves with scrolling, so a dirty frame must
    /// still recompute [`PlacementCursor::render_info`]. Zero means nothing
    /// has ever been stored.
    pub fn generation(&self) -> Result<u64> {
        let mut generation = 0u64;
        // SAFETY: the storage handle is live and `generation` has the
        // `uint64_t *` output type the key documents.
        let result = unsafe {
            sys::ghostty_kitty_graphics_get(
                self.raw,
                sys::GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_GENERATION,
                (&raw mut generation).cast::<c_void>(),
            )
        };
        check("kitty_graphics_get_generation", result)?;
        Ok(generation)
    }

    /// Look up a stored image by its protocol ID.
    #[must_use]
    pub fn image(&self, image_id: u32) -> Option<KittyImage<'terminal>> {
        // SAFETY: the storage handle is live; the call returns NULL rather
        // than failing when no such image exists.
        let raw = unsafe { sys::ghostty_kitty_graphics_image(self.raw, image_id) };
        (!raw.is_null()).then_some(KittyImage {
            raw,
            _terminal: PhantomData,
        })
    }

    /// Walk the placements in `layer`.
    pub fn placements(&self, layer: PlacementLayer) -> Result<PlacementCursor<'terminal>> {
        // SAFETY: the null allocator selects libghostty's default, and
        // `placement_iterator_free` is the matching destructor.
        let iterator = unsafe {
            create(
                "kitty_placement_iterator_new",
                std::ptr::null(),
                sys::ghostty_kitty_graphics_placement_iterator_new,
                sys::ghostty_kitty_graphics_placement_iterator_free,
            )?
        };
        // A fresh iterator holds nothing but its allocator until the storage
        // fills it in, so this has to happen before the layer filter.
        let mut raw_iterator = iterator.raw();
        // SAFETY: both handles are live and `raw_iterator` has the
        // `GhosttyKittyGraphicsPlacementIterator *` output type the key
        // documents.
        let result = unsafe {
            sys::ghostty_kitty_graphics_get(
                self.raw,
                sys::GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR,
                (&raw mut raw_iterator).cast::<c_void>(),
            )
        };
        check("kitty_graphics_get_placement_iterator", result)?;
        let layer = layer.raw();
        // SAFETY: the iterator is live and the option's documented input type
        // is `GhosttyKittyPlacementLayer *`.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_iterator_set(
                iterator.raw(),
                sys::GhosttyKittyGraphicsPlacementIteratorOption_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_ITERATOR_OPTION_LAYER,
                (&raw const layer).cast::<c_void>(),
            )
        };
        check("kitty_placement_iterator_set_layer", result)?;
        Ok(PlacementCursor {
            iterator,
            terminal: self.terminal,
        })
    }
}

/// One stored image, borrowed from its terminal.
pub struct KittyImage<'terminal> {
    raw: sys::GhosttyKittyGraphicsImage,
    _terminal: PhantomData<&'terminal DisplayTerminal>,
}

impl<'terminal> KittyImage<'terminal> {
    /// Read every metadata field in one call.
    pub fn info(&self) -> Result<ImageInfo> {
        let mut id = 0u32;
        let mut number = 0u32;
        let mut width = 0u32;
        let mut height = 0u32;
        let mut format = sys::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_RGBA;
        let mut compression = sys::GhosttyKittyImageCompression_GHOSTTY_KITTY_IMAGE_COMPRESSION_NONE;
        let mut len = 0usize;
        let mut generation = 0u64;
        use sys as s;
        // SAFETY: every destination matches the output type
        // kitty_graphics.h documents for its key, and all of them outlive
        // the call.
        unsafe {
            get_multi(
                "kitty_image_get_multi",
                self.raw,
                sys::ghostty_kitty_graphics_image_get_multi,
                [
                    Slot::new(s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_ID, &mut id),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_NUMBER,
                        &mut number,
                    ),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
                        &mut width,
                    ),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
                        &mut height,
                    ),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_FORMAT,
                        &mut format,
                    ),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_COMPRESSION,
                        &mut compression,
                    ),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_DATA_LEN,
                        &mut len,
                    ),
                    Slot::new(
                        s::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
                        &mut generation,
                    ),
                ],
            )?;
        }
        Ok(ImageInfo {
            id,
            number,
            width,
            height,
            format: ImageFormat::from_raw(format)?,
            compression: ImageCompression::from_raw(compression)?,
            len,
            generation,
        })
    }

    /// The decoded pixels, ready to upload without any decode step.
    ///
    /// `None` while the image's metadata is stored but its payload is still
    /// arriving. The slice lives until the terminal is next written to,
    /// which the borrow enforces; its size is bounded by the storage budget
    /// passed to [`DisplayTerminal::enable_kitty_graphics`].
    pub fn pixels(&self) -> Result<Option<&'terminal [u8]>> {
        let mut pointer: *const u8 = std::ptr::null();
        // SAFETY: the image handle is live and `pointer` has the
        // `const uint8_t **` output type the key documents.
        let result = unsafe {
            sys::ghostty_kitty_graphics_image_get(
                self.raw,
                sys::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR,
                (&raw mut pointer).cast::<c_void>(),
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("kitty_image_get_data_ptr", result)?;
        if pointer.is_null() {
            return Ok(None);
        }
        let mut len = 0usize;
        // SAFETY: the image handle is live and `len` has the `size_t *`
        // output type the key documents.
        let result = unsafe {
            sys::ghostty_kitty_graphics_image_get(
                self.raw,
                sys::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_DATA_LEN,
                (&raw mut len).cast::<c_void>(),
            )
        };
        check("kitty_image_get_data_len", result)?;
        // SAFETY: libghostty reports `len` initialized bytes at `pointer`,
        // valid until the terminal is mutated, which `'terminal` outlives.
        Ok(Some(unsafe { std::slice::from_raw_parts(pointer, len) }))
    }
}

/// A cursor over the placements of one storage.
///
/// Not a [`Iterator`]: every read is fallible and the geometry calls need the
/// placement's image, which the caller resolves.
pub struct PlacementCursor<'terminal> {
    iterator: OwnedHandle<sys::GhosttyKittyGraphicsPlacementIterator>,
    terminal: &'terminal DisplayTerminal,
}

impl PlacementCursor<'_> {
    /// Move to the next placement in the configured layer.
    ///
    /// Must be called once before any read. Returns `false` at the end.
    pub fn advance(&mut self) -> bool {
        // SAFETY: the iterator handle is live and owned by `self`.
        unsafe { sys::ghostty_kitty_graphics_placement_next(self.iterator.raw()) }
    }

    /// The image ID of the current placement, on its own.
    ///
    /// This is the cheap half of a texture cache lookup: read the ID, test
    /// the cache against the image's generation, and only fall through to
    /// [`Self::read`] and the geometry calls on a miss.
    pub fn image_id(&self) -> Result<u32> {
        let mut image_id = 0u32;
        // SAFETY: the iterator is live and positioned, and `image_id` has
        // the `uint32_t *` output type the key documents.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_get(
                self.iterator.raw(),
                sys::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
                (&raw mut image_id).cast::<c_void>(),
            )
        };
        check("kitty_placement_get_image_id", result)?;
        Ok(image_id)
    }

    /// Read every protocol field of the current placement in one call.
    pub fn read(&self) -> Result<Placement> {
        let mut image_id = 0u32;
        let mut placement_id = 0u32;
        let mut virtual_placement = false;
        let mut x_offset = 0u32;
        let mut y_offset = 0u32;
        let mut source_x = 0u32;
        let mut source_y = 0u32;
        let mut source_width = 0u32;
        let mut source_height = 0u32;
        let mut columns = 0u32;
        let mut rows = 0u32;
        let mut z = 0i32;
        use sys as s;
        type Key = s::GhosttyKittyGraphicsPlacementData;
        const IMAGE_ID: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID;
        const PLACEMENT_ID: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_PLACEMENT_ID;
        const IS_VIRTUAL: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL;
        const X_OFFSET: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_X_OFFSET;
        const Y_OFFSET: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Y_OFFSET;
        const SOURCE_X: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_X;
        const SOURCE_Y: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_Y;
        const SOURCE_WIDTH: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_WIDTH;
        const SOURCE_HEIGHT: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_HEIGHT;
        const COLUMNS: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_COLUMNS;
        const ROWS: Key =
            s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_ROWS;
        const Z: Key = s::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z;
        // SAFETY: every destination matches the output type
        // kitty_graphics.h documents for its key, and all of them outlive
        // the call.
        unsafe {
            get_multi(
                "kitty_placement_get_multi",
                self.iterator.raw(),
                sys::ghostty_kitty_graphics_placement_get_multi,
                [
                    Slot::new(IMAGE_ID, &mut image_id),
                    Slot::new(PLACEMENT_ID, &mut placement_id),
                    Slot::new(IS_VIRTUAL, &mut virtual_placement),
                    Slot::new(X_OFFSET, &mut x_offset),
                    Slot::new(Y_OFFSET, &mut y_offset),
                    Slot::new(SOURCE_X, &mut source_x),
                    Slot::new(SOURCE_Y, &mut source_y),
                    Slot::new(SOURCE_WIDTH, &mut source_width),
                    Slot::new(SOURCE_HEIGHT, &mut source_height),
                    Slot::new(COLUMNS, &mut columns),
                    Slot::new(ROWS, &mut rows),
                    Slot::new(Z, &mut z),
                ],
            )?;
        }
        Ok(Placement {
            image_id,
            placement_id,
            virtual_placement,
            x_offset,
            y_offset,
            source_x,
            source_y,
            source_width,
            source_height,
            columns,
            rows,
            z,
        })
    }

    /// The grid rectangle the current placement covers.
    ///
    /// `None` for a virtual placement, which has no pin of its own.
    pub fn rect(&self, image: &KittyImage<'_>) -> Result<Option<SelectionRange>> {
        let mut selection = empty_selection();
        // SAFETY: all three handles are live, the iterator is positioned, and
        // `selection` is a correctly sized out-parameter.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_rect(
                self.iterator.raw(),
                image.raw,
                self.terminal.terminal.raw(),
                &mut selection,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("kitty_placement_rect", result)?;
        Ok(Some(self.terminal.selection_range_of(&selection)?))
    }

    /// The size the current placement renders at, in pixels.
    pub fn pixel_size(&self, image: &KittyImage<'_>) -> Result<(u32, u32)> {
        let mut width = 0u32;
        let mut height = 0u32;
        // SAFETY: all three handles are live, the iterator is positioned, and
        // both out-parameters are valid storage.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_pixel_size(
                self.iterator.raw(),
                image.raw,
                self.terminal.terminal.raw(),
                &mut width,
                &mut height,
            )
        };
        check("kitty_placement_pixel_size", result)?;
        Ok((width, height))
    }

    /// The number of grid columns and rows the current placement covers.
    pub fn grid_size(&self, image: &KittyImage<'_>) -> Result<(u32, u32)> {
        let mut cols = 0u32;
        let mut rows = 0u32;
        // SAFETY: all three handles are live, the iterator is positioned, and
        // both out-parameters are valid storage.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_grid_size(
                self.iterator.raw(),
                image.raw,
                self.terminal.terminal.raw(),
                &mut cols,
                &mut rows,
            )
        };
        check("kitty_placement_grid_size", result)?;
        Ok((cols, rows))
    }

    /// The current placement's top-left cell, relative to the viewport.
    ///
    /// `None` when the placement is fully scrolled out or virtual. The row is
    /// negative when the image's top has scrolled above the viewport, so the
    /// renderer clips rather than repositions.
    pub fn viewport_position(&self, image: &KittyImage<'_>) -> Result<Option<(i32, i32)>> {
        let mut col = 0i32;
        let mut row = 0i32;
        // SAFETY: all three handles are live, the iterator is positioned, and
        // both out-parameters are valid storage.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_viewport_pos(
                self.iterator.raw(),
                image.raw,
                self.terminal.terminal.raw(),
                &mut col,
                &mut row,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("kitty_placement_viewport_pos", result)?;
        Ok(Some((col, row)))
    }

    /// The current placement's source rectangle, clamped to the image.
    ///
    /// Unlike [`Placement::source_width`], a zero in the protocol has already
    /// been resolved to the full image dimension.
    pub fn source_rect(&self, image: &KittyImage<'_>) -> Result<SourceRect> {
        let mut x = 0u32;
        let mut y = 0u32;
        let mut width = 0u32;
        let mut height = 0u32;
        // SAFETY: both handles are live, the iterator is positioned, and all
        // four out-parameters are valid storage.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_source_rect(
                self.iterator.raw(),
                image.raw,
                &mut x,
                &mut y,
                &mut width,
                &mut height,
            )
        };
        check("kitty_placement_source_rect", result)?;
        Ok(SourceRect {
            x,
            y,
            width,
            height,
        })
    }

    /// Pixel size, grid size, viewport position, and source rectangle in one
    /// call, which is what a render pass actually wants.
    pub fn render_info(&self, image: &KittyImage<'_>) -> Result<PlacementRenderInfo> {
        let mut info = sys::GhosttyKittyGraphicsPlacementRenderInfo {
            size: std::mem::size_of::<sys::GhosttyKittyGraphicsPlacementRenderInfo>(),
            pixel_width: 0,
            pixel_height: 0,
            grid_cols: 0,
            grid_rows: 0,
            viewport_col: 0,
            viewport_row: 0,
            viewport_visible: false,
            source_x: 0,
            source_y: 0,
            source_width: 0,
            source_height: 0,
        };
        // SAFETY: all three handles are live, the iterator is positioned, and
        // `info` carries its own size for the sized-struct ABI.
        let result = unsafe {
            sys::ghostty_kitty_graphics_placement_render_info(
                self.iterator.raw(),
                image.raw,
                self.terminal.terminal.raw(),
                &mut info,
            )
        };
        check("kitty_placement_render_info", result)?;
        Ok(PlacementRenderInfo {
            pixel_width: info.pixel_width,
            pixel_height: info.pixel_height,
            grid_cols: info.grid_cols,
            grid_rows: info.grid_rows,
            viewport: info
                .viewport_visible
                .then_some((info.viewport_col, info.viewport_row)),
            source: SourceRect {
                x: info.source_x,
                y: info.source_y,
                width: info.source_width,
                height: info.source_height,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, Scroll, TerminalAppearance, WindowSize};

    /// An inline RGB image, transmitted and placed at the cursor.
    ///
    /// `f=24` is three bytes per pixel, so one pixel is exactly one base64
    /// group and a solid image is one group repeated. `s`/`v` are the pixel
    /// dimensions, `a=T` transmits and places in one command, and `q=2`
    /// suppresses the protocol's acknowledgement.
    fn image_command(width: u32, height: u32, pixel: &str, extra: &str) -> Vec<u8> {
        let payload = pixel.repeat((width * height) as usize);
        format!("\x1b_Ga=T,f=24,s={width},v={height},q=2{extra};{payload}\x1b\\").into_bytes()
    }

    /// Solid red, `RED` per pixel, sized to cover two 8x16 cells each way.
    fn red_image() -> Vec<u8> {
        image_command(16, 32, RED, "")
    }

    /// One red pixel: 0xff, 0x00, 0x00.
    const RED: &str = "/wAA";
    /// One blue pixel: 0x00, 0x00, 0xff.
    const BLUE: &str = "AAD/";

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize");
        terminal
            .enable_kitty_graphics(4 * 1024 * 1024, 1024 * 1024)
            .expect("kitty graphics must enable");
        terminal
    }

    fn only_placement<'a>(
        graphics: &KittyGraphics<'a>,
    ) -> (PlacementCursor<'a>, Placement, ImageInfo, u32) {
        let mut cursor = graphics
            .placements(PlacementLayer::All)
            .expect("iterator must populate");
        assert!(cursor.advance(), "the fixture must store one placement");
        let placement = cursor.read().expect("placement fields");
        let image_id = cursor.image_id().expect("single-key image id");
        let info = graphics
            .image(placement.image_id)
            .expect("the placement's image must exist")
            .info()
            .expect("image metadata");
        (cursor, placement, info, image_id)
    }

    #[test]
    fn graphics_are_off_until_the_embedder_opts_in() {
        let size = WindowSize::new(20, 5, 8, 16).expect("valid terminal size");
        let mut plain = DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize");
        plain.feed(&red_image()).expect("image command must parse");
        // The storage object always exists; what the constructor withholds is
        // the budget that lets an image reach it. libghostty still stamps a
        // generation for the transmission it refused, so emptiness, not the
        // stamp, is what says the protocol is off.
        let storage = plain
            .kitty_graphics()
            .expect("storage query")
            .expect("the storage object exists even when disabled");
        assert!(
            !storage
                .placements(PlacementLayer::All)
                .expect("iterator must populate")
                .advance(),
            "the constructor must leave the protocol disabled"
        );

        let mut enabled = terminal(20, 5);
        enabled.feed(&red_image()).expect("image command must parse");
        let graphics = enabled
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist once enabled");
        assert!(graphics.generation().expect("generation") > 0);
    }

    #[test]
    fn a_transmitted_image_is_stored_decoded_and_placed() {
        let mut terminal = terminal(20, 5);
        terminal.feed(&red_image()).expect("image command must parse");
        let graphics = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist");
        let (_cursor, placement, info, image_id) = only_placement(&graphics);

        assert_eq!(image_id, placement.image_id);
        assert!(!placement.virtual_placement);
        assert_eq!(placement.z, 0);
        assert_eq!((info.width, info.height), (16, 32));
        assert_eq!(info.format, ImageFormat::Rgb);
        assert_eq!(info.compression, ImageCompression::None);
        assert_eq!(info.format.bytes_per_pixel(), Some(3));
        assert_eq!(info.len, 16 * 32 * 3);
        assert!(info.generation > 0);
        assert_eq!(info.id, placement.image_id);

        let image = graphics.image(placement.image_id).expect("image handle");
        let pixels = image.pixels().expect("pixel query").expect("stored pixels");
        assert_eq!(pixels, &[0xff, 0x00, 0x00].repeat(16 * 32));
        assert!(graphics.image(placement.image_id + 1000).is_none());
    }

    #[test]
    fn geometry_helpers_agree_with_the_batched_render_info() {
        let mut terminal = terminal(20, 5);
        terminal.feed(&red_image()).expect("image command must parse");
        let graphics = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist");
        let (cursor, placement, _info, _) = only_placement(&graphics);
        let image = graphics.image(placement.image_id).expect("image handle");

        let pixel_size = cursor.pixel_size(&image).expect("pixel size");
        let grid_size = cursor.grid_size(&image).expect("grid size");
        let viewport = cursor.viewport_position(&image).expect("viewport position");
        let source = cursor.source_rect(&image).expect("source rect");
        let info = cursor.render_info(&image).expect("render info");

        assert_eq!(pixel_size, (info.pixel_width, info.pixel_height));
        assert_eq!(grid_size, (info.grid_cols, info.grid_rows));
        assert_eq!(viewport, info.viewport);
        assert_eq!(source, info.source);

        // 16x32 pixels is exactly two 8x16 cells each way.
        assert_eq!(pixel_size, (16, 32));
        assert_eq!(grid_size, (2, 2));
        assert_eq!(viewport, Some((0, 0)));
        // The protocol's zero source size resolves to the full image.
        assert_eq!((placement.source_width, placement.source_height), (0, 0));
        assert_eq!(
            source,
            SourceRect {
                x: 0,
                y: 0,
                width: 16,
                height: 32,
            }
        );

        let rect = cursor
            .rect(&image)
            .expect("bounding rect")
            .expect("a non-virtual placement has a rect");
        assert!(rect.rectangle);
        assert_eq!(rect.start, Point::new(0, 0));
    }

    #[test]
    fn the_layer_filter_selects_by_z_index() {
        let mut terminal = terminal(20, 5);
        // Two placements of the same image, one under the text and one over.
        terminal
            .feed(&image_command(16, 32, RED, ",i=7,p=1,z=5"))
            .expect("above-text placement must parse");
        terminal
            .feed(b"\x1b_Ga=p,i=7,p=2,z=-5,q=2\x1b\\")
            .expect("below-text placement must parse");
        let graphics = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist");

        let count = |layer| {
            let mut cursor = graphics.placements(layer).expect("iterator must populate");
            let mut seen = Vec::new();
            while cursor.advance() {
                seen.push(cursor.read().expect("placement fields").z);
            }
            seen
        };

        let mut all = count(PlacementLayer::All);
        all.sort_unstable();
        assert_eq!(all, [-5, 5]);
        assert_eq!(count(PlacementLayer::AboveText), [5]);
        assert_eq!(count(PlacementLayer::BelowText), [-5]);
        assert!(count(PlacementLayer::BelowBackground).is_empty());
    }

    #[test]
    fn the_storage_generation_only_moves_on_content_changes() {
        let mut terminal = terminal(20, 5);
        terminal.feed(&red_image()).expect("image command must parse");
        let first = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist")
            .generation()
            .expect("generation");

        // Scrolling moves the placement without changing what is stored.
        terminal.feed(b"\r\n\r\n\r\n\r\n\r\n\r\n").expect("newlines");
        terminal.scroll(Scroll::Top);
        let after_scroll = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist")
            .generation()
            .expect("generation");
        assert_eq!(after_scroll, first);

        // A retransmission of the same ID does change it, which is the case
        // size heuristics cannot see.
        terminal
            .feed(&image_command(16, 32, BLUE, ""))
            .expect("second image must parse");
        let after_transmit = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist")
            .generation()
            .expect("generation");
        assert!(after_transmit > first);
    }

    #[test]
    fn a_scrolled_out_placement_reports_no_viewport_position() {
        let mut terminal = terminal(20, 3);
        terminal.feed(&red_image()).expect("image command must parse");
        for _ in 0..40 {
            terminal.feed(b"\r\nfiller").expect("filler must parse");
        }

        let graphics = terminal
            .kitty_graphics()
            .expect("storage query")
            .expect("storage must exist");
        let (cursor, placement, _info, _) = only_placement(&graphics);
        let image = graphics.image(placement.image_id).expect("image handle");

        assert_eq!(cursor.viewport_position(&image).expect("viewport"), None);
        assert_eq!(
            cursor.render_info(&image).expect("render info").viewport,
            None
        );
        // The placement is still stored, and still has a grid rectangle.
        assert!(cursor.rect(&image).expect("rect").is_some());
    }

    #[test]
    fn disabling_the_protocol_drops_the_storage() {
        let mut terminal = terminal(20, 5);
        terminal.feed(&red_image()).expect("image command must parse");
        assert!(terminal.kitty_graphics().expect("storage query").is_some());

        terminal
            .disable_kitty_graphics()
            .expect("protocol must disable");
        terminal.feed(&red_image()).expect("image command must parse");
        let graphics = terminal.kitty_graphics().expect("storage query");
        let placements = match graphics {
            None => 0,
            Some(graphics) => {
                let mut cursor = graphics
                    .placements(PlacementLayer::All)
                    .expect("iterator must populate");
                let mut seen = 0;
                while cursor.advance() {
                    seen += 1;
                }
                seen
            }
        };
        assert_eq!(placements, 0);
    }
}
