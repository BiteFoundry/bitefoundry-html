//! Strict content classification to prevent HTML/SVG-as-image misclassification.

/// Normalized high-level content classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    RasterImage,
    Svg,
    Html,
    Markdown,
    Pdf,
    PlainText,
    Binary,
}

const HTML_TAG_PREFIXES: &[&[u8]] = &[
    b"<!doctype html",
    b"<html",
    b"<head",
    b"<body",
    b"<div",
    b"<p",
    b"<table",
    b"<script",
    b"<style",
    b"<form",
    b"<button",
    b"<section",
    b"<article",
    b"<main",
    b"<h1",
    b"<h2",
    b"<h3",
    b"<h4",
    b"<h5",
    b"<h6",
    b"<span",
    b"<nav",
    b"<header",
    b"<footer",
    b"<ul",
    b"<ol",
    b"<pre",
    b"<blockquote",
    b"<meta",
    b"<title",
    b"<link",
    b"<iframe",
];

fn strip_bom_and_ws(mut bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes = &bytes[3..];
    }
    while let Some(&first) = bytes.first() {
        if first.is_ascii_whitespace() {
            bytes = &bytes[1..];
        } else {
            break;
        }
    }
    bytes
}

fn starts_with_tag_ci(bytes: &[u8], prefix: &[u8]) -> bool {
    if bytes.len() < prefix.len() {
        return false;
    }
    if !bytes[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return false;
    }
    if prefix == b"<!doctype html" {
        return bytes
            .get(prefix.len())
            .is_none_or(|b| b.is_ascii_whitespace() || *b == b'>');
    }
    match bytes.get(prefix.len()) {
        None => false,
        Some(&next) => next == b'>' || next == b'/' || next.is_ascii_whitespace(),
    }
}

fn contains_tag_ci(haystack: &[u8], tag: &[u8]) -> bool {
    if haystack.len() < tag.len() {
        return false;
    }
    for i in 0..=haystack.len() - tag.len() {
        if starts_with_tag_ci(&haystack[i..], tag) {
            return true;
        }
    }
    false
}

/// Checks whether raw bytes look like an SVG document (`<svg` or `<?xml` containing `<svg`).
pub fn looks_like_svg_bytes(bytes: &[u8]) -> bool {
    let mut slice = strip_bom_and_ws(bytes);
    let window_len = slice.len().min(4096);
    slice = &slice[..window_len];

    // Skip leading XML/HTML comments.
    while slice.starts_with(b"<!--") {
        if let Some(end) = slice.windows(3).position(|w| w == b"-->") {
            slice = strip_bom_and_ws(&slice[end + 3..]);
        } else {
            break;
        }
    }

    if starts_with_tag_ci(slice, b"<svg") {
        return true;
    }

    let is_xml_or_svg_doctype = (slice.len() >= 5 && slice[..5].eq_ignore_ascii_case(b"<?xml"))
        || (slice.len() >= 13 && slice[..13].eq_ignore_ascii_case(b"<!doctype svg"));

    if is_xml_or_svg_doctype {
        return contains_tag_ci(slice, b"<svg");
    }

    false
}

/// Checks whether raw bytes look like an HTML document or HTML fragment.
pub fn looks_like_html_bytes(bytes: &[u8]) -> bool {
    if looks_like_svg_bytes(bytes) {
        return false;
    }

    let mut slice = strip_bom_and_ws(bytes);
    let window_len = slice.len().min(4096);
    slice = &slice[..window_len];

    if slice.is_empty() {
        return false;
    }

    let mut saw_comment = false;
    while slice.starts_with(b"<!--") {
        saw_comment = true;
        if let Some(end) = slice.windows(3).position(|w| w == b"-->") {
            slice = strip_bom_and_ws(&slice[end + 3..]);
        } else {
            break;
        }
    }

    if HTML_TAG_PREFIXES
        .iter()
        .any(|prefix| starts_with_tag_ci(slice, prefix))
    {
        return true;
    }

    if saw_comment {
        return HTML_TAG_PREFIXES
            .iter()
            .any(|prefix| contains_tag_ci(slice, prefix));
    }

    false
}

fn has_raster_magic(bytes: &[u8]) -> bool {
    // PNG: \x89PNG
    if bytes.starts_with(b"\x89PNG") {
        return true;
    }
    // JPEG: \xFF\xD8\xFF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return true;
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return true;
    }
    // BMP: BM followed by valid BMP reserved bytes or binary header data (avoid plain text "BM ...")
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        if bytes.len() >= 10 && bytes[6..10] == [0, 0, 0, 0] {
            return true;
        }
        if bytes.len() < 10
            && (bytes.len() == 2
                || bytes[2..]
                    .iter()
                    .any(|b| !b.is_ascii_graphic() && !b.is_ascii_whitespace()))
        {
            return true;
        }
    }
    false
}

fn classify_mime(mime_hint: &str) -> Option<ContentKind> {
    let base = mime_hint
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "text/html" | "application/xhtml+xml" => Some(ContentKind::Html),
        "image/svg+xml" => Some(ContentKind::Svg),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp"
        | "image/x-ms-bmp" => Some(ContentKind::RasterImage),
        "text/markdown" | "text/x-markdown" => Some(ContentKind::Markdown),
        "application/pdf" => Some(ContentKind::Pdf),
        "text/plain" => Some(ContentKind::PlainText),
        _ => None,
    }
}

fn classify_extension(file_name: &str) -> Option<ContentKind> {
    let trimmed = file_name
        .split(['?', '#'])
        .next()
        .unwrap_or(file_name)
        .trim();
    let ext = trimmed.rsplit('.').next()?.to_ascii_lowercase();
    if trimmed.len() == ext.len() {
        // No dot in file name.
        return None;
    }
    match ext.as_str() {
        "html" | "htm" | "xhtml" => Some(ContentKind::Html),
        "svg" | "svgz" => Some(ContentKind::Svg),
        "md" | "markdown" | "mdx" => Some(ContentKind::Markdown),
        "pdf" => Some(ContentKind::Pdf),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Some(ContentKind::RasterImage),
        "txt" | "log" | "csv" => Some(ContentKind::PlainText),
        _ => None,
    }
}

fn looks_like_plain_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return false;
    }
    std::str::from_utf8(sample).is_ok()
}

/// Strictly classifies content using bytes (when available), MIME hint, and filename.
///
/// Critical invariant: when `bytes` are provided, real raster magic bytes are checked first.
/// If bytes are not a valid raster image, SVG and HTML sniffing take precedence over MIME
/// or filename hints so an HTML error page or SVG served at a `.png` URL is never classified
/// as `ContentKind::RasterImage`.
pub fn sniff_content_kind(
    mime_hint: Option<&str>,
    file_name: Option<&str>,
    bytes: Option<&[u8]>,
) -> ContentKind {
    if let Some(b) = bytes {
        if !b.is_empty() {
            if has_raster_magic(b) {
                return ContentKind::RasterImage;
            }
            if looks_like_svg_bytes(b) {
                return ContentKind::Svg;
            }
            if looks_like_html_bytes(b) {
                let explicit_markdown =
                    mime_hint.and_then(classify_mime) == Some(ContentKind::Markdown)
                        || file_name.and_then(classify_extension) == Some(ContentKind::Markdown);
                let slice = strip_bom_and_ws(b);
                let is_full_html_doc = starts_with_tag_ci(slice, b"<!doctype html")
                    || starts_with_tag_ci(slice, b"<html");
                if explicit_markdown && !is_full_html_doc && looks_like_plain_text(b) {
                    return ContentKind::Markdown;
                }
                return ContentKind::Html;
            }
            if b.starts_with(b"%PDF-") {
                return ContentKind::Pdf;
            }

            // Bytes failed raster magic check, so never trust a raster MIME/extension hint.
            if let Some(kind) = mime_hint.and_then(classify_mime) {
                if kind != ContentKind::RasterImage {
                    return kind;
                }
            }
            if let Some(kind) = file_name.and_then(classify_extension) {
                if kind != ContentKind::RasterImage {
                    return kind;
                }
            }

            return if looks_like_plain_text(b) {
                ContentKind::PlainText
            } else {
                ContentKind::Binary
            };
        }
    }

    if let Some(kind) = mime_hint.and_then(classify_mime) {
        return kind;
    }
    if let Some(kind) = file_name.and_then(classify_extension) {
        return kind;
    }

    ContentKind::Binary
}

/// Returns `true` strictly when content is a verified/safe raster image (never SVG or HTML).
pub fn is_safe_raster_image(
    mime_hint: Option<&str>,
    file_name: Option<&str>,
    bytes: Option<&[u8]>,
) -> bool {
    sniff_content_kind(mime_hint, file_name, bytes) == ContentKind::RasterImage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_magic_bytes_detected() {
        assert_eq!(
            sniff_content_kind(None, None, Some(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR")),
            ContentKind::RasterImage
        );
        assert_eq!(
            sniff_content_kind(None, None, Some(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10])),
            ContentKind::RasterImage
        );
        assert_eq!(
            sniff_content_kind(None, None, Some(b"GIF89a\x01\x00\x01\x00")),
            ContentKind::RasterImage
        );
        assert_eq!(
            sniff_content_kind(None, None, Some(b"GIF87a\x01\x00")),
            ContentKind::RasterImage
        );
        assert_eq!(
            sniff_content_kind(None, None, Some(b"RIFF\x24\x00\x00\x00WEBPVP8 ")),
            ContentKind::RasterImage
        );
        assert_eq!(
            sniff_content_kind(
                None,
                None,
                Some(b"BM\x36\x00\x00\x00\x00\x00\x00\x00\x36\x00\x00\x00")
            ),
            ContentKind::RasterImage
        );
        assert!(is_safe_raster_image(
            Some("image/png"),
            Some("icon.png"),
            Some(b"\x89PNG\r\n\x1a\n")
        ));
    }

    #[test]
    fn html_returned_for_png_url_is_html_not_raster() {
        let html = b"\xEF\xBB\xBF  <!DOCTYPE html><html><body>Login required</body></html>";
        assert_eq!(
            sniff_content_kind(Some("image/png"), Some("avatar.png"), Some(html)),
            ContentKind::Html
        );
        assert!(!is_safe_raster_image(
            Some("image/png"),
            Some("avatar.png"),
            Some(html)
        ));

        let commented_html = b"<!-- server notice -->\n<div class=\"err\">Forbidden</div>";
        assert!(looks_like_html_bytes(commented_html));
        assert_eq!(
            sniff_content_kind(Some("image/jpeg"), Some("photo.jpg"), Some(commented_html)),
            ContentKind::Html
        );
    }

    #[test]
    fn svg_is_never_raster_image() {
        assert_eq!(
            sniff_content_kind(Some("image/svg+xml"), Some("logo.svg"), None),
            ContentKind::Svg
        );
        assert!(!is_safe_raster_image(
            Some("image/svg+xml"),
            Some("logo.svg"),
            None
        ));

        let svg_bytes = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"10\"/></svg>";
        assert!(looks_like_svg_bytes(svg_bytes));
        assert!(!looks_like_html_bytes(svg_bytes));
        assert_eq!(
            sniff_content_kind(Some("image/png"), Some("fake.png"), Some(svg_bytes)),
            ContentKind::Svg
        );
        assert!(!is_safe_raster_image(
            Some("image/png"),
            Some("fake.png"),
            Some(svg_bytes)
        ));
    }

    #[test]
    fn extensions_and_mimes_classified_cleanly() {
        assert_eq!(
            sniff_content_kind(None, Some("index.html"), None),
            ContentKind::Html
        );
        assert_eq!(
            sniff_content_kind(None, Some("page.htm"), None),
            ContentKind::Html
        );
        assert_eq!(
            sniff_content_kind(None, Some("doc.xhtml"), None),
            ContentKind::Html
        );
        assert_eq!(
            sniff_content_kind(Some("text/html; charset=utf-8"), None, None),
            ContentKind::Html
        );
        assert_eq!(
            sniff_content_kind(None, Some("README.md"), None),
            ContentKind::Markdown
        );
        assert_eq!(
            sniff_content_kind(None, Some("guide.mdx"), Some(b"\xEF\xBB\xBF# Title\nBody")),
            ContentKind::Markdown
        );
        assert_eq!(
            sniff_content_kind(
                Some("text/markdown; charset=utf-8"),
                Some("doc.markdown"),
                Some(b"\xEF\xBB\xBF<!-- comment -->\n# Heading\n")
            ),
            ContentKind::Markdown
        );
        assert_eq!(
            sniff_content_kind(Some("application/pdf"), None, None),
            ContentKind::Pdf
        );
    }
}
