//! Ratatui interactive HTML renderer with exact hit-map coordinates,
//! lossless Track D wrapping/tables, and Pure-Rust inline graphic slots.

#[cfg(feature = "graphics")]
use std::collections::HashMap;
use std::sync::Arc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

#[cfg(feature = "graphics")]
use crate::canvas::CanvasSurface;
use crate::dom::{DomDocument, DomNodeId, DomNodeKind, ElementData};
#[cfg(feature = "graphics")]
use crate::graphics::{
    decode_data_image_uri, probe_raster_dimensions, rasterize_svg_to_png, RasterizedImage,
};
use crate::layout::{
    render_wrapped_table, windowed_single_line_input, wrap_styled_spans, TableAlign, TableBorder,
    TableSpec, WrapPrefix,
};

/// Plain `ratatui::style::Color` theme for HTML rendering so any BiteFoundry app can pass its palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlTheme {
    pub text: Color,
    pub muted: Color,
    pub heading: Color,
    pub accent: Color,
    pub link: Color,
    pub code_fg: Color,
    pub code_bg: Color,
    pub quote_border: Color,
    pub border: Color,
    pub control_fg: Color,
    pub control_bg: Color,
    pub focus_fg: Color,
    pub focus_bg: Color,
    pub raw_tag: Color,
    pub raw_attr: Color,
}

impl HtmlTheme {
    pub fn dark() -> Self {
        Self {
            text: Color::Rgb(230, 234, 240),
            muted: Color::Rgb(140, 150, 165),
            heading: Color::Rgb(120, 200, 255),
            accent: Color::Rgb(95, 195, 255),
            link: Color::Rgb(110, 185, 255),
            code_fg: Color::Rgb(245, 215, 140),
            code_bg: Color::Rgb(38, 42, 52),
            quote_border: Color::Rgb(110, 130, 160),
            border: Color::Rgb(85, 95, 115),
            control_fg: Color::Rgb(235, 242, 255),
            control_bg: Color::Rgb(45, 65, 95),
            focus_fg: Color::Rgb(15, 20, 28),
            focus_bg: Color::Rgb(110, 205, 255),
            raw_tag: Color::Rgb(115, 190, 255),
            raw_attr: Color::Rgb(240, 200, 125),
        }
    }

    pub fn light() -> Self {
        Self {
            text: Color::Rgb(30, 34, 42),
            muted: Color::Rgb(110, 118, 132),
            heading: Color::Rgb(20, 85, 175),
            accent: Color::Rgb(20, 100, 195),
            link: Color::Rgb(15, 95, 200),
            code_fg: Color::Rgb(155, 45, 75),
            code_bg: Color::Rgb(238, 240, 245),
            quote_border: Color::Rgb(140, 150, 170),
            border: Color::Rgb(175, 182, 195),
            control_fg: Color::Rgb(20, 35, 60),
            control_bg: Color::Rgb(215, 230, 250),
            focus_fg: Color::Rgb(255, 255, 255),
            focus_bg: Color::Rgb(20, 100, 205),
            raw_tag: Color::Rgb(20, 95, 185),
            raw_attr: Color::Rgb(155, 85, 20),
        }
    }

    pub fn no_color() -> Self {
        Self {
            text: Color::Reset,
            muted: Color::Reset,
            heading: Color::Reset,
            accent: Color::Reset,
            link: Color::Reset,
            code_fg: Color::Reset,
            code_bg: Color::Reset,
            quote_border: Color::Reset,
            border: Color::Reset,
            control_fg: Color::Reset,
            control_bg: Color::Reset,
            focus_fg: Color::Reset,
            focus_bg: Color::Reset,
            raw_tag: Color::Reset,
            raw_attr: Color::Reset,
        }
    }
}

impl Default for HtmlTheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Interactive HTML control classification for focus & click dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlKind {
    Link { href: String },
    Button,
    DetailsSummary { open: bool },
    Checkbox { checked: bool },
    Radio { checked: bool },
    TextInput { value: String },
    Select { current_label: String },
}

/// Positioned interactive control in the rendered output.
#[derive(Debug, Clone)]
pub struct InteractiveControl {
    pub index: usize,
    pub node_id: DomNodeId,
    pub kind: ControlKind,
    pub label: String,
    pub line: usize,
    pub col_start: u16,
    pub col_end: u16,
}

/// Classification of an inline graphic embedded in an HTML document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicKind {
    InlineSvg,
    Canvas2d,
    DataImage,
}

/// Rasterized or decoded inline graphic occupying a reserved vertical slot in [`RenderedHtml::lines`].
#[derive(Debug, Clone)]
pub struct RenderedGraphic {
    pub node_id: DomNodeId,
    pub kind: GraphicKind,
    pub alt: Option<String>,
    /// Starting line index in [`RenderedHtml::lines`].
    pub line_start: usize,
    /// Number of placeholder rows reserved in [`RenderedHtml::lines`].
    pub line_count: usize,
    pub width_px: u32,
    pub height_px: u32,
    /// PNG bytes (from `resvg`/`tiny-skia`) or verified raster bytes (from `data:image/*`).
    pub image_bytes: Arc<[u8]>,
}

/// Complete rendered output for a document at a given viewport width.
#[derive(Debug, Clone)]
pub struct RenderedHtml {
    pub title: Option<String>,
    pub lines: Vec<Line<'static>>,
    pub controls: Vec<InteractiveControl>,
    pub graphics: Vec<RenderedGraphic>,
    pub external_scripts: Vec<String>,
}

/// Renders a [`DomDocument`] into wrapped Ratatui lines, clickable control coordinates,
/// and inline graphic slots.
pub fn render_dom(
    doc: &DomDocument,
    width: u16,
    theme: &HtmlTheme,
    focused_control: Option<usize>,
    editing_input: Option<DomNodeId>,
    show_link_urls: bool,
) -> RenderedHtml {
    #[cfg(feature = "graphics")]
    {
        render_dom_with_graphics(
            doc,
            width,
            theme,
            focused_control,
            editing_input,
            show_link_urls,
            None,
            None,
            0,
        )
    }
    #[cfg(not(feature = "graphics"))]
    {
        render_dom_internal(
            doc,
            width,
            theme,
            focused_control,
            editing_input,
            show_link_urls,
        )
    }
}

#[cfg(feature = "graphics")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_dom_with_graphics(
    doc: &DomDocument,
    width: u16,
    theme: &HtmlTheme,
    focused_control: Option<usize>,
    editing_input: Option<DomNodeId>,
    show_link_urls: bool,
    canvases: Option<&HashMap<DomNodeId, CanvasSurface>>,
    graphic_cache: Option<&mut HashMap<(u64, DomNodeId), RasterizedImage>>,
    dom_version: u64,
) -> RenderedHtml {
    let max_width = width.max(12) as usize;
    let mut ctx = RenderCtx {
        doc,
        width: max_width,
        theme,
        focused_control,
        editing_input,
        show_link_urls,
        lines: Vec::new(),
        controls: Vec::new(),
        graphics: Vec::new(),
        cur_spans: Vec::new(),
        cur_col: 0,
        prefix_stack: Vec::new(),
        prefix_emitted: false,
        canvases,
        graphic_cache,
        dom_version,
    };

    ctx.emit_external_scripts_banner();
    let base_style = Style::default().fg(theme.text);
    ctx.render_children(doc.root(), base_style);
    ctx.flush_line(false);
    ctx.trim_trailing_empty_lines();

    RenderedHtml {
        title: doc.title(),
        lines: ctx.lines,
        controls: ctx.controls,
        graphics: ctx.graphics,
        external_scripts: doc.external_scripts().to_vec(),
    }
}

#[cfg(not(feature = "graphics"))]
fn render_dom_internal(
    doc: &DomDocument,
    width: u16,
    theme: &HtmlTheme,
    focused_control: Option<usize>,
    editing_input: Option<DomNodeId>,
    show_link_urls: bool,
) -> RenderedHtml {
    let max_width = width.max(12) as usize;
    let mut ctx = RenderCtx {
        doc,
        width: max_width,
        theme,
        focused_control,
        editing_input,
        show_link_urls,
        lines: Vec::new(),
        controls: Vec::new(),
        graphics: Vec::new(),
        cur_spans: Vec::new(),
        cur_col: 0,
        prefix_stack: Vec::new(),
        prefix_emitted: false,
    };

    ctx.emit_external_scripts_banner();
    let base_style = Style::default().fg(theme.text);
    ctx.render_children(doc.root(), base_style);
    ctx.flush_line(false);
    ctx.trim_trailing_empty_lines();

    RenderedHtml {
        title: doc.title(),
        lines: ctx.lines,
        controls: ctx.controls,
        graphics: ctx.graphics,
        external_scripts: doc.external_scripts().to_vec(),
    }
}

/// Renders raw HTML source with lightweight syntax coloring for the Rendered ↔ Raw toggle,
/// expanding `\t` to 4 columns and wrapping losslessly via [`wrap_styled_spans`].
pub fn render_raw_html(raw: &str, width: u16, theme: &HtmlTheme) -> Vec<Line<'static>> {
    let max_width = width.max(12) as usize;
    let mut lines = Vec::new();
    let tag_style = Style::default()
        .fg(theme.raw_tag)
        .add_modifier(Modifier::BOLD);
    let attr_style = Style::default().fg(theme.raw_attr);
    let text_style = Style::default().fg(theme.text);
    let muted_style = Style::default().fg(theme.muted);

    for raw_line in raw.replace("\r\n", "\n").split('\n') {
        if raw_line.is_empty() {
            lines.push(Line::default());
            continue;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut chars = raw_line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '<' {
                let mut buf = String::from("<");
                while let Some(&nc) = chars.peek() {
                    buf.push(nc);
                    chars.next();
                    if nc == '>' {
                        break;
                    }
                }
                if buf.starts_with("<!--") {
                    spans.push(Span::styled(buf, muted_style));
                } else if let Some(ws) = buf.find(char::is_whitespace) {
                    spans.push(Span::styled(buf[..ws].to_string(), tag_style));
                    let rest = &buf[ws..];
                    if let Some(stripped) = rest.strip_suffix('>') {
                        spans.push(Span::styled(stripped.to_string(), attr_style));
                        spans.push(Span::styled(">".to_string(), tag_style));
                    } else {
                        spans.push(Span::styled(rest.to_string(), attr_style));
                    }
                } else {
                    spans.push(Span::styled(buf, tag_style));
                }
            } else {
                let mut buf = String::new();
                buf.push(c);
                while let Some(&nc) = chars.peek() {
                    if nc == '<' {
                        break;
                    }
                    buf.push(nc);
                    chars.next();
                }
                spans.push(Span::styled(buf, text_style));
            }
        }

        lines.extend(wrap_styled_spans(&spans, max_width, WrapPrefix::empty()));
    }

    lines
}

struct RenderCtx<'a> {
    doc: &'a DomDocument,
    width: usize,
    theme: &'a HtmlTheme,
    focused_control: Option<usize>,
    editing_input: Option<DomNodeId>,
    show_link_urls: bool,
    lines: Vec<Line<'static>>,
    controls: Vec<InteractiveControl>,
    graphics: Vec<RenderedGraphic>,
    cur_spans: Vec<Span<'static>>,
    cur_col: usize,
    prefix_stack: Vec<(String, Style)>,
    prefix_emitted: bool,
    #[cfg(feature = "graphics")]
    canvases: Option<&'a HashMap<DomNodeId, CanvasSurface>>,
    #[cfg(feature = "graphics")]
    graphic_cache: Option<&'a mut HashMap<(u64, DomNodeId), RasterizedImage>>,
    #[cfg(feature = "graphics")]
    dom_version: u64,
}

impl<'a> RenderCtx<'a> {
    fn emit_external_scripts_banner(&mut self) {
        let scripts = self.doc.external_scripts();
        if scripts.is_empty() {
            return;
        }
        let mut names: Vec<String> = Vec::new();
        for src in scripts.iter().take(2) {
            let short = src
                .split(['?', '#'])
                .next()
                .unwrap_or(src)
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(src);
            names.push(short.to_string());
        }
        if scripts.len() > 2 {
            names.push(format!("+{} more", scripts.len() - 2));
        }
        let banner = format!(
            "ℹ External script(s) skipped ({}) · Press 'o' for full Browser view",
            names.join(", ")
        );
        let style = Style::default()
            .fg(self.theme.accent)
            .add_modifier(Modifier::ITALIC);
        let wrapped = wrap_styled_spans(&[Span::styled(banner, style)], self.width, WrapPrefix::empty());
        self.lines.extend(wrapped);
        self.lines.push(Line::default());
    }

    fn trim_trailing_empty_lines(&mut self) {
        let min_len = self
            .graphics
            .iter()
            .map(|g| g.line_start + g.line_count)
            .max()
            .unwrap_or(0);
        while self.lines.len() > min_len
            && self
                .lines
                .last()
                .is_some_and(|line| line.spans.iter().all(|s| s.content.trim().is_empty()))
        {
            self.lines.pop();
        }
    }

    fn prefix_spans(&self) -> Vec<Span<'static>> {
        self.prefix_stack
            .iter()
            .map(|(text, style)| Span::styled(text.clone(), *style))
            .collect()
    }

    fn prefix_width(&self) -> usize {
        self.prefix_stack
            .iter()
            .map(|(s, _)| UnicodeWidthStr::width(s.as_str()))
            .sum()
    }

    fn ensure_prefix(&mut self) {
        if !self.prefix_emitted {
            for (text, style) in &self.prefix_stack {
                let w = UnicodeWidthStr::width(text.as_str());
                self.cur_spans.push(Span::styled(text.clone(), *style));
                self.cur_col += w;
            }
            self.prefix_emitted = true;
        }
    }

    fn flush_line(&mut self, force_empty: bool) {
        let has_content = !self.cur_spans.is_empty();
        if has_content || force_empty {
            let spans = std::mem::take(&mut self.cur_spans);
            self.lines.push(Line::from(spans));
        }
        self.cur_col = 0;
        self.prefix_emitted = false;
    }

    fn ensure_block_break(&mut self, blank_line: bool) {
        if !self.cur_spans.is_empty() {
            self.flush_line(false);
        }
        if blank_line
            && !self.lines.is_empty()
            && self
                .lines
                .last()
                .is_some_and(|l| !l.spans.iter().all(|s| s.content.trim().is_empty()))
        {
            self.lines.push(Line::default());
        }
    }

    fn push_inline_text(&mut self, raw: &str, style: Style) {
        let collapsed = collapse_html_ws(raw);
        if collapsed.is_empty() {
            return;
        }

        let mut iter = collapsed.chars().peekable();
        while iter.peek().is_some() {
            let is_ws = iter.peek().is_some_and(|c| c.is_whitespace());
            let mut token = String::new();
            while let Some(&c) = iter.peek() {
                if c.is_whitespace() == is_ws {
                    token.push(c);
                    iter.next();
                } else {
                    break;
                }
            }

            if is_ws {
                if !self.prefix_emitted || self.cur_col <= self.prefix_width() {
                    continue;
                }
                if self.cur_col < self.width {
                    self.cur_spans.push(Span::styled(" ".to_string(), style));
                    self.cur_col += 1;
                }
                continue;
            }

            self.ensure_prefix();
            let tok_w = UnicodeWidthStr::width(token.as_str());
            if self.cur_col + tok_w <= self.width {
                self.cur_spans.push(Span::styled(token, style));
                self.cur_col += tok_w;
            } else if self.prefix_width() + tok_w <= self.width && self.cur_col > self.prefix_width()
            {
                self.flush_line(false);
                self.ensure_prefix();
                self.cur_spans.push(Span::styled(token, style));
                self.cur_col += tok_w;
            } else {
                // Lossless wrap for tokens wider than line budget via `wrap_styled_spans`.
                if self.cur_col > self.prefix_width() {
                    self.flush_line(false);
                }
                let pfx = self.prefix_spans();
                let wrapped = wrap_styled_spans(
                    &[Span::styled(token, style)],
                    self.width,
                    WrapPrefix {
                        first: &pfx,
                        cont: &pfx,
                    },
                );
                let mut iter_lines = wrapped.into_iter().peekable();
                while let Some(line) = iter_lines.next() {
                    if iter_lines.peek().is_some() {
                        self.lines.push(line);
                    } else {
                        self.cur_col = line.width();
                        self.cur_spans = line.spans;
                        self.prefix_emitted = true;
                    }
                }
            }
        }
    }

    fn emit_control(
        &mut self,
        node_id: DomNodeId,
        kind: ControlKind,
        label: String,
        display_text: String,
        base_style: Style,
    ) {
        let idx = self.controls.len();
        let is_focused = self.focused_control == Some(idx);
        let style = if is_focused {
            if self.theme.focus_bg == Color::Reset && self.theme.focus_fg == Color::Reset {
                base_style.add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
                    .fg(self.theme.focus_fg)
                    .bg(self.theme.focus_bg)
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            base_style
        };

        let w = UnicodeWidthStr::width(display_text.as_str());
        self.ensure_prefix();
        if self.cur_col > self.prefix_width() && self.cur_col + w > self.width {
            self.flush_line(false);
            self.ensure_prefix();
        }

        if self.cur_col + w <= self.width {
            let line = self.lines.len();
            let col_start = self.cur_col as u16;
            let col_end = (self.cur_col + w).min(self.width) as u16;

            self.cur_spans.push(Span::styled(display_text, style));
            self.cur_col += w;

            self.controls.push(InteractiveControl {
                index: idx,
                node_id,
                kind,
                label,
                line,
                col_start,
                col_end,
            });
            return;
        }

        // Control text itself exceeds available line width: wrap losslessly across multiple lines
        // while keeping `col_end <= self.width` and preserving exact hit coordinates.
        if self.cur_col > self.prefix_width() {
            self.flush_line(false);
        }
        let pfx = self.prefix_spans();
        let pfx_w = self.prefix_width().min(self.width.saturating_sub(2));
        let wrapped = wrap_styled_spans(
            &[Span::styled(display_text, style)],
            self.width,
            WrapPrefix {
                first: &pfx,
                cont: &pfx,
            },
        );
        let first_line_idx = self.lines.len();
        let first_line_w = wrapped
            .first()
            .map(Line::width)
            .unwrap_or(pfx_w)
            .min(self.width);
        let col_start = pfx_w.min(first_line_w) as u16;
        let col_end = first_line_w as u16;

        let mut iter_lines = wrapped.into_iter().peekable();
        while let Some(line) = iter_lines.next() {
            if iter_lines.peek().is_some() {
                self.lines.push(line);
            } else {
                self.cur_col = line.width();
                self.cur_spans = line.spans;
                self.prefix_emitted = true;
            }
        }

        self.controls.push(InteractiveControl {
            index: idx,
            node_id,
            kind,
            label,
            line: first_line_idx,
            col_start,
            col_end,
        });
    }

    #[cfg(feature = "graphics")]
    fn emit_graphic_slot(
        &mut self,
        node_id: DomNodeId,
        kind: GraphicKind,
        alt: Option<String>,
        width_px: u32,
        height_px: u32,
        image_bytes: Arc<[u8]>,
    ) {
        self.ensure_block_break(false);
        let line_start = self.lines.len();
        let avail_cols = self.width.saturating_sub(self.prefix_width()).max(8);
        let line_count = compute_graphic_rows(width_px, height_px, avail_cols);
        for _ in 0..line_count {
            self.lines.push(Line::default());
        }
        self.graphics.push(RenderedGraphic {
            node_id,
            kind,
            alt,
            line_start,
            line_count,
            width_px,
            height_px,
            image_bytes,
        });
    }

    fn render_children(&mut self, parent: DomNodeId, style: Style) {
        let Some(node) = self.doc.node(parent) else {
            return;
        };
        for &child in &node.children {
            self.render_node(child, style);
        }
    }

    fn render_node(&mut self, id: DomNodeId, inherited_style: Style) {
        let Some(node) = self.doc.node(id) else {
            return;
        };
        match &node.kind {
            DomNodeKind::Document => self.render_children(id, inherited_style),
            DomNodeKind::Text(t) => self.push_inline_text(t, inherited_style),
            DomNodeKind::Element(el) => self.render_element(id, el, inherited_style),
        }
    }

    fn render_element(&mut self, id: DomNodeId, el: &ElementData, inherited_style: Style) {
        // Skip non-visual, metadata, and hidden elements.
        if matches!(
            el.tag.as_str(),
            "script"
                | "style"
                | "noscript"
                | "template"
                | "head"
                | "meta"
                | "link"
                | "title"
                | "xml"
                | "o:p"
                | "metadata"
                | "desc"
                | "ac:parameter"
                | "ac:plain-text-link-body"
        ) || self.doc.is_effectively_hidden(id)
        {
            return;
        }

        let mut style = inherited_style;
        if let Some(color_str) = self.doc.effective_color(id) {
            if let Some(c) = parse_css_color(&color_str) {
                if self.theme.text != Color::Reset {
                    style = style.fg(c);
                }
            }
        }

        match el.tag.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.ensure_block_break(true);
                let level: usize = el.tag[1..].parse().unwrap_or(1);
                let heading_style = Style::default()
                    .fg(self.theme.heading)
                    .add_modifier(Modifier::BOLD);
                let prefix = match level {
                    1 => "",
                    2 => "## ",
                    3 => "### ",
                    _ => "#### ",
                };
                if !prefix.is_empty() {
                    self.push_inline_text(prefix, heading_style);
                }
                self.render_children(id, heading_style);
                self.flush_line(false);
                if level == 1 {
                    let rule_w = self.width.saturating_sub(self.prefix_width()).min(48);
                    self.ensure_prefix();
                    self.cur_spans.push(Span::styled(
                        "═".repeat(rule_w),
                        Style::default().fg(self.theme.border),
                    ));
                    self.flush_line(false);
                }
                self.ensure_block_break(true);
            }
            "p" => {
                self.ensure_block_break(true);
                self.render_children(id, style);
                self.ensure_block_break(true);
            }
            "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "form"
            | "aside" | "figure" | "figcaption" => {
                self.ensure_block_break(false);
                self.render_children(id, style);
                self.ensure_block_break(false);
            }
            "br" => {
                self.flush_line(true);
            }
            "hr" => {
                self.ensure_block_break(true);
                let rule_w = self.width.saturating_sub(self.prefix_width()).max(4);
                self.ensure_prefix();
                self.cur_spans.push(Span::styled(
                    "─".repeat(rule_w),
                    Style::default().fg(self.theme.border),
                ));
                self.flush_line(false);
                self.ensure_block_break(true);
            }
            "strong" | "b" => {
                self.render_children(id, style.add_modifier(Modifier::BOLD));
            }
            "em" | "i" => {
                self.render_children(id, style.add_modifier(Modifier::ITALIC));
            }
            "u" => {
                self.render_children(id, style.add_modifier(Modifier::UNDERLINED));
            }
            "s" | "del" | "strike" => {
                self.render_children(id, style.add_modifier(Modifier::CROSSED_OUT));
            }
            "code" | "kbd" | "samp" => {
                let code_style = Style::default()
                    .fg(self.theme.code_fg)
                    .bg(self.theme.code_bg);
                self.render_children(id, code_style);
            }
            "pre" => {
                self.ensure_block_break(true);
                let raw = self.doc.visible_text_content(id);
                let code_style = Style::default()
                    .fg(self.theme.code_fg)
                    .bg(self.theme.code_bg);
                let mut pre_prefix = self.prefix_spans();
                pre_prefix.push(Span::styled(
                    "  │ ".to_string(),
                    Style::default().fg(self.theme.border),
                ));
                let trimmed = raw.replace("\r\n", "\n");
                let body = trimmed.trim_matches('\n');
                let wrapped = wrap_styled_spans(
                    &[Span::styled(body.to_string(), code_style)],
                    self.width,
                    WrapPrefix {
                        first: &pre_prefix,
                        cont: &pre_prefix,
                    },
                );
                self.lines.extend(wrapped);
                self.ensure_block_break(true);
            }
            "blockquote" => {
                self.ensure_block_break(true);
                self.prefix_stack.push((
                    "│ ".to_string(),
                    Style::default().fg(self.theme.quote_border),
                ));
                self.render_children(id, style.add_modifier(Modifier::ITALIC));
                self.flush_line(false);
                self.prefix_stack.pop();
                self.ensure_block_break(true);
            }
            "ul" => {
                self.ensure_block_break(false);
                let children = self.doc.node(id).map(|n| n.children.clone()).unwrap_or_default();
                for child in children {
                    if self.doc.element(child).is_some_and(|e| e.tag == "li") {
                        self.render_list_item(child, "• ".to_string(), style);
                    } else {
                        self.render_node(child, style);
                    }
                }
                self.ensure_block_break(true);
            }
            "ol" => {
                self.ensure_block_break(false);
                let children = self.doc.node(id).map(|n| n.children.clone()).unwrap_or_default();
                let mut num = 1usize;
                for child in children {
                    if self.doc.element(child).is_some_and(|e| e.tag == "li") {
                        self.render_list_item(child, format!("{num}. "), style);
                        num += 1;
                    } else {
                        self.render_node(child, style);
                    }
                }
                self.ensure_block_break(true);
            }
            "li" => {
                self.render_list_item(id, "• ".to_string(), style);
            }
            "table" => {
                if self.should_linearize_table(id) {
                    self.ensure_block_break(false);
                    self.render_children(id, style);
                    self.ensure_block_break(false);
                } else {
                    self.render_table(id, style);
                }
            }
            "svg" => {
                self.render_svg_element(id, el);
            }
            "canvas" => {
                self.render_canvas_element(id, el);
            }
            "img" => {
                self.render_img_element(id, el);
            }
            "a" => {
                let href = el.get_attr("href").unwrap_or("").to_string();
                let raw_label = self.doc.visible_text_content(id);
                let label = collapse_html_ws(&raw_label).trim().to_string();
                let shown = if label.is_empty() {
                    if href.is_empty() {
                        "link".to_string()
                    } else {
                        href.clone()
                    }
                } else {
                    label
                };
                if href.is_empty() {
                    self.render_children(id, style);
                } else {
                    let display = format!("{shown} ↗");
                    let link_style = Style::default()
                        .fg(self.theme.link)
                        .add_modifier(Modifier::UNDERLINED);
                    self.emit_control(
                        id,
                        ControlKind::Link { href: href.clone() },
                        shown,
                        display,
                        link_style,
                    );
                    if self.show_link_urls {
                        self.push_inline_text(
                            &format!(" ({href})"),
                            Style::default().fg(self.theme.muted),
                        );
                    }
                }
            }
            "button" => {
                let raw_label = self.doc.visible_text_content(id);
                let label = collapse_html_ws(&raw_label).trim().to_string();
                let label = if label.is_empty() {
                    "Button".to_string()
                } else {
                    label
                };
                let display = format!("[ {label} ]");
                let btn_style = Style::default()
                    .fg(self.theme.control_fg)
                    .bg(self.theme.control_bg)
                    .add_modifier(Modifier::BOLD);
                self.emit_control(id, ControlKind::Button, label, display, btn_style);
            }
            "details" => {
                self.ensure_block_break(false);
                let open = el.open;
                let children = self.doc.node(id).map(|n| n.children.clone()).unwrap_or_default();
                let summary_id = children
                    .iter()
                    .copied()
                    .find(|&c| self.doc.element(c).is_some_and(|e| e.tag == "summary"));

                let (ctrl_id, summary_label) = if let Some(sid) = summary_id {
                    let lbl = collapse_html_ws(&self.doc.visible_text_content(sid))
                        .trim()
                        .to_string();
                    (
                        sid,
                        if lbl.is_empty() {
                            "Details".to_string()
                        } else {
                            lbl
                        },
                    )
                } else {
                    (id, "Details".to_string())
                };

                let marker = if open { "▾" } else { "▸" };
                let display = format!("{marker} {summary_label}");
                let sum_style = Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD);
                self.emit_control(
                    ctrl_id,
                    ControlKind::DetailsSummary { open },
                    summary_label,
                    display,
                    sum_style,
                );
                self.flush_line(false);

                if open {
                    self.prefix_stack.push(("  ".to_string(), style));
                    for child in children {
                        if Some(child) != summary_id {
                            self.render_node(child, style);
                        }
                    }
                    self.flush_line(false);
                    self.prefix_stack.pop();
                }
                self.ensure_block_break(false);
            }
            "input" => {
                self.render_input(id, el);
            }
            "textarea" => {
                self.render_textarea(id, el);
            }
            "select" => {
                let options = self.doc.select_options(id);
                let current_label = options
                    .get(el.selected_index)
                    .map(|(_, l)| l.clone())
                    .or_else(|| options.first().map(|(_, l)| l.clone()))
                    .unwrap_or_else(|| "Select".to_string());
                let display = format!("[▾ {current_label}]");
                let sel_style = Style::default()
                    .fg(self.theme.control_fg)
                    .bg(self.theme.control_bg);
                self.emit_control(
                    id,
                    ControlKind::Select {
                        current_label: current_label.clone(),
                    },
                    current_label,
                    display,
                    sel_style,
                );
            }
            "label" => {
                self.render_children(id, style);
                self.push_inline_text(" ", style);
            }
            _ => {
                self.render_children(id, style);
            }
        }
    }

    fn render_svg_element(&mut self, id: DomNodeId, el: &ElementData) {
        let alt = el
            .get_attr("aria-label")
            .or_else(|| el.get_attr("title"))
            .map(ToString::to_string);
        #[cfg(feature = "graphics")]
        {
            let cached = self
                .graphic_cache
                .as_deref()
                .and_then(|c| c.get(&(self.dom_version, id)).cloned());
            let rasterized = if let Some(hit) = cached {
                Ok(hit)
            } else {
                let svg_markup = self.doc.serialize_outer_html(id);
                let max_px_w = ((self.width as u32) * 12).clamp(256, 1600);
                let res = rasterize_svg_to_png(svg_markup.as_bytes(), max_px_w, 1200, None);
                if let (Ok(ref img), Some(cache)) = (&res, self.graphic_cache.as_deref_mut()) {
                    cache.insert((self.dom_version, id), img.clone());
                }
                res
            };

            match rasterized {
                Ok(img) => {
                    self.emit_graphic_slot(
                        id,
                        GraphicKind::InlineSvg,
                        alt,
                        img.width_px,
                        img.height_px,
                        img.png_bytes,
                    );
                }
                Err(_) => {
                    self.push_inline_text(
                        "[◇ SVG unavailable · Press 'o' for Browser]",
                        Style::default().fg(self.theme.muted),
                    );
                }
            }
        }
        #[cfg(not(feature = "graphics"))]
        {
            let _ = (id, alt);
            self.push_inline_text(
                "[◇ SVG graphic · Press 'o' for Browser]",
                Style::default().fg(self.theme.muted),
            );
        }
    }

    fn render_canvas_element(&mut self, id: DomNodeId, el: &ElementData) {
        let alt = el
            .get_attr("aria-label")
            .or_else(|| el.get_attr("id"))
            .map(ToString::to_string);
        #[cfg(feature = "graphics")]
        {
            if let Some(surface) = self.canvases.and_then(|m| m.get(&id)) {
                if surface.has_drawn {
                    let cached = self
                        .graphic_cache
                        .as_deref()
                        .and_then(|c| c.get(&(self.dom_version, id)).cloned());
                    let rasterized = if let Some(hit) = cached {
                        Ok(hit)
                    } else {
                        surface.encode_png().map(|png| {
                            let img = RasterizedImage {
                                png_bytes: Arc::from(png),
                                width_px: surface.width(),
                                height_px: surface.height(),
                            };
                            if let Some(cache) = self.graphic_cache.as_deref_mut() {
                                cache.insert((self.dom_version, id), img.clone());
                            }
                            img
                        })
                    };
                    if let Ok(img) = rasterized {
                        self.emit_graphic_slot(
                            id,
                            GraphicKind::Canvas2d,
                            alt,
                            img.width_px,
                            img.height_px,
                            img.png_bytes,
                        );
                        return;
                    }
                }
            }
            let msg = if !self.doc.external_scripts().is_empty() {
                "[▣ Canvas (external script) · Press 'o' for Browser]"
            } else {
                "[▣ Empty canvas]"
            };
            self.push_inline_text(msg, Style::default().fg(self.theme.muted));
        }
        #[cfg(not(feature = "graphics"))]
        {
            let _ = (id, alt);
            let msg = if !self.doc.external_scripts().is_empty() {
                "[▣ Canvas (external script) · Press 'o' for Browser]"
            } else {
                "[▣ Canvas · Press 'o' for Browser]"
            };
            self.push_inline_text(msg, Style::default().fg(self.theme.muted));
        }
    }

    fn render_img_element(&mut self, id: DomNodeId, el: &ElementData) {
        let alt_str = el.get_attr("alt").unwrap_or("").trim();
        let src = el.get_attr("src").unwrap_or("").trim();

        #[cfg(feature = "graphics")]
        if src.len() >= 11 && src[..11].eq_ignore_ascii_case("data:image/") {
            let alt = (!alt_str.is_empty()).then(|| alt_str.to_string());
            let cached = self
                .graphic_cache
                .as_deref()
                .and_then(|c| c.get(&(self.dom_version, id)).cloned());
            if let Some(img) = cached {
                self.emit_graphic_slot(
                    id,
                    GraphicKind::DataImage,
                    alt,
                    img.width_px,
                    img.height_px,
                    img.png_bytes,
                );
                return;
            }
            if let Ok(decoded) = decode_data_image_uri(src) {
                if decoded.is_svg {
                    let max_px_w = ((self.width as u32) * 12).clamp(256, 1600);
                    if let Ok(img) = rasterize_svg_to_png(&decoded.bytes, max_px_w, 1200, None) {
                        if let Some(cache) = self.graphic_cache.as_deref_mut() {
                            cache.insert((self.dom_version, id), img.clone());
                        }
                        self.emit_graphic_slot(
                            id,
                            GraphicKind::DataImage,
                            alt,
                            img.width_px,
                            img.height_px,
                            img.png_bytes,
                        );
                        return;
                    }
                } else {
                    let (w, h) = probe_raster_dimensions(&decoded.bytes).unwrap_or((300, 150));
                    let bytes: Arc<[u8]> = Arc::from(decoded.bytes);
                    if let Some(cache) = self.graphic_cache.as_deref_mut() {
                        cache.insert(
                            (self.dom_version, id),
                            RasterizedImage {
                                png_bytes: Arc::clone(&bytes),
                                width_px: w,
                                height_px: h,
                            },
                        );
                    }
                    self.emit_graphic_slot(id, GraphicKind::DataImage, alt, w, h, bytes);
                    return;
                }
            }
        }

        #[cfg(not(feature = "graphics"))]
        let _ = (id, src);

        let txt = if alt_str.is_empty() {
            "[🖼 image]".to_string()
        } else {
            format!("[🖼 {alt_str}]")
        };
        self.push_inline_text(&txt, Style::default().fg(self.theme.muted));
    }

    fn render_textarea(&mut self, id: DomNodeId, el: &ElementData) {
        self.ensure_block_break(false);
        let value = el.value.clone();
        let editing = self.editing_input == Some(id);
        let idx = self.controls.len();
        let is_focused = self.focused_control == Some(idx);

        let base_style = Style::default()
            .fg(self.theme.control_fg)
            .bg(self.theme.control_bg);
        let style = if is_focused {
            if self.theme.focus_bg == Color::Reset && self.theme.focus_fg == Color::Reset {
                base_style.add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
                    .fg(self.theme.focus_fg)
                    .bg(self.theme.focus_bg)
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            base_style
        };

        let raw_text = if !value.is_empty() {
            if editing {
                format!("{value}▏")
            } else {
                value.clone()
            }
        } else if editing {
            "▏".to_string()
        } else if let Some(ph) = el.get_attr("placeholder").filter(|p| !p.trim().is_empty()) {
            ph.trim().to_string()
        } else {
            String::new()
        };

        let pfx = self.prefix_spans();
        let pfx_w = self.prefix_width();
        let inner_max = self.width.saturating_sub(pfx_w + 4).max(4);
        let wrapped = if raw_text.is_empty() {
            vec![Line::default()]
        } else {
            wrap_styled_spans(&[Span::styled(raw_text, style)], inner_max, WrapPrefix::empty())
        };
        let box_inner_w = wrapped
            .iter()
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .max(20.min(inner_max))
            .min(inner_max);

        let first_line_idx = self.lines.len();
        let col_start = pfx_w.min(self.width) as u16;
        let col_end = (pfx_w + box_inner_w + 4).min(self.width) as u16;

        for line in wrapped {
            let mut row_spans = pfx.clone();
            let content_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let cw = UnicodeWidthStr::width(content_text.as_str());
            let pad = box_inner_w.saturating_sub(cw);
            let framed = format!("[ {content_text}{} ]", " ".repeat(pad));
            row_spans.push(Span::styled(framed, style));
            self.lines.push(Line::from(row_spans));
        }

        self.controls.push(InteractiveControl {
            index: idx,
            node_id: id,
            kind: ControlKind::TextInput { value },
            label: el.get_attr("name").unwrap_or("textarea").to_string(),
            line: first_line_idx,
            col_start,
            col_end,
        });
    }

    fn render_input(&mut self, id: DomNodeId, el: &ElementData) {
        let t = el.input_type().to_ascii_lowercase();
        match t.as_str() {
            "hidden" => {}
            "button" | "submit" | "reset" => {
                let label = if !el.value.trim().is_empty() {
                    el.value.trim().to_string()
                } else if t == "submit" {
                    "Submit".to_string()
                } else if t == "reset" {
                    "Reset".to_string()
                } else {
                    "Button".to_string()
                };
                let display = format!("[ {label} ]");
                let btn_style = Style::default()
                    .fg(self.theme.control_fg)
                    .bg(self.theme.control_bg)
                    .add_modifier(Modifier::BOLD);
                self.emit_control(id, ControlKind::Button, label, display, btn_style);
            }
            "checkbox" => {
                let checked = el.checked;
                let display = if checked { "[x]" } else { "[ ]" }.to_string();
                let style = Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD);
                self.emit_control(
                    id,
                    ControlKind::Checkbox { checked },
                    el.get_attr("name").unwrap_or("checkbox").to_string(),
                    display,
                    style,
                );
            }
            "radio" => {
                let checked = el.checked;
                let display = if checked { "(●)" } else { "( )" }.to_string();
                let style = Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD);
                self.emit_control(
                    id,
                    ControlKind::Radio { checked },
                    el.get_attr("name").unwrap_or("radio").to_string(),
                    display,
                    style,
                );
            }
            _ => {
                let value = el.value.clone();
                let display_val = if t == "password" {
                    "•".repeat(value.chars().count())
                } else {
                    value.clone()
                };
                let editing = self.editing_input == Some(id);
                let avail_inner = self.width.saturating_sub(self.prefix_width() + 4).max(4);
                let shown = format_input_box(
                    &display_val,
                    el.get_attr("placeholder"),
                    editing,
                    14.min(avail_inner),
                    avail_inner,
                );
                let input_style = Style::default()
                    .fg(self.theme.control_fg)
                    .bg(self.theme.control_bg);
                self.emit_control(
                    id,
                    ControlKind::TextInput { value },
                    el.get_attr("name").unwrap_or("input").to_string(),
                    shown,
                    input_style,
                );
            }
        }
    }

    fn render_list_item(&mut self, li_id: DomNodeId, bullet: String, style: Style) {
        self.ensure_block_break(false);
        self.ensure_prefix();
        let bullet_w = UnicodeWidthStr::width(bullet.as_str());
        self.cur_spans.push(Span::styled(
            bullet,
            Style::default().fg(self.theme.accent),
        ));
        self.cur_col += bullet_w;
        self.prefix_stack.push((" ".repeat(bullet_w), style));
        self.render_children(li_id, style);
        self.flush_line(false);
        self.prefix_stack.pop();
    }

    /// Detects layout tables (`role="presentation"|"none"`, nested `<table>`, `max_cols <= 1`,
    /// or `!has_th_or_thead && has_block_td`) ported from Outlook's HTML parser so layout tables
    /// linearize cleanly instead of rendering as broken grids.
    fn should_linearize_table(&self, table_id: DomNodeId) -> bool {
        if self.has_presentation_role(table_id) || self.has_nested_table(table_id) {
            return true;
        }
        let Some(table_node) = self.doc.node(table_id) else {
            return false;
        };

        let mut max_cols = 0usize;
        let mut has_th_or_thead = false;
        let mut has_block_td = false;

        for &child_id in &table_node.children {
            let Some(child_el) = self.doc.element(child_id) else {
                continue;
            };
            if self.doc.is_effectively_hidden(child_id) {
                continue;
            }
            if self.has_presentation_role(child_id) {
                return true;
            }
            match child_el.tag.as_str() {
                "thead" | "tbody" | "tfoot" => {
                    if child_el.tag == "thead" {
                        has_th_or_thead = true;
                    }
                    if let Some(sec_node) = self.doc.node(child_id) {
                        for &row_id in &sec_node.children {
                            if self.doc.element(row_id).is_some_and(|e| e.tag == "tr") {
                                if self.has_presentation_role(row_id) {
                                    return true;
                                }
                                max_cols = max_cols.max(self.count_row_columns(row_id));
                                self.inspect_row_cells(
                                    row_id,
                                    &mut has_th_or_thead,
                                    &mut has_block_td,
                                );
                            }
                        }
                    }
                }
                "tr" => {
                    max_cols = max_cols.max(self.count_row_columns(child_id));
                    self.inspect_row_cells(child_id, &mut has_th_or_thead, &mut has_block_td);
                }
                _ => {}
            }
        }

        max_cols <= 1 || (!has_th_or_thead && has_block_td)
    }

    fn has_presentation_role(&self, id: DomNodeId) -> bool {
        self.doc.get_attr(id, "role").is_some_and(|r| {
            let t = r.trim();
            t.eq_ignore_ascii_case("presentation") || t.eq_ignore_ascii_case("none")
        })
    }

    fn has_nested_table(&self, table_id: DomNodeId) -> bool {
        let mut stack = self
            .doc
            .node(table_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        while let Some(nid) = stack.pop() {
            if self.doc.element(nid).is_some_and(|e| e.tag == "table") {
                return true;
            }
            if let Some(n) = self.doc.node(nid) {
                stack.extend(&n.children);
            }
        }
        false
    }

    fn count_row_columns(&self, row_id: DomNodeId) -> usize {
        let Some(row_node) = self.doc.node(row_id) else {
            return 0;
        };
        let mut cols = 0usize;
        for &cid in &row_node.children {
            if let Some(el) = self.doc.element(cid) {
                if el.tag == "td" || el.tag == "th" {
                    let span = el
                        .get_attr("colspan")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .filter(|&n| n > 0)
                        .unwrap_or(1);
                    cols += span;
                }
            }
        }
        cols
    }

    fn inspect_row_cells(
        &self,
        row_id: DomNodeId,
        has_th: &mut bool,
        has_block_td: &mut bool,
    ) {
        let Some(row_node) = self.doc.node(row_id) else {
            return;
        };
        for &cid in &row_node.children {
            let Some(el) = self.doc.element(cid) else {
                continue;
            };
            match el.tag.as_str() {
                "th" => *has_th = true,
                "td" => {
                    let mut stack = self
                        .doc
                        .node(cid)
                        .map(|n| n.children.clone())
                        .unwrap_or_default();
                    while let Some(desc) = stack.pop() {
                        if let Some(del) = self.doc.element(desc) {
                            if matches!(
                                del.tag.as_str(),
                                "div"
                                    | "p"
                                    | "ul"
                                    | "ol"
                                    | "blockquote"
                                    | "pre"
                                    | "h1"
                                    | "h2"
                                    | "h3"
                                    | "h4"
                                    | "h5"
                                    | "h6"
                                    | "hr"
                                    | "table"
                            ) {
                                *has_block_td = true;
                                break;
                            }
                        }
                        if let Some(dn) = self.doc.node(desc) {
                            stack.extend(&dn.children);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn render_table(&mut self, table_id: DomNodeId, style: Style) {
        self.ensure_block_break(true);
        let mut rows: Vec<(bool, Vec<Vec<Span<'static>>>)> = Vec::new();
        self.collect_table_rows(table_id, &mut rows);
        if rows.is_empty() {
            return;
        }

        let (headers, body_rows): (Vec<Vec<Span<'static>>>, Vec<Vec<Vec<Span<'static>>>>) =
            if rows.first().is_some_and(|(is_hdr, _)| *is_hdr) {
                let mut iter = rows.into_iter();
                let hdr = iter.next().map(|(_, r)| r).unwrap_or_default();
                let body = iter.map(|(_, r)| r).collect();
                (hdr, body)
            } else {
                (Vec::new(), rows.into_iter().map(|(_, r)| r).collect())
            };

        let prefix = self.prefix_spans();
        let border_style = Style::default().fg(self.theme.border);
        let header_style = Style::default()
            .fg(self.theme.heading)
            .add_modifier(Modifier::BOLD);
        let alignments: Vec<TableAlign> = Vec::new();

        let spec = TableSpec {
            headers: &headers,
            rows: &body_rows,
            alignments: &alignments,
            border: TableBorder::Pipe,
            prefix: &prefix,
            border_style,
            header_style,
            cell_style: style,
        };
        let table_lines = render_wrapped_table(&spec, self.width);
        self.lines.extend(table_lines);
        self.ensure_block_break(true);
    }

    fn collect_table_rows(
        &self,
        node_id: DomNodeId,
        rows: &mut Vec<(bool, Vec<Vec<Span<'static>>>)>,
    ) {
        let Some(node) = self.doc.node(node_id) else {
            return;
        };
        for &child in &node.children {
            let Some(el) = self.doc.element(child) else {
                continue;
            };
            if self.doc.is_effectively_hidden(child) {
                continue;
            }
            match el.tag.as_str() {
                "thead" | "tbody" | "tfoot" => self.collect_table_rows(child, rows),
                "tr" => {
                    let mut cells = Vec::new();
                    let mut all_th = true;
                    if let Some(tr_node) = self.doc.node(child) {
                        for &cell_id in &tr_node.children {
                            let Some(cell_el) = self.doc.element(cell_id) else {
                                continue;
                            };
                            if cell_el.tag == "th" || cell_el.tag == "td" {
                                if cell_el.tag != "th" {
                                    all_th = false;
                                }
                                let txt = collapse_html_ws(&self.doc.visible_text_content(cell_id))
                                    .trim()
                                    .to_string();
                                cells.push(vec![Span::raw(txt)]);
                            }
                        }
                    }
                    if !cells.is_empty() {
                        rows.push((all_th, cells));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(feature = "graphics")]
fn compute_graphic_rows(width_px: u32, height_px: u32, avail_cols: usize) -> usize {
    let w = width_px.max(1) as f32;
    let h = height_px.max(1) as f32;
    let target_cols = (avail_cols as f32).min((w / 8.0).max(8.0));
    let rows = ((h / w) * target_cols * 0.5).round() as usize;
    rows.clamp(3, 24)
}

fn format_input_box(
    value: &str,
    placeholder: Option<&str>,
    editing: bool,
    min_w: usize,
    max_inner_w: usize,
) -> String {
    let max_w = max_inner_w.max(2);
    let inner = if editing {
        let win = windowed_single_line_input(value, value.len(), max_w);
        format!(
            "{}{}▏{}{}",
            if win.clipped_left { "‹" } else { "" },
            win.before_cursor,
            win.after_cursor,
            if win.clipped_right { "›" } else { "" }
        )
    } else {
        let raw = if !value.is_empty() {
            value
        } else if let Some(ph) = placeholder.filter(|p| !p.trim().is_empty()) {
            ph.trim()
        } else {
            ""
        };
        if UnicodeWidthStr::width(raw) <= max_w {
            raw.to_string()
        } else {
            // Window from start (cursor at 0) without cursor glyph so non-editing display never overflows.
            let win = windowed_single_line_input(raw, 0, max_w.saturating_add(1));
            format!(
                "{}{}",
                win.after_cursor,
                if win.clipped_right { "›" } else { "" }
            )
        }
    };
    let w = UnicodeWidthStr::width(inner.as_str());
    let target = min_w.min(max_w);
    let pad = target.saturating_sub(w);
    format!("[ {inner}{} ]", "_".repeat(pad))
}

fn collapse_html_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

pub(crate) fn parse_css_color(raw: &str) -> Option<Color> {
    let s = raw.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        } else if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            return Some(Color::Rgb(r, g, b));
        }
    }
    match s.as_str() {
        "red" | "crimson" => Some(Color::Red),
        "green" | "lime" => Some(Color::Green),
        "blue" | "dodgerblue" => Some(Color::Blue),
        "yellow" | "gold" => Some(Color::Yellow),
        "cyan" | "aqua" => Some(Color::Cyan),
        "magenta" | "purple" => Some(Color::Magenta),
        "white" => Some(Color::White),
        "black" => Some(Color::Black),
        "gray" | "grey" => Some(Color::Gray),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text(r: &RenderedHtml) -> String {
        r.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_headings_tables_and_skips_hidden() {
        let html = r#"
            <h1>Report</h1>
            <p style="display:none">Secret</p>
            <table>
              <tr><th>Name</th><th>Score</th></tr>
              <tr><td>Alpha</td><td>98</td></tr>
            </table>
        "#;
        let doc = DomDocument::parse(html);
        let r = render_dom(&doc, 60, &HtmlTheme::dark(), None, None, false);
        let text = rendered_text(&r);
        assert!(text.contains("Report"));
        assert!(!text.contains("Secret"));
        assert!(text.contains("Name"));
        assert!(text.contains("Alpha"));
        assert!(text.contains("│"));
    }

    #[test]
    fn details_collapsed_vs_expanded() {
        let html = r#"<details id="d"><summary>Info</summary><p>Hidden body</p></details>"#;
        let mut doc = DomDocument::parse(html);
        let collapsed = render_dom(&doc, 60, &HtmlTheme::dark(), None, None, false);
        let collapsed_txt = rendered_text(&collapsed);
        assert!(collapsed_txt.contains("▸ Info"));
        assert!(!collapsed_txt.contains("Hidden body"));

        let d = doc.get_element_by_id("d").unwrap();
        doc.toggle_details(d);
        let expanded = render_dom(&doc, 60, &HtmlTheme::dark(), None, None, false);
        let expanded_txt = rendered_text(&expanded);
        assert!(expanded_txt.contains("▾ Info"));
        assert!(expanded_txt.contains("Hidden body"));
    }

    #[test]
    fn control_bounding_boxes_match_exact_columns() {
        let html = r#"<p>Press <button id="go">Launch</button> now</p>"#;
        let doc = DomDocument::parse(html);
        let r = render_dom(&doc, 80, &HtmlTheme::dark(), Some(0), None, false);
        assert_eq!(r.controls.len(), 1);
        let ctrl = &r.controls[0];
        assert_eq!(ctrl.kind, ControlKind::Button);
        assert_eq!(ctrl.label, "Launch");
        // "Press " is 6 cols, "[ Launch ]" is 10 cols -> cols 6..16
        assert_eq!(ctrl.col_start, 6);
        assert_eq!(ctrl.col_end, 16);
    }

    #[test]
    fn long_controls_textarea_pre_and_raw_html_wrap_losslessly() {
        let long_100 = "A".repeat(100);
        let html = format!(
            r#"<p><a href="https://example.com">{long_100}</a></p>
               <p><button>{long_100}</button></p>
               <details><summary>{long_100}</summary><p>Inner</p></details>
               <textarea>line1
line2
line3</textarea>
               <pre>	tab_indented_very_long_code_identifier_that_must_wrap_cleanly_without_losing_any_chars</pre>"#
        );
        let doc = DomDocument::parse(&html);
        let r = render_dom(&doc, 40, &HtmlTheme::dark(), None, None, false);
        for (i, line) in r.lines.iter().enumerate() {
            assert!(
                line.width() <= 40,
                "line {i} overflowed width 40: {} ({:?})",
                line.width(),
                line
            );
        }
        for ctrl in &r.controls {
            assert!(
                ctrl.col_end <= 40,
                "control {:?} col_end={} > 40",
                ctrl.kind,
                ctrl.col_end
            );
        }
        let txt = rendered_text(&r);
        assert!(txt.contains("line1"));
        assert!(txt.contains("line2"));
        assert!(txt.contains("line3"));
        let l1 = r.lines.iter().position(|l| l.to_string().contains("line1")).unwrap();
        let l2 = r.lines.iter().position(|l| l.to_string().contains("line2")).unwrap();
        let l3 = r.lines.iter().position(|l| l.to_string().contains("line3")).unwrap();
        assert!(l1 < l2 && l2 < l3);
        let pre_no_ws: String = txt
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '│')
            .collect();
        assert!(pre_no_ws.contains("tab_indented_very_long_code_identifier_that_must_wrap_cleanly_without_losing_any_chars"));

        let raw_lines = render_raw_html(&html, 40, &HtmlTheme::dark());
        for (i, line) in raw_lines.iter().enumerate() {
            assert!(
                line.width() <= 40,
                "raw html line {i} overflowed width 40: {}",
                line.width()
            );
        }
    }

    #[test]
    fn zero_source_leak_with_media_script_xml_svg_desc_and_layout_vs_data_table() {
        let html = r#"
            <style>
              @media (max-width: 600px) { .m { color: red; } }
              .hidden { display: none; }
            </style>
            <div class="hidden">ShouldNotAppear</div>
            <table role="presentation">
              <tr><td><p>Layout cell</p><xml><o:p>meta</o:p></xml></td></tr>
            </table>
            <p><a href="https://example.com">VisibleLink<style>.a{color:blue}</style><script>const x=1;</script></a></p>
            <button>VisibleBtn<svg><desc>secret-svg-desc</desc><rect width="20" height="20"/></svg></button>
            <details><summary>VisibleSum<xml>meta-sum</xml></summary><p>Body</p></details>
            <pre>safe_code<script>evil_js()</script></pre>
            <table>
              <thead><tr><th>ColA</th><th>ColB</th></tr></thead>
              <tbody><tr><td>ValA</td><td>ValB</td></tr></tbody>
            </table>
        "#;
        let mut doc = DomDocument::parse(html);
        let r = render_dom(&doc, 80, &HtmlTheme::dark(), None, None, false);
        let text = rendered_text(&r);

        assert!(text.contains("Layout cell"));
        assert!(text.contains("VisibleLink"));
        assert!(text.contains("VisibleBtn"));
        assert!(text.contains("VisibleSum"));
        assert!(text.contains("safe_code"));
        assert!(text.contains("ColA"));
        assert!(text.contains("│"));
        assert!(text.contains("─┼─"));

        // Zero source code / metadata / hidden leak assertions
        assert!(!text.contains("ShouldNotAppear"));
        assert!(!text.contains("color: red"));
        assert!(!text.contains("const x=1"));
        assert!(!text.contains("evil_js"));
        assert!(!text.contains("meta"));
        assert!(!text.contains("secret-svg-desc"));

        // `set_inner_html` with comment-only fragment must not leak `<!--`
        let body = doc.query_selector("body").unwrap();
        doc.set_inner_html(body, "<!-- comment only -->");
        assert!(!doc.visible_text_content(body).contains("<!--"));
        let r2 = render_dom(&doc, 80, &HtmlTheme::dark(), None, None, false);
        assert!(!rendered_text(&r2).contains("<!--"));
    }
}
