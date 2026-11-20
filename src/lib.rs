//! Shared native interactive HTML renderer, on-demand QuickJS sandbox, content sniffer,
//! and lazy localhost Axum server for BiteFoundry terminal applications.

pub mod dom;
#[cfg(feature = "js")]
pub mod js;
pub mod render;
#[cfg(feature = "serve")]
pub mod serve;
pub mod session;
pub mod sniff;

pub use dom::{DomDocument, DomNode, DomNodeId, DomNodeKind, ElementData};
#[cfg(feature = "js")]
pub use js::JsSession;
pub use render::{
    render_dom, render_raw_html, ControlKind, HtmlTheme, InteractiveControl, RenderedHtml,
};
#[cfg(feature = "serve")]
pub use serve::NativeHtmlServer;
pub use session::{DomSharedState, HtmlEffect, HtmlViewSession};
pub use sniff::{
    is_safe_raster_image, looks_like_html_bytes, looks_like_svg_bytes, sniff_content_kind,
    ContentKind,
};
