//! Pure-Rust SVG rasterization (`resvg` + `tiny-skia`) and `data:image/*` URI decoding.

use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use resvg::tiny_skia::{Color, Pixmap, Transform};
use resvg::usvg::{self, fontdb};

use crate::sniff::{sniff_content_kind, ContentKind};

/// Maximum pixel dimension allowed for rasterized SVG / Canvas output (prevents OOM).
pub const MAX_RASTER_DIM_PX: u32 = 2048;

/// Maximum decoded byte size accepted for `data:image/*` URIs (8 MiB).
pub const MAX_DATA_URI_BYTES: usize = 8 * 1024 * 1024;

/// Rasterized PNG image produced from an inline `<svg>` or `<canvas>`.
#[derive(Debug, Clone)]
pub struct RasterizedImage {
    pub png_bytes: Arc<[u8]>,
    pub width_px: u32,
    pub height_px: u32,
}

/// Decoded payload extracted from a `data:image/*` URI.
#[derive(Debug, Clone)]
pub struct DecodedDataUri {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub is_svg: bool,
}

static FONTDB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

/// Lazily loads system fonts once per process for `resvg` text rendering.
pub fn shared_fontdb() -> Arc<fontdb::Database> {
    Arc::clone(FONTDB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    }))
}

/// Ensures an HTML5 inline `<svg ...>` has an XML SVG namespace (`xmlns`) so `usvg` parses it cleanly.
fn ensure_svg_xmlns(svg_bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    // Gzip-compressed `.svgz` starts with 0x1F 0x8B; pass straight through to `usvg`.
    if svg_bytes.starts_with(&[0x1F, 0x8B]) {
        return std::borrow::Cow::Borrowed(svg_bytes);
    }
    let Ok(text) = std::str::from_utf8(svg_bytes) else {
        return std::borrow::Cow::Borrowed(svg_bytes);
    };

    let lower = text.to_ascii_lowercase();
    let Some(svg_pos) = lower.find("<svg") else {
        return std::borrow::Cow::Borrowed(svg_bytes);
    };
    // Ensure `<svg` is followed by whitespace, `>`, or `/`.
    let after_tag = svg_pos + 4;
    if let Some(&b) = lower.as_bytes().get(after_tag) {
        if !b.is_ascii_whitespace() && b != b'>' && b != b'/' {
            return std::borrow::Cow::Borrowed(svg_bytes);
        }
    }

    let Some(close_rel) = lower[svg_pos..].find('>') else {
        return std::borrow::Cow::Borrowed(svg_bytes);
    };
    let open_tag_lower = &lower[svg_pos..svg_pos + close_rel];
    let needs_xmlns = !open_tag_lower.contains("xmlns=");
    let needs_xlink = lower.contains("xlink:href") && !open_tag_lower.contains("xmlns:xlink=");

    if !needs_xmlns && !needs_xlink {
        return std::borrow::Cow::Borrowed(svg_bytes);
    }

    let mut out = String::with_capacity(text.len() + 80);
    out.push_str(&text[..after_tag]);
    if needs_xmlns {
        out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
    }
    if needs_xlink {
        out.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
    }
    out.push_str(&text[after_tag..]);
    std::borrow::Cow::Owned(out.into_bytes())
}

/// Rasterizes raw SVG bytes into a PNG [`RasterizedImage`] bounded to
/// `max_width_px` × `max_height_px` (capped at `2048×2048`).
pub fn rasterize_svg_to_png(
    svg_bytes: &[u8],
    max_width_px: u32,
    max_height_px: u32,
    bg_rgba: Option<[u8; 4]>,
) -> Result<RasterizedImage> {
    if svg_bytes.is_empty() {
        bail!("empty SVG input");
    }
    let normalized = ensure_svg_xmlns(svg_bytes);

    let opts = usvg::Options {
        fontdb: shared_fontdb(),
        font_family: "sans-serif".to_string(),
        ..usvg::Options::default()
    };

    let tree = usvg::Tree::from_data(&normalized, &opts)
        .map_err(|e| anyhow!("SVG parse failed: {e}"))?;

    let src_size = tree.size();
    let src_w = src_size.width();
    let src_h = src_size.height();
    if src_w <= 0.0 || src_h <= 0.0 || !src_w.is_finite() || !src_h.is_finite() {
        bail!("SVG has invalid dimensions ({src_w}x{src_h})");
    }

    let cap_w = max_width_px.clamp(1, MAX_RASTER_DIM_PX) as f32;
    let cap_h = max_height_px.clamp(1, MAX_RASTER_DIM_PX) as f32;
    let scale = (cap_w / src_w).min(cap_h / src_h).min(1.0);

    let out_w = (src_w * scale).round().clamp(1.0, MAX_RASTER_DIM_PX as f32) as u32;
    let out_h = (src_h * scale).round().clamp(1.0, MAX_RASTER_DIM_PX as f32) as u32;

    let mut pixmap =
        Pixmap::new(out_w, out_h).ok_or_else(|| anyhow!("failed to allocate {out_w}x{out_h} pixmap"))?;

    if let Some([r, g, b, a]) = bg_rgba {
        pixmap.fill(Color::from_rgba8(r, g, b, a));
    }

    let sx = out_w as f32 / src_w;
    let sy = out_h as f32 / src_h;
    resvg::render(&tree, Transform::from_scale(sx, sy), &mut pixmap.as_mut());

    let png = pixmap
        .encode_png()
        .map_err(|e| anyhow!("PNG encode failed: {e}"))?;

    Ok(RasterizedImage {
        png_bytes: Arc::from(png),
        width_px: out_w,
        height_px: out_h,
    })
}

/// Decodes a `data:image/*` URI, enforces the 8 MiB size limit, and re-verifies the decoded
/// payload with [`sniff_content_kind`] so spoofed HTML/script payloads are rejected.
pub fn decode_data_image_uri(uri: &str) -> Result<DecodedDataUri> {
    let trimmed = uri.trim();
    if trimmed.len() < 12 || !trimmed[..5].eq_ignore_ascii_case("data:") {
        bail!("URI is not a data: URI");
    }
    // Reject absurdly large raw strings upfront before allocating decode buffers.
    if trimmed.len() > MAX_DATA_URI_BYTES * 2 {
        bail!("data:image URI exceeds maximum allowed size");
    }

    let rest = &trimmed[5..];
    let Some((meta, payload)) = rest.split_once(',') else {
        bail!("malformed data: URI (missing comma separator)");
    };

    let mut parts = meta.split(';');
    let mime = parts
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let is_svg = mime == "image/svg+xml";
    let supported_mime = matches!(
        mime.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/x-ms-bmp"
            | "image/svg+xml"
    );
    if !supported_mime {
        bail!("unsupported data URI MIME type: {mime}");
    }

    let is_base64 = parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));

    let bytes = if is_base64 {
        let compact: String = payload.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        if compact.len() > (MAX_DATA_URI_BYTES * 4) / 3 + 16 {
            bail!("base64 data URI payload exceeds 8 MiB limit");
        }
        STANDARD
            .decode(compact.as_bytes())
            .or_else(|_| STANDARD_NO_PAD.decode(compact.as_bytes()))
            .map_err(|e| anyhow!("invalid base64 payload in data URI: {e}"))?
    } else {
        percent_decode_bytes(payload)
    };

    if bytes.is_empty() {
        bail!("empty data:image payload");
    }
    if bytes.len() > MAX_DATA_URI_BYTES {
        bail!("decoded data:image payload exceeds 8 MiB limit");
    }

    let kind = sniff_content_kind(Some(&mime), None, Some(&bytes));
    if is_svg {
        if kind != ContentKind::Svg {
            bail!("data:image/svg+xml payload failed SVG verification (detected {kind:?})");
        }
    } else if kind != ContentKind::RasterImage {
        bail!("data:{mime} payload failed raster magic verification (detected {kind:?})");
    }

    Ok(DecodedDataUri {
        bytes,
        mime,
        is_svg,
    })
}

fn percent_decode_bytes(input: &str) -> Vec<u8> {
    let raw = input.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let (Some(hi), Some(lo)) = (hex_val(raw[i + 1]), hex_val(raw[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Lightweight header-only dimension probe for standard raster image formats (PNG, GIF, BMP, JPEG, WebP).
pub fn probe_raster_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // PNG: 8-byte sig + IHDR length(4) + "IHDR"(4) + width(4) + height(4)
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }
    // GIF: GIF87a/GIF89a + width(LE u16) + height(LE u16)
    if bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }
    // BMP: "BM" + ... + width(LE i32 @ 18) + height(LE i32 @ 22)
    if bytes.len() >= 26 && bytes.starts_with(b"BM") {
        let w = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]).unsigned_abs();
        let h = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]).unsigned_abs();
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }
    // WebP: RIFF....WEBP + VP8X/VP8 /VP8L
    if bytes.len() >= 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        match &bytes[12..16] {
            b"VP8X" => {
                let w = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
                let h = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
                return Some((w, h));
            }
            b"VP8 " if bytes.len() >= 30 => {
                let w = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
                let h = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
            b"VP8L" if bytes.len() >= 25 => {
                let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
                let w = (bits & 0x3FFF) + 1;
                let h = ((bits >> 14) & 0x3FFF) + 1;
                return Some((w, h));
            }
            _ => {}
        }
    }
    // JPEG: scan markers for SOF0..SOF3
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2usize;
        while i + 4 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            while i < bytes.len() && bytes[i] == 0xFF {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let marker = bytes[i];
            i += 1;
            if matches!(marker, 0xD8 | 0xD9 | 0x01) {
                continue;
            }
            if i + 2 > bytes.len() {
                break;
            }
            let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
            if matches!(marker, 0xC0..=0xC3) && i + 7 < bytes.len() {
                let h = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
            if seg_len < 2 {
                break;
            }
            i += seg_len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterize_html5_svg_without_xmlns_and_viewbox_only() {
        let svg = br##"<svg width="100" height="50"><rect width="100" height="50" fill="#e11d48"/><text x="10" y="30" fill="#ffffff">Chart</text></svg>"##;
        let img = rasterize_svg_to_png(svg, 400, 400, None).expect("rasterize svg");
        assert!(img.png_bytes.starts_with(b"\x89PNG"));
        assert_eq!(img.width_px, 100);
        assert_eq!(img.height_px, 50);

        let viewbox_only = br#"<svg viewBox="0 0 80 40"><circle cx="40" cy="20" r="15" fill="blue"/></svg>"#;
        let vb_img = rasterize_svg_to_png(viewbox_only, 400, 400, None).expect("viewBox svg");
        assert!(vb_img.png_bytes.starts_with(b"\x89PNG"));
        assert_eq!(vb_img.width_px, 80);
        assert_eq!(vb_img.height_px, 40);
    }

    #[test]
    fn decode_data_image_uri_accepts_valid_and_rejects_spoofed() {
        // Generate a real 2x2 PNG via tiny-skia.
        let pm = Pixmap::new(2, 2).unwrap();
        let png = pm.encode_png().unwrap();
        let b64 = STANDARD.encode(&png);
        let uri = format!("data:image/png;base64,{b64}");
        let decoded = decode_data_image_uri(&uri).expect("valid png data uri");
        assert!(!decoded.is_svg);
        assert_eq!(decoded.mime, "image/png");
        assert!(decoded.bytes.starts_with(b"\x89PNG"));
        assert_eq!(probe_raster_dimensions(&decoded.bytes), Some((2, 2)));

        // Percent-encoded SVG
        let svg_uri = "data:image/svg+xml,%3Csvg%20width%3D%2210%22%20height%3D%2210%22%3E%3Crect%20width%3D%2210%22%20height%3D%2210%22%20fill%3D%22red%22/%3E%3C/svg%3E";
        let svg_dec = decode_data_image_uri(svg_uri).expect("valid percent-encoded svg");
        assert!(svg_dec.is_svg);

        // Spoofed HTML disguised as PNG must be rejected
        let spoofed = "data:image/png;base64,PGh0bWw+PGJvZHk+SGk8L2JvZHk+PC9odG1sPg==";
        assert!(decode_data_image_uri(spoofed).is_err());
    }
}
