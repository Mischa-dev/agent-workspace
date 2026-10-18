//! Images people attach: which ones show inline, their size, their location
//! data stripped, and a small preview for the timeline.
//!
//! Only PNG, JPEG, GIF and WebP show inline; any other image (SVG, HEIC) is
//! an ordinary file and downloads. What a file is comes from its bytes, never
//! from the name or the type the browser claimed.

use base64::Engine as _;
use resvg::{tiny_skia, usvg};

/// Previews are at most this many pixels on their longer side.
pub const THUMBNAIL_SIDE: u32 = 800;
/// Images larger than this (in pixels) get no preview; drawing one would
/// take too much memory.
const MAX_THUMBNAIL_SOURCE_PIXELS: u64 = 40_000_000;
/// Small images are their own preview.
const SMALL_IMAGE_BYTES: usize = 300 * 1024;
/// A GIF up to this size is its own preview, so it stays animated.
const ANIMATED_PREVIEW_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    fn svg_media_type(self) -> &'static str {
        self.content_type()
    }
}

/// The inline image format of these bytes, if they are one.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// An image ready to store: its bytes without location data, how it shows
/// (width and height after the camera's rotation), and its preview.
pub struct ProcessedImage {
    pub format: ImageFormat,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// None when the image is its own preview.
    pub thumbnail: Option<Thumbnail>,
}

pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

/// Strips metadata and makes the preview. None when the bytes aren't an
/// image we can read (they are then stored as an ordinary file).
pub fn process(bytes: Vec<u8>) -> Option<ProcessedImage> {
    let prepared = prepare(bytes)?;
    let (width, height) = prepared.shown_size();
    let own_preview = match prepared.format {
        ImageFormat::Gif => prepared.bytes.len() <= ANIMATED_PREVIEW_BYTES,
        _ => {
            prepared.bytes.len() <= SMALL_IMAGE_BYTES
                && width <= THUMBNAIL_SIDE
                && height <= THUMBNAIL_SIDE
                && prepared.orientation == 1
        }
    };
    let thumbnail = if own_preview || prepared.too_big_to_draw() {
        None
    } else {
        prepared.thumbnail(THUMBNAIL_SIDE)
    };
    Some(ProcessedImage {
        format: prepared.format,
        bytes: prepared.bytes,
        width,
        height,
        thumbnail,
    })
}

/// An image at most `side` pixels on its longer side, for a link preview:
/// (bytes, content type, width, height). The original when it is already
/// that small and needs no turning.
pub fn shrink(bytes: Vec<u8>, side: u32) -> Option<(Vec<u8>, &'static str, u32, u32)> {
    let prepared = prepare(bytes)?;
    let (width, height) = prepared.shown_size();
    if width <= side && height <= side && prepared.orientation == 1 {
        let content_type = prepared.format.content_type();
        return Some((prepared.bytes, content_type, width, height));
    }
    if prepared.too_big_to_draw() {
        return None;
    }
    let thumbnail = prepared.thumbnail(side)?;
    let scale = f64::from(side) / f64::from(width.max(height));
    Some((
        thumbnail.bytes,
        thumbnail.content_type,
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    ))
}

/// An image with its metadata stripped, before any preview is drawn.
struct Prepared {
    format: ImageFormat,
    bytes: Vec<u8>,
    raw_width: u32,
    raw_height: u32,
    orientation: u8,
}

impl Prepared {
    fn shown_size(&self) -> (u32, u32) {
        if self.orientation >= 5 {
            (self.raw_height, self.raw_width)
        } else {
            (self.raw_width, self.raw_height)
        }
    }

    fn too_big_to_draw(&self) -> bool {
        u64::from(self.raw_width) * u64::from(self.raw_height) > MAX_THUMBNAIL_SOURCE_PIXELS
    }

    fn thumbnail(&self, side: u32) -> Option<Thumbnail> {
        thumbnail(
            &self.bytes,
            self.format,
            self.raw_width,
            self.raw_height,
            self.orientation,
            side,
        )
    }
}

fn prepare(bytes: Vec<u8>) -> Option<Prepared> {
    let format = sniff(&bytes)?;
    let size = imagesize::blob_size(&bytes).ok()?;
    let (raw_width, raw_height) = (
        u32::try_from(size.width).ok()?,
        u32::try_from(size.height).ok()?,
    );
    if raw_width == 0 || raw_height == 0 {
        return None;
    }
    let (bytes, orientation) = match format {
        ImageFormat::Jpeg => strip_jpeg(&bytes)?,
        ImageFormat::Png => (strip_png(&bytes)?, 1),
        ImageFormat::Webp => (strip_webp(&bytes)?, 1),
        ImageFormat::Gif => (bytes, 1),
    };
    Some(Prepared {
        format,
        bytes,
        raw_width,
        raw_height,
        orientation,
    })
}

/// A preview of an image at most `side` pixels on its longer side, turned
/// the way the camera said: JPEG for photos, PNG for the rest (screenshots
/// stay sharp and transparency survives).
pub fn thumbnail(
    bytes: &[u8],
    format: ImageFormat,
    raw_width: u32,
    raw_height: u32,
    orientation: u8,
    side: u32,
) -> Option<Thumbnail> {
    let (width, height) = if orientation >= 5 {
        (raw_height, raw_width)
    } else {
        (raw_width, raw_height)
    };
    let (w, h) = (f64::from(raw_width), f64::from(raw_height));
    let transform = match orientation {
        2 => format!("matrix(-1 0 0 1 {w} 0)"),
        3 => format!("matrix(-1 0 0 -1 {w} {h})"),
        4 => format!("matrix(1 0 0 -1 0 {h})"),
        5 => "matrix(0 1 1 0 0 0)".to_owned(),
        6 => format!("matrix(0 1 -1 0 {h} 0)"),
        7 => format!("matrix(0 -1 -1 0 {h} {w})"),
        8 => format!("matrix(0 -1 1 0 0 {w})"),
        _ => "matrix(1 0 0 1 0 0)".to_owned(),
    };
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><image width="{raw_width}" height="{raw_height}" preserveAspectRatio="none" transform="{transform}" xlink:href="data:{};base64,{data}"/></svg>"#,
        format.svg_media_type(),
    );
    drop(data);
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()?;
    let scale = (f64::from(side) / f64::from(width.max(height))).min(1.0);
    let out_width = ((f64::from(width) * scale).round() as u32).max(1);
    let out_height = ((f64::from(height) * scale).round() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(out_width, out_height)?;
    let photo = format == ImageFormat::Jpeg;
    if photo {
        pixmap.fill(tiny_skia::Color::WHITE);
    }
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );
    if photo {
        let mut out = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut out, 80);
        encoder
            .encode(
                pixmap.data(),
                u16::try_from(out_width).ok()?,
                u16::try_from(out_height).ok()?,
                jpeg_encoder::ColorType::Rgba,
            )
            .ok()?;
        Some(Thumbnail {
            bytes: out,
            content_type: "image/jpeg",
        })
    } else {
        Some(Thumbnail {
            bytes: pixmap.encode_png().ok()?,
            content_type: "image/png",
        })
    }
}

// ---------------------------------------------------------------------------
// Metadata

/// A JPEG without its EXIF and XMP segments (which carry GPS location), and
/// the camera's orientation, which is kept as a minimal EXIF segment so the
/// photo still shows the right way up. None if the JPEG is malformed.
fn strip_jpeg(bytes: &[u8]) -> Option<(Vec<u8>, u8)> {
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..2]);
    let mut orientation = 1;
    let mut position = 2;
    let mut orientation_at = None;
    loop {
        if position + 4 > bytes.len() || bytes[position] != 0xff {
            return None;
        }
        let marker = bytes[position + 1];
        // Fill bytes before a marker.
        if marker == 0xff {
            position += 1;
            continue;
        }
        // Start of scan: the rest is image data.
        if marker == 0xda {
            if let Some(at) = orientation_at
                && orientation != 1
            {
                let segment = orientation_segment(orientation);
                out.splice(at..at, segment);
            }
            out.extend_from_slice(&bytes[position..]);
            return Some((out, orientation));
        }
        let length = usize::from(u16::from_be_bytes([
            bytes[position + 2],
            bytes[position + 3],
        ]));
        if length < 2 || position + 2 + length > bytes.len() {
            return None;
        }
        let segment = &bytes[position..position + 2 + length];
        let payload = &segment[4..];
        let drop = match marker {
            0xe1 if payload.starts_with(b"Exif\0\0") => {
                if let Some(value) = exif_orientation(&payload[6..]) {
                    orientation = value;
                }
                true
            }
            0xe1 => payload.starts_with(b"http://ns.adobe.com/"),
            // Comments can carry anything; APP13 is Photoshop's IPTC block.
            0xfe | 0xed => true,
            _ => false,
        };
        // The orientation goes right after the JFIF header, where EXIF was.
        if orientation_at.is_none() && marker != 0xe0 {
            orientation_at = Some(out.len());
        }
        if !drop {
            out.extend_from_slice(segment);
        }
        position += 2 + length;
    }
}

/// The orientation tag (0x0112) of an EXIF TIFF block's first directory.
fn exif_orientation(tiff: &[u8]) -> Option<u8> {
    let big_endian = match tiff.get(..4)? {
        [b'M', b'M', 0, 42] => true,
        [b'I', b'I', 42, 0] => false,
        _ => return None,
    };
    let read_u16 = |at: usize| -> Option<u16> {
        let pair = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        })
    };
    let read_u32 = |at: usize| -> Option<u32> {
        let quad = [
            *tiff.get(at)?,
            *tiff.get(at + 1)?,
            *tiff.get(at + 2)?,
            *tiff.get(at + 3)?,
        ];
        Some(if big_endian {
            u32::from_be_bytes(quad)
        } else {
            u32::from_le_bytes(quad)
        })
    };
    let directory = usize::try_from(read_u32(4)?).ok()?;
    let entries = usize::from(read_u16(directory)?);
    for index in 0..entries.min(256) {
        let entry = directory + 2 + index * 12;
        if read_u16(entry)? == 0x0112 {
            let value = read_u16(entry + 8)?;
            return u8::try_from(value)
                .ok()
                .filter(|value| (1..=8).contains(value));
        }
    }
    None
}

/// An APP1 segment holding only an EXIF orientation.
fn orientation_segment(orientation: u8) -> Vec<u8> {
    let mut segment = vec![0xff, 0xe1, 0x00, 0x22];
    segment.extend_from_slice(b"Exif\0\0");
    // Big-endian TIFF header, first directory at offset 8.
    segment.extend_from_slice(&[b'M', b'M', 0, 42, 0, 0, 0, 8]);
    // One entry: orientation, SHORT, count 1, the value, padding.
    segment.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
    // No next directory.
    segment.extend_from_slice(&[0, 0, 0, 0]);
    segment
}

/// A PNG without its EXIF and text chunks. None if the PNG is malformed.
fn strip_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..8]);
    let mut position = 8;
    while position < bytes.len() {
        let length = usize::try_from(u32::from_be_bytes(
            bytes.get(position..position + 4)?.try_into().ok()?,
        ))
        .ok()?;
        let end = position.checked_add(12)?.checked_add(length)?;
        let chunk = bytes.get(position..end)?;
        let kind = &chunk[4..8];
        if !matches!(kind, b"eXIf" | b"tEXt" | b"iTXt" | b"zTXt") {
            out.extend_from_slice(chunk);
        }
        position = end;
        if kind == b"IEND" {
            break;
        }
    }
    Some(out)
}

/// A WebP without its EXIF and XMP chunks. None if the WebP is malformed.
fn strip_webp(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut chunks = Vec::with_capacity(bytes.len());
    let mut position = 12;
    let mut vp8x_at = None;
    while position + 8 <= bytes.len() {
        let kind = &bytes[position..position + 4];
        let length = usize::try_from(u32::from_le_bytes(
            bytes[position + 4..position + 8].try_into().ok()?,
        ))
        .ok()?;
        let padded = length + (length & 1);
        let end = position
            .checked_add(8)?
            .checked_add(padded)?
            .min(bytes.len());
        if position + 8 + length > bytes.len() {
            return None;
        }
        if kind == b"VP8X" {
            vp8x_at = Some(chunks.len());
        }
        if !matches!(kind, b"EXIF" | b"XMP ") {
            chunks.extend_from_slice(&bytes[position..end]);
        }
        position = end;
    }
    if let Some(at) = vp8x_at {
        // Clear the "has EXIF" (0x08) and "has XMP" (0x04) flags.
        if let Some(flags) = chunks.get_mut(at + 8) {
            *flags &= !0x0c;
        }
    }
    let size = u32::try_from(chunks.len() + 4).ok()?;
    let mut out = Vec::with_capacity(chunks.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&chunks);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG header with JFIF, an EXIF block holding orientation 6 and a GPS
    /// pointer, an XMP block, then a (fake) scan.
    fn jpeg_with_exif() -> Vec<u8> {
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        // Two entries: orientation 6, GPS IFD pointer.
        tiff.extend_from_slice(&[2, 0]);
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend_from_slice(&[0x25, 0x88, 4, 0, 1, 0, 0, 0, 38, 0, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        tiff.extend_from_slice(b"GPS 52.5200 N 13.4050 E");
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend_from_slice(&tiff);
        let mut bytes = vec![0xff, 0xd8];
        bytes.extend_from_slice(&[0xff, 0xe0, 0, 16]);
        bytes.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        bytes.extend_from_slice(&[0xff, 0xe1]);
        bytes.extend_from_slice(
            &u16::try_from(exif.len() + 2)
                .expect("test fixture")
                .to_be_bytes(),
        );
        bytes.extend_from_slice(&exif);
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>GPSLatitude</x:xmpmeta>";
        bytes.extend_from_slice(&[0xff, 0xe1]);
        bytes.extend_from_slice(
            &u16::try_from(xmp.len() + 2)
                .expect("test fixture")
                .to_be_bytes(),
        );
        bytes.extend_from_slice(xmp);
        bytes.extend_from_slice(&[0xff, 0xda, 0, 2, 1, 2, 3, 0xff, 0xd9]);
        bytes
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn jpeg_location_is_stripped_and_orientation_kept() {
        let (stripped, orientation) = strip_jpeg(&jpeg_with_exif()).expect("strip");
        assert_eq!(orientation, 6);
        assert!(!contains(&stripped, b"GPS"));
        assert!(!contains(&stripped, b"xmpmeta"));
        assert!(contains(&stripped, b"JFIF"));
        // The kept orientation reads back.
        let at = stripped
            .windows(6)
            .position(|window| window == b"Exif\0\0")
            .expect("orientation segment");
        assert_eq!(exif_orientation(&stripped[at + 6..]), Some(6));
        assert!(stripped.ends_with(&[0xff, 0xda, 0, 2, 1, 2, 3, 0xff, 0xd9]));
    }

    #[test]
    fn png_text_and_exif_chunks_are_dropped() {
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
            .expect("test fixture");
        // Insert a tEXt chunk after IHDR (8 + 25 bytes).
        let mut with_text = png[..33].to_vec();
        let text = b"Comment\0lat 52.52";
        with_text.extend_from_slice(
            &u32::try_from(text.len())
                .expect("test fixture")
                .to_be_bytes(),
        );
        with_text.extend_from_slice(b"tEXt");
        with_text.extend_from_slice(text);
        with_text.extend_from_slice(&[0, 0, 0, 0]);
        with_text.extend_from_slice(&png[33..]);
        let stripped = strip_png(&with_text).expect("strip");
        assert_eq!(stripped, png);
        let processed = process(with_text).expect("image");
        assert_eq!((processed.width, processed.height), (1, 1));
        assert!(processed.thumbnail.is_none());
    }

    #[test]
    fn only_raster_images_are_inline() {
        assert_eq!(sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), None);
        assert_eq!(sniff(b"GIF89a...."), Some(ImageFormat::Gif));
        assert!(process(b"\x89PNG\r\n\x1a\nnot really".to_vec()).is_none());
    }

    #[test]
    fn thumbnails_turn_with_the_camera() {
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
            .expect("test fixture");
        let thumbnail = thumbnail(&png, ImageFormat::Png, 1, 1, 6, 64).expect("thumbnail");
        assert_eq!(thumbnail.content_type, "image/png");
        assert!(thumbnail.bytes.starts_with(b"\x89PNG"));
    }
}
