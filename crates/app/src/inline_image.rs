use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::ImageFormat;
use knightty_core::{
    GridPoint, ImageId, ImagePixelOffset, ImagePlacement, ImagePlacementId, ImageSourceRect,
    ImageZIndex,
};
use knightty_proto::iterm2::{ImageDimension, InlineImageSequence};
use knightty_proto::kitty::KittyFormat;
use knightty_render::CellMetrics;
use thiserror::Error;

use crate::config::GraphicsConfig;

const OSC: u8 = 0x9d;
const ST: u8 = 0x9c;
const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const IT2_PREFIX: &[u8] = b"1337;File=";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageLimits {
    pub max_encoded_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_images: usize,
    pub max_gpu_bytes: usize,
}

impl From<&GraphicsConfig> for ImageLimits {
    fn from(config: &GraphicsConfig) -> Self {
        Self {
            max_encoded_bytes: config.max_encoded_bytes,
            max_decoded_bytes: config.max_decoded_bytes,
            max_width: config.max_width,
            max_height: config.max_height,
            max_pixels: config.max_pixels,
            max_images: config.max_images,
            max_gpu_bytes: config.max_gpu_bytes,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KittyPlacementOptions {
    pub columns: Option<u16>,
    pub rows: Option<u16>,
    pub source_x: Option<u32>,
    pub source_y: Option<u32>,
    pub source_width: Option<u32>,
    pub source_height: Option<u32>,
    pub pixel_offset_x: Option<u16>,
    pub pixel_offset_y: Option<u16>,
    pub z_index: i32,
    pub kitty_image_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PtyStreamItem<'a> {
    Text(&'a [u8]),
    InlineImage(&'a str),
    InvalidInlineImage,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PtyStreamScan<'a> {
    pub items: Vec<PtyStreamItem<'a>>,
    pub consumed: usize,
}

/// Split complete iTerm2 image OSC sequences from ordinary PTY bytes.
pub fn scan_pty_stream(input: &[u8]) -> PtyStreamScan<'_> {
    let mut items = Vec::new();
    let mut cursor = 0;

    while let Some((osc_start, payload_start)) = find_osc_start(input, cursor) {
        if osc_start > cursor {
            items.push(PtyStreamItem::Text(&input[cursor..osc_start]));
        }

        let Some((payload_end, sequence_end)) = find_osc_end(input, payload_start) else {
            return PtyStreamScan {
                items,
                consumed: osc_start,
            };
        };
        let payload = &input[payload_start..payload_end];
        if let Some(image) = payload.strip_prefix(IT2_PREFIX) {
            match std::str::from_utf8(image) {
                Ok(image) => items.push(PtyStreamItem::InlineImage(image)),
                Err(_) => items.push(PtyStreamItem::InvalidInlineImage),
            }
        } else {
            items.push(PtyStreamItem::Text(&input[osc_start..sequence_end]));
        }
        cursor = sequence_end;
    }

    if cursor < input.len() {
        items.push(PtyStreamItem::Text(&input[cursor..]));
    }
    PtyStreamScan {
        items,
        consumed: input.len(),
    }
}

pub fn pending_inline_image_is_oversized(pending: &[u8], limits: ImageLimits) -> bool {
    let payload_start = if pending.starts_with(b"\x1b]") {
        2
    } else if pending.first() == Some(&OSC) {
        1
    } else {
        return false;
    };
    pending[payload_start..].starts_with(IT2_PREFIX)
        && pending.len()
            > limits
                .max_encoded_bytes
                .saturating_add(knightty_proto::iterm2::MAX_METADATA_BYTES)
                .saturating_add(64)
}

pub fn decode_png(
    sequence: &InlineImageSequence<'_>,
    limits: ImageLimits,
) -> Result<DecodedImage, InlineImageError> {
    decode_png_payload(sequence.payload.as_bytes(), limits)
}

pub fn decode_png_payload(
    payload: &[u8],
    limits: ImageLimits,
) -> Result<DecodedImage, InlineImageError> {
    let compressed = decode_base64_payload(payload, limits)?;
    if compressed.len() > limits.max_encoded_bytes {
        return Err(InlineImageError::CompressedTooLarge);
    }
    decode_png_bytes(&compressed, limits)
}

pub fn decode_kitty_image(
    format: KittyFormat,
    payload: &[u8],
    limits: ImageLimits,
) -> Result<DecodedImage, InlineImageError> {
    match format {
        KittyFormat::Png => decode_png_payload(payload, limits),
        KittyFormat::Rgb24 { width, height } => {
            let raw = decode_base64_payload(payload, limits)?;
            decode_raw_image(raw, width, height, 3, limits)
        }
        KittyFormat::Rgba32 { width, height } => {
            let raw = decode_base64_payload(payload, limits)?;
            decode_raw_image(raw, width, height, 4, limits)
        }
    }
}

fn decode_base64_payload(payload: &[u8], limits: ImageLimits) -> Result<Vec<u8>, InlineImageError> {
    if payload.len() > limits.max_encoded_bytes {
        return Err(InlineImageError::EncodedTooLarge);
    }
    STANDARD
        .decode(payload)
        .map_err(|_| InlineImageError::InvalidBase64)
}

fn decode_png_bytes(
    compressed: &[u8],
    limits: ImageLimits,
) -> Result<DecodedImage, InlineImageError> {
    let rgba = image::load_from_memory_with_format(compressed, ImageFormat::Png)
        .map_err(|_| InlineImageError::InvalidPng)?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(InlineImageError::ZeroDimension);
    }
    if width > limits.max_width || height > limits.max_height {
        return Err(InlineImageError::DimensionLimit);
    }

    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(InlineImageError::SizeOverflow)?;
    if pixels > limits.max_pixels {
        return Err(InlineImageError::PixelLimit);
    }
    let decoded_bytes = pixels
        .checked_mul(4)
        .ok_or(InlineImageError::SizeOverflow)?;
    if decoded_bytes > limits.max_decoded_bytes as u64 {
        return Err(InlineImageError::DecodedTooLarge);
    }

    let raw = rgba.into_raw();
    if raw.len() as u64 != decoded_bytes {
        return Err(InlineImageError::SizeOverflow);
    }
    Ok(DecodedImage {
        width,
        height,
        rgba: Arc::from(raw),
    })
}

fn decode_raw_image(
    raw: Vec<u8>,
    width: u32,
    height: u32,
    channels: u64,
    limits: ImageLimits,
) -> Result<DecodedImage, InlineImageError> {
    if width == 0 || height == 0 {
        return Err(InlineImageError::ZeroDimension);
    }
    if width > limits.max_width || height > limits.max_height {
        return Err(InlineImageError::DimensionLimit);
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(InlineImageError::SizeOverflow)?;
    if pixels > limits.max_pixels {
        return Err(InlineImageError::PixelLimit);
    }
    let decoded_bytes = pixels
        .checked_mul(4)
        .ok_or(InlineImageError::SizeOverflow)?;
    if decoded_bytes > limits.max_decoded_bytes as u64 {
        return Err(InlineImageError::DecodedTooLarge);
    }
    let expected_payload_bytes = pixels
        .checked_mul(channels)
        .ok_or(InlineImageError::SizeOverflow)?;
    if raw.len() as u64 != expected_payload_bytes {
        return Err(InlineImageError::InvalidPayloadLength);
    }

    let rgba = if channels == 4 {
        raw
    } else {
        let capacity =
            usize::try_from(decoded_bytes).map_err(|_| InlineImageError::SizeOverflow)?;
        let mut rgba = Vec::with_capacity(capacity);
        for pixel in raw.chunks_exact(3) {
            rgba.extend_from_slice(pixel);
            rgba.push(255);
        }
        rgba
    };
    if rgba.len() as u64 != decoded_bytes {
        return Err(InlineImageError::SizeOverflow);
    }
    Ok(DecodedImage {
        width,
        height,
        rgba: Arc::from(rgba),
    })
}

pub fn placement_for_kitty(
    id: ImageId,
    placement_id: ImagePlacementId,
    image: &DecodedImage,
    options: KittyPlacementOptions,
    anchor: GridPoint,
    metrics: CellMetrics,
) -> Result<ImagePlacement, InlineImageError> {
    if metrics.width == 0 || metrics.height == 0 {
        return Err(InlineImageError::ZeroCellSize);
    }
    let source_rect = resolve_kitty_source_rect(image, options)?;
    let pixel_offset = ImagePixelOffset {
        x: options.pixel_offset_x.unwrap_or(0),
        y: options.pixel_offset_y.unwrap_or(0),
    };
    if u32::from(pixel_offset.x) >= metrics.width || u32::from(pixel_offset.y) >= metrics.height {
        return Err(InlineImageError::InvalidPixelOffset);
    }

    let (columns, rows) = match (options.columns, options.rows) {
        (None, None) => (
            ceil_ratio(u64::from(source_rect.width), u64::from(metrics.width)),
            ceil_ratio(u64::from(source_rect.height), u64::from(metrics.height)),
        ),
        (Some(columns), None) => (
            columns,
            rows_for_dimensions(source_rect.width, source_rect.height, columns, metrics)?,
        ),
        (None, Some(rows)) => (
            columns_for_dimensions(source_rect.width, source_rect.height, rows, metrics)?,
            rows,
        ),
        (Some(columns), Some(rows)) => (columns, rows),
    };

    Ok(ImagePlacement {
        placement_id,
        image_id: id,
        kitty_image_id: options.kitty_image_id,
        anchor,
        columns: columns.max(1),
        rows: rows.max(1),
        source_width: image.width,
        source_height: image.height,
        source_rect,
        pixel_offset,
        z_index: ImageZIndex(options.z_index),
    })
}

fn resolve_kitty_source_rect(
    image: &DecodedImage,
    options: KittyPlacementOptions,
) -> Result<ImageSourceRect, InlineImageError> {
    let x = options.source_x.unwrap_or(0);
    let y = options.source_y.unwrap_or(0);
    let requested_right = match options.source_width {
        None | Some(0) => image.width,
        Some(width) => x
            .checked_add(width)
            .ok_or(InlineImageError::InvalidSourceRectangle)?,
    };
    let requested_bottom = match options.source_height {
        None | Some(0) => image.height,
        Some(height) => y
            .checked_add(height)
            .ok_or(InlineImageError::InvalidSourceRectangle)?,
    };
    let right = requested_right.min(image.width);
    let bottom = requested_bottom.min(image.height);
    let width = right
        .checked_sub(x)
        .filter(|width| *width > 0)
        .ok_or(InlineImageError::InvalidSourceRectangle)?;
    let height = bottom
        .checked_sub(y)
        .filter(|height| *height > 0)
        .ok_or(InlineImageError::InvalidSourceRectangle)?;

    Ok(ImageSourceRect {
        x,
        y,
        width,
        height,
    })
}

pub fn placement_for_image(
    id: ImageId,
    placement_id: ImagePlacementId,
    image: &DecodedImage,
    sequence: &InlineImageSequence<'_>,
    anchor: GridPoint,
    terminal_columns: usize,
    metrics: CellMetrics,
) -> Result<ImagePlacement, InlineImageError> {
    if metrics.width == 0 || metrics.height == 0 || terminal_columns == 0 {
        return Err(InlineImageError::ZeroCellSize);
    }

    let available_columns = terminal_columns
        .saturating_sub(anchor.col)
        .max(1)
        .min(u16::MAX as usize) as u16;
    let width = explicit_cells(sequence.width);
    let height = explicit_cells(sequence.height);
    let (columns, rows) = match (width, height) {
        (None, None) => {
            let columns = ceil_ratio(u64::from(image.width), u64::from(metrics.width));
            let rows = ceil_ratio(u64::from(image.height), u64::from(metrics.height));
            clamp_width_preserving_ratio(columns, rows, available_columns)
        }
        (Some(columns), None) => {
            let columns = columns.min(available_columns).max(1);
            let rows = rows_for_columns(image, columns, metrics)?;
            (columns, rows)
        }
        (None, Some(rows)) => {
            let rows = rows.max(1);
            let columns = columns_for_rows(image, rows, metrics)?;
            if columns > available_columns {
                (
                    available_columns,
                    rows_for_columns(image, available_columns, metrics)?,
                )
            } else {
                (columns, rows)
            }
        }
        (Some(columns), Some(rows)) => fit_inside_cell_box(
            image,
            columns.min(available_columns).max(1),
            rows.max(1),
            metrics,
        )?,
    };

    Ok(ImagePlacement {
        placement_id,
        image_id: id,
        kitty_image_id: None,
        anchor,
        columns: columns.max(1),
        rows: rows.max(1),
        source_width: image.width,
        source_height: image.height,
        source_rect: ImageSourceRect {
            x: 0,
            y: 0,
            width: image.width,
            height: image.height,
        },
        pixel_offset: ImagePixelOffset::default(),
        z_index: ImageZIndex::default(),
    })
}

fn find_osc_start(input: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut cursor = from;
    while cursor < input.len() {
        if input[cursor] == OSC {
            return Some((cursor, cursor + 1));
        }
        if input[cursor] == ESC && input.get(cursor + 1) == Some(&b']') {
            return Some((cursor, cursor + 2));
        }
        cursor += 1;
    }
    None
}

fn find_osc_end(input: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut cursor = from;
    while cursor < input.len() {
        match input[cursor] {
            BEL | ST => return Some((cursor, cursor + 1)),
            ESC if input.get(cursor + 1) == Some(&b'\\') => {
                return Some((cursor, cursor + 2));
            }
            _ => cursor += 1,
        }
    }
    None
}

fn explicit_cells(dimension: Option<ImageDimension>) -> Option<u16> {
    match dimension {
        Some(ImageDimension::Cells(cells)) => Some(cells),
        Some(ImageDimension::Auto) | None => None,
    }
}

fn clamp_width_preserving_ratio(columns: u16, rows: u16, maximum: u16) -> (u16, u16) {
    let columns = columns.max(1);
    let rows = rows.max(1);
    if columns <= maximum {
        return (columns, rows);
    }

    let scaled_rows = ceil_ratio(u64::from(rows) * u64::from(maximum), u64::from(columns));
    (maximum.max(1), scaled_rows)
}

fn rows_for_columns(
    image: &DecodedImage,
    columns: u16,
    metrics: CellMetrics,
) -> Result<u16, InlineImageError> {
    rows_for_dimensions(image.width, image.height, columns, metrics)
}

fn rows_for_dimensions(
    image_width: u32,
    image_height: u32,
    columns: u16,
    metrics: CellMetrics,
) -> Result<u16, InlineImageError> {
    let numerator = u64::from(columns)
        .checked_mul(u64::from(metrics.width))
        .and_then(|value| value.checked_mul(u64::from(image_height)))
        .ok_or(InlineImageError::SizeOverflow)?;
    let denominator = u64::from(image_width)
        .checked_mul(u64::from(metrics.height))
        .ok_or(InlineImageError::SizeOverflow)?;
    Ok(ceil_ratio(numerator, denominator))
}

fn columns_for_rows(
    image: &DecodedImage,
    rows: u16,
    metrics: CellMetrics,
) -> Result<u16, InlineImageError> {
    columns_for_dimensions(image.width, image.height, rows, metrics)
}

fn columns_for_dimensions(
    image_width: u32,
    image_height: u32,
    rows: u16,
    metrics: CellMetrics,
) -> Result<u16, InlineImageError> {
    let numerator = u64::from(rows)
        .checked_mul(u64::from(metrics.height))
        .and_then(|value| value.checked_mul(u64::from(image_width)))
        .ok_or(InlineImageError::SizeOverflow)?;
    let denominator = u64::from(image_height)
        .checked_mul(u64::from(metrics.width))
        .ok_or(InlineImageError::SizeOverflow)?;
    Ok(ceil_ratio(numerator, denominator))
}

fn fit_inside_cell_box(
    image: &DecodedImage,
    box_columns: u16,
    box_rows: u16,
    metrics: CellMetrics,
) -> Result<(u16, u16), InlineImageError> {
    let width_limited_rows = rows_for_columns(image, box_columns, metrics)?;
    if width_limited_rows <= box_rows {
        Ok((box_columns, width_limited_rows))
    } else {
        Ok((
            columns_for_rows(image, box_rows, metrics)?.min(box_columns),
            box_rows,
        ))
    }
}

fn ceil_ratio(numerator: u64, denominator: u64) -> u16 {
    if denominator == 0 {
        return 1;
    }
    numerator
        .saturating_add(denominator - 1)
        .checked_div(denominator)
        .unwrap_or(1)
        .clamp(1, u64::from(u16::MAX)) as u16
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum InlineImageError {
    #[error("encoded image payload exceeds the configured limit")]
    EncodedTooLarge,
    #[error("decoded compressed image exceeds the configured limit")]
    CompressedTooLarge,
    #[error("image payload is not valid base64")]
    InvalidBase64,
    #[error("image payload is not a valid PNG")]
    InvalidPng,
    #[error("raw image payload length does not match its dimensions")]
    InvalidPayloadLength,
    #[error("image has a zero dimension")]
    ZeroDimension,
    #[error("image dimensions exceed the configured limit")]
    DimensionLimit,
    #[error("image pixel count exceeds the configured limit")]
    PixelLimit,
    #[error("decoded RGBA image exceeds the configured limit")]
    DecodedTooLarge,
    #[error("image size calculation overflowed")]
    SizeOverflow,
    #[error("terminal cell size is zero")]
    ZeroCellSize,
    #[error("image source rectangle does not intersect the image")]
    InvalidSourceRectangle,
    #[error("image pixel offset is outside the anchor cell")]
    InvalidPixelOffset,
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use knightty_core::{GridPoint, ImageId, ImagePlacementId};
    use knightty_proto::iterm2::parse_iterm2_inline_image;
    use knightty_proto::kitty::KittyFormat;
    use knightty_render::CellMetrics;

    use super::{
        ImageLimits, InlineImageError, KittyPlacementOptions, PtyStreamItem, decode_kitty_image,
        decode_png, placement_for_image, placement_for_kitty, scan_pty_stream,
    };

    const TRANSPARENT_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    fn limits() -> ImageLimits {
        ImageLimits {
            max_encoded_bytes: 1024 * 1024,
            max_decoded_bytes: 4 * 1024 * 1024,
            max_width: 8192,
            max_height: 8192,
            max_pixels: 32_000_000,
            max_images: 128,
            max_gpu_bytes: 256 * 1024 * 1024,
        }
    }

    fn sequence(metadata: &str) -> knightty_proto::iterm2::InlineImageSequence<'_> {
        parse_iterm2_inline_image(metadata).expect("valid test sequence")
    }

    #[test]
    fn bel_and_st_inline_images_are_scanned_without_exposing_payload_as_text() {
        let input =
            b"before\x1b]1337;File=inline=1:AAAA\x07middle\x1b]1337;File=inline=1:BBBB\x1b\\after";
        let scan = scan_pty_stream(input);

        assert_eq!(
            scan.items,
            vec![
                PtyStreamItem::Text(b"before"),
                PtyStreamItem::InlineImage("inline=1:AAAA"),
                PtyStreamItem::Text(b"middle"),
                PtyStreamItem::InlineImage("inline=1:BBBB"),
                PtyStreamItem::Text(b"after"),
            ]
        );
        assert_eq!(scan.consumed, input.len());
    }

    #[test]
    fn incomplete_osc_is_retained_for_the_next_feed() {
        let input = b"text\x1b]1337;File=inline=1:AAAA";
        let scan = scan_pty_stream(input);

        assert_eq!(scan.items, vec![PtyStreamItem::Text(b"text")]);
        assert_eq!(scan.consumed, 4);
    }

    #[test]
    fn valid_transparent_png_decodes_to_rgba() {
        let command = format!("File=inline=1:{TRANSPARENT_PNG}");
        let sequence = sequence(&command);

        let image = decode_png(&sequence, limits()).expect("PNG decodes");

        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.rgba.len(), 4);
    }

    #[test]
    fn invalid_base64_and_invalid_png_are_rejected() {
        let invalid_base64 = sequence("File=inline=1:***");
        assert_eq!(
            decode_png(&invalid_base64, limits()).unwrap_err(),
            InlineImageError::InvalidBase64
        );

        let invalid_png = sequence("File=inline=1:bm90IGEgcG5n");
        assert_eq!(
            decode_png(&invalid_png, limits()).unwrap_err(),
            InlineImageError::InvalidPng
        );
    }

    #[test]
    fn raw_rgb_expands_to_opaque_rgba_and_rgba_preserves_straight_alpha() {
        let rgb = STANDARD.encode([10, 20, 30]);
        let image = decode_kitty_image(
            KittyFormat::Rgb24 {
                width: 1,
                height: 1,
            },
            rgb.as_bytes(),
            limits(),
        )
        .unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(&*image.rgba, &[10, 20, 30, 255]);

        let rgba_bytes = [
            1, 2, 3, 0, // transparent
            4, 5, 6, 128, // translucent
            7, 8, 9, 255, // opaque
        ];
        let rgba = STANDARD.encode(rgba_bytes);
        let image = decode_kitty_image(
            KittyFormat::Rgba32 {
                width: 3,
                height: 1,
            },
            rgba.as_bytes(),
            limits(),
        )
        .unwrap();
        assert_eq!(&*image.rgba, &rgba_bytes);
    }

    #[test]
    fn raw_payload_requires_valid_base64_and_exact_decoded_length() {
        assert_eq!(
            decode_kitty_image(
                KittyFormat::Rgb24 {
                    width: 1,
                    height: 1,
                },
                b"***",
                limits(),
            )
            .unwrap_err(),
            InlineImageError::InvalidBase64
        );

        for raw in [[1, 2].as_slice(), [1, 2, 3, 4].as_slice()] {
            let payload = STANDARD.encode(raw);
            assert_eq!(
                decode_kitty_image(
                    KittyFormat::Rgb24 {
                        width: 1,
                        height: 1,
                    },
                    payload.as_bytes(),
                    limits(),
                )
                .unwrap_err(),
                InlineImageError::InvalidPayloadLength
            );
        }
    }

    #[test]
    fn raw_dimensions_pixels_decoded_bytes_and_overflow_are_bounded() {
        let rgba = STANDARD.encode([1, 2, 3, 4]);

        let mut restricted = limits();
        restricted.max_width = 0;
        assert_eq!(
            decode_kitty_image(
                KittyFormat::Rgba32 {
                    width: 1,
                    height: 1,
                },
                rgba.as_bytes(),
                restricted,
            )
            .unwrap_err(),
            InlineImageError::DimensionLimit
        );

        let mut restricted = limits();
        restricted.max_pixels = 0;
        assert_eq!(
            decode_kitty_image(
                KittyFormat::Rgba32 {
                    width: 1,
                    height: 1,
                },
                rgba.as_bytes(),
                restricted,
            )
            .unwrap_err(),
            InlineImageError::PixelLimit
        );

        let mut restricted = limits();
        restricted.max_decoded_bytes = 3;
        assert_eq!(
            decode_kitty_image(
                KittyFormat::Rgba32 {
                    width: 1,
                    height: 1,
                },
                rgba.as_bytes(),
                restricted,
            )
            .unwrap_err(),
            InlineImageError::DecodedTooLarge
        );

        let unbounded = ImageLimits {
            max_encoded_bytes: usize::MAX,
            max_decoded_bytes: usize::MAX,
            max_width: u32::MAX,
            max_height: u32::MAX,
            max_pixels: u64::MAX,
            max_images: usize::MAX,
            max_gpu_bytes: usize::MAX,
        };
        assert_eq!(
            decode_kitty_image(
                KittyFormat::Rgba32 {
                    width: u32::MAX,
                    height: u32::MAX,
                },
                b"",
                unbounded,
            )
            .unwrap_err(),
            InlineImageError::SizeOverflow
        );
    }

    #[test]
    fn truncated_png_is_rejected() {
        let mut compressed = STANDARD.decode(TRANSPARENT_PNG).unwrap();
        compressed.truncate(compressed.len() / 2);
        let command = format!("File=inline=1:{}", STANDARD.encode(compressed));

        assert_eq!(
            decode_png(&sequence(&command), limits()).unwrap_err(),
            InlineImageError::InvalidPng
        );
    }

    #[test]
    fn encoded_and_decoded_limits_are_enforced() {
        let command = format!("File=inline=1:{TRANSPARENT_PNG}");
        let sequence = sequence(&command);
        let mut restricted = limits();
        restricted.max_encoded_bytes = 8;
        assert_eq!(
            decode_png(&sequence, restricted).unwrap_err(),
            InlineImageError::EncodedTooLarge
        );

        let mut restricted = limits();
        restricted.max_decoded_bytes = 3;
        assert_eq!(
            decode_png(&sequence, restricted).unwrap_err(),
            InlineImageError::DecodedTooLarge
        );
    }

    #[test]
    fn dimension_and_pixel_limits_are_enforced() {
        let command = format!("File=inline=1:{TRANSPARENT_PNG}");
        let sequence = sequence(&command);
        let mut restricted = limits();
        restricted.max_width = 0;
        assert_eq!(
            decode_png(&sequence, restricted).unwrap_err(),
            InlineImageError::DimensionLimit
        );

        let mut restricted = limits();
        restricted.max_pixels = 0;
        assert_eq!(
            decode_png(&sequence, restricted).unwrap_err(),
            InlineImageError::PixelLimit
        );
    }

    #[test]
    fn natural_size_uses_non_square_cell_metrics() {
        let image = decode_png(
            &sequence(&format!("File=inline=1:{TRANSPARENT_PNG}")),
            limits(),
        )
        .unwrap();
        let placement = placement_for_image(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            &sequence(&format!("File=inline=1:{TRANSPARENT_PNG}")),
            GridPoint { col: 0, row: 0 },
            80,
            CellMetrics {
                width: 8,
                height: 16,
                font_size: 16.0,
                line_height: 16.0,
            },
        )
        .unwrap();

        assert_eq!((placement.columns, placement.rows), (1, 1));
    }

    #[test]
    fn width_only_derives_height_and_clamps_to_terminal_width() {
        let image = super::DecodedImage {
            width: 400,
            height: 200,
            rgba: std::sync::Arc::from(vec![0; 400 * 200 * 4]),
        };
        let dimensions = sequence("File=inline=1;width=10:AAAA");

        let placement = placement_for_image(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            &dimensions,
            GridPoint { col: 3, row: 0 },
            8,
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        )
        .unwrap();

        assert_eq!(placement.columns, 5);
        assert_eq!(placement.rows, 2);
    }

    #[test]
    fn height_only_derives_width() {
        let image = super::DecodedImage {
            width: 400,
            height: 200,
            rgba: std::sync::Arc::from(vec![0; 400 * 200 * 4]),
        };
        let dimensions = sequence("File=inline=1;height=4:AAAA");

        let placement = placement_for_image(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            &dimensions,
            GridPoint { col: 0, row: 0 },
            80,
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        )
        .unwrap();

        assert_eq!((placement.columns, placement.rows), (16, 4));
    }

    #[test]
    fn explicit_width_and_height_fit_inside_the_requested_cell_box() {
        let image = super::DecodedImage {
            width: 400,
            height: 200,
            rgba: std::sync::Arc::from(vec![0; 400 * 200 * 4]),
        };
        let dimensions = sequence("File=inline=1;width=10;height=2:AAAA");

        let placement = placement_for_image(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            &dimensions,
            GridPoint { col: 0, row: 0 },
            80,
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        )
        .unwrap();

        assert_eq!((placement.columns, placement.rows), (8, 2));
    }

    #[test]
    fn kitty_explicit_columns_and_rows_use_the_exact_requested_rectangle() {
        let image = super::DecodedImage {
            width: 400,
            height: 200,
            rgba: std::sync::Arc::from(vec![0; 400 * 200 * 4]),
        };

        let placement = placement_for_kitty(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            KittyPlacementOptions {
                columns: Some(10),
                rows: Some(2),
                ..KittyPlacementOptions::default()
            },
            GridPoint { col: 0, row: 0 },
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        )
        .expect("Kitty placement is valid");

        assert_eq!((placement.columns, placement.rows), (10, 2));
    }

    #[test]
    fn kitty_single_dimension_preserves_pixel_aspect_ratio() {
        let image = super::DecodedImage {
            width: 400,
            height: 200,
            rgba: std::sync::Arc::from(vec![0; 400 * 200 * 4]),
        };

        let placement = placement_for_kitty(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            KittyPlacementOptions {
                columns: Some(10),
                ..KittyPlacementOptions::default()
            },
            GridPoint { col: 0, row: 0 },
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        )
        .expect("Kitty placement is valid");

        assert_eq!((placement.columns, placement.rows), (10, 3));
    }

    #[test]
    fn kitty_source_rectangle_intersects_bounds_and_zero_extends_to_edges() {
        let image = super::DecodedImage {
            width: 100,
            height: 80,
            rgba: std::sync::Arc::from(vec![0; 100 * 80 * 4]),
        };
        let metrics = CellMetrics {
            width: 10,
            height: 10,
            font_size: 10.0,
            line_height: 10.0,
        };

        let clipped = placement_for_kitty(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            KittyPlacementOptions {
                source_x: Some(70),
                source_y: Some(60),
                source_width: Some(50),
                source_height: Some(50),
                ..KittyPlacementOptions::default()
            },
            GridPoint { col: 0, row: 0 },
            metrics,
        )
        .unwrap();
        assert_eq!(
            clipped.source_rect,
            knightty_core::ImageSourceRect {
                x: 70,
                y: 60,
                width: 30,
                height: 20,
            }
        );
        assert_eq!((clipped.columns, clipped.rows), (3, 2));

        let to_edges = placement_for_kitty(
            ImageId::new(1),
            ImagePlacementId::new(2),
            &image,
            KittyPlacementOptions {
                source_x: Some(25),
                source_y: Some(30),
                source_width: Some(0),
                source_height: Some(0),
                ..KittyPlacementOptions::default()
            },
            GridPoint { col: 0, row: 0 },
            metrics,
        )
        .unwrap();
        assert_eq!(
            (to_edges.source_rect.width, to_edges.source_rect.height),
            (75, 50)
        );
    }

    #[test]
    fn kitty_crop_drives_aspect_ratio_and_invalid_intersections_are_rejected() {
        let image = super::DecodedImage {
            width: 100,
            height: 80,
            rgba: std::sync::Arc::from(vec![0; 100 * 80 * 4]),
        };
        let metrics = CellMetrics {
            width: 10,
            height: 10,
            font_size: 10.0,
            line_height: 10.0,
        };
        let cropped = placement_for_kitty(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            KittyPlacementOptions {
                columns: Some(10),
                source_height: Some(20),
                ..KittyPlacementOptions::default()
            },
            GridPoint { col: 0, row: 0 },
            metrics,
        )
        .unwrap();
        assert_eq!((cropped.columns, cropped.rows), (10, 2));

        for options in [
            KittyPlacementOptions {
                source_x: Some(100),
                ..KittyPlacementOptions::default()
            },
            KittyPlacementOptions {
                source_y: Some(80),
                ..KittyPlacementOptions::default()
            },
            KittyPlacementOptions {
                source_x: Some(u32::MAX),
                source_width: Some(2),
                ..KittyPlacementOptions::default()
            },
        ] {
            assert_eq!(
                placement_for_kitty(
                    ImageId::new(1),
                    ImagePlacementId::new(1),
                    &image,
                    options,
                    GridPoint { col: 0, row: 0 },
                    metrics,
                )
                .unwrap_err(),
                InlineImageError::InvalidSourceRectangle
            );
        }
    }

    #[test]
    fn kitty_pixel_offset_validates_only_against_current_cell_size() {
        let image = super::DecodedImage {
            width: 1,
            height: 1,
            rgba: std::sync::Arc::from(vec![0; 4]),
        };
        let metrics = CellMetrics {
            width: 10,
            height: 20,
            font_size: 16.0,
            line_height: 20.0,
        };
        let placement = placement_for_kitty(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            KittyPlacementOptions {
                pixel_offset_x: Some(9),
                pixel_offset_y: Some(19),
                ..KittyPlacementOptions::default()
            },
            GridPoint { col: 0, row: 0 },
            metrics,
        )
        .unwrap();
        assert_eq!(
            (placement.pixel_offset.x, placement.pixel_offset.y),
            (9, 19)
        );

        for options in [
            KittyPlacementOptions {
                pixel_offset_x: Some(10),
                ..KittyPlacementOptions::default()
            },
            KittyPlacementOptions {
                pixel_offset_y: Some(20),
                ..KittyPlacementOptions::default()
            },
        ] {
            assert_eq!(
                placement_for_kitty(
                    ImageId::new(1),
                    ImagePlacementId::new(1),
                    &image,
                    options,
                    GridPoint { col: 0, row: 0 },
                    metrics,
                )
                .unwrap_err(),
                InlineImageError::InvalidPixelOffset
            );
        }
    }

    #[test]
    fn zero_cell_size_is_rejected() {
        let image = super::DecodedImage {
            width: 1,
            height: 1,
            rgba: std::sync::Arc::from(vec![0; 4]),
        };

        let error = placement_for_image(
            ImageId::new(1),
            ImagePlacementId::new(1),
            &image,
            &sequence("File=inline=1:AAAA"),
            GridPoint { col: 0, row: 0 },
            80,
            CellMetrics {
                width: 0,
                height: 16,
                font_size: 16.0,
                line_height: 16.0,
            },
        )
        .unwrap_err();

        assert_eq!(error, InlineImageError::ZeroCellSize);
    }
}
