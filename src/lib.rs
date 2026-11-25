//! Shared native interactive HTML & Markdown renderer, Pure-Rust layout & graphics engine,
//! on-demand QuickJS sandbox, content sniffer, and lazy localhost Axum server for BiteFoundry terminal apps.

#[cfg(feature = "graphics")]
pub mod canvas;
pub mod dom;
#[cfg(feature = "graphics")]
pub mod graphics;
#[cfg(feature = "js")]
pub mod js;
pub mod layout;
pub mod markdown;
pub mod render;
#[cfg(feature = "serve")]
pub mod serve;
pub mod session;
pub mod sniff;

pub use pulldown_cmark;

#[cfg(feature = "graphics")]
pub use canvas::CanvasSurface;
pub use dom::{DomDocument, DomNode, DomNodeId, DomNodeKind, ElementData};
#[cfg(feature = "graphics")]
pub use graphics::{decode_data_image_uri, rasterize_svg_to_png, DecodedDataUri, RasterizedImage};
#[cfg(feature = "js")]
pub use js::JsSession;
pub use layout::{
    pad_display_width, render_wrapped_table, windowed_single_line_input, wrap_styled_spans,
    TableAlign, TableBorder, TableSpec, WindowedInput, WrapPrefix,
};
pub use markdown::{
    markdown_to_html_document, render_markdown, render_markdown_document, render_raw_markdown,
};
pub use render::{
    render_dom, render_raw_html, ControlKind, GraphicKind, HtmlTheme, InteractiveControl,
    RenderedGraphic, RenderedHtml,
};
#[cfg(feature = "serve")]
pub use serve::NativeHtmlServer;
pub use session::{DomSharedState, HtmlEffect, HtmlViewSession, SourceKind};
pub use sniff::{
    is_safe_raster_image, looks_like_html_bytes, looks_like_svg_bytes, sniff_content_kind,
    ContentKind,
};
