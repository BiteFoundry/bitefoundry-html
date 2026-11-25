//! Unified CommonMark + GFM + Math + Footnotes + YAML Frontmatter Markdown renderer
//! built on `pulldown-cmark`, `scraper`, `HtmlTheme`, and `layout` primitives.

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, MetadataBlockKind, Options,
    Parser, Tag, TagEnd, TextMergeStream,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use scraper::{Html, Node};
use unicode_width::UnicodeWidthStr;

use crate::dom::{DomDocument, DomNodeId};
use crate::layout::{
    render_wrapped_table, wrap_styled_spans, TableAlign, TableBorder, TableSpec, WrapPrefix,
};
use crate::render::{
    parse_css_color, render_dom, ControlKind, HtmlTheme, InteractiveControl, RenderedGraphic,
    RenderedHtml,
};

const MAX_RENDERED_LINES: usize = 2_000;

/// Full CommonMark + GFM + YAML frontmatter + Footnotes + Math parser options.
pub fn markdown_parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
}

/// Convenience helper returning rendered Markdown lines directly for apps that only need `Vec<Line>`.
pub fn render_markdown(
    raw_md: &str,
    width: u16,
    theme: &HtmlTheme,
    show_link_urls: bool,
) -> Vec<Line<'static>> {
    render_markdown_document(raw_md, width as usize, theme, show_link_urls).lines
}

/// Renders CommonMark + GFM Markdown into a complete [`RenderedHtml`] with interactive link
/// controls, wrapped multiline tables, YAML metadata strip, multiline math boxes, footnotes,
/// and sanitized inline/block HTML.
pub fn render_markdown_document(
    raw_md: &str,
    width: usize,
    theme: &HtmlTheme,
    show_link_urls: bool,
) -> RenderedHtml {
    render_markdown_document_with_focus(raw_md, width, theme, None, show_link_urls)
}

pub(crate) fn render_markdown_document_with_focus(
    raw_md: &str,
    width: usize,
    theme: &HtmlTheme,
    focused_control: Option<usize>,
    show_link_urls: bool,
) -> RenderedHtml {
    let mut sink = MdSink::new(width, theme, focused_control, show_link_urls);
    render_markdown_into(raw_md, &mut sink);
    sink.finish()
}

/// Renders raw Markdown source with lightweight syntax highlighting for the Rendered ↔ Raw toggle.
pub fn render_raw_markdown(raw_md: &str, width: usize, theme: &HtmlTheme) -> RenderedHtml {
    let max_width = width.max(12);
    let mut lines = Vec::new();
    let heading_style = Style::default()
        .fg(theme.heading)
        .add_modifier(Modifier::BOLD);
    let fence_style = Style::default().fg(theme.raw_tag);
    let code_style = Style::default().fg(theme.code_fg).bg(theme.code_bg);
    let quote_style = Style::default()
        .fg(theme.quote_border)
        .add_modifier(Modifier::ITALIC);
    let muted_style = Style::default().fg(theme.muted);
    let text_style = Style::default().fg(theme.text);

    let mut in_fence = false;
    for raw_line in raw_md.replace("\r\n", "\n").split('\n') {
        if raw_line.is_empty() {
            lines.push(Line::default());
            continue;
        }
        let trimmed = raw_line.trim_start();
        let style = if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            fence_style
        } else if in_fence {
            code_style
        } else if trimmed.starts_with('#') {
            heading_style
        } else if trimmed.starts_with('>') {
            quote_style
        } else if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            muted_style
        } else {
            text_style
        };

        lines.extend(wrap_styled_spans(
            &[Span::styled(raw_line.to_string(), style)],
            max_width,
            WrapPrefix::empty(),
        ));
    }

    RenderedHtml {
        title: None,
        lines,
        controls: Vec::new(),
        graphics: Vec::new(),
        external_scripts: Vec::new(),
    }
}

/// Compiles Markdown to a standalone UTF-8 HTML5 document suitable for external browser viewing.
pub fn markdown_to_html_document(title: Option<&str>, raw_md: &str) -> String {
    let options = markdown_parser_options();
    let mut frontmatter_lines: Vec<String> = Vec::new();
    let mut in_meta = false;
    let mut meta_buf = String::new();
    let mut first_heading_title: Option<String> = None;
    let mut in_h1 = false;
    let mut h1_buf = String::new();

    let mut events = Vec::new();
    for ev in Parser::new_ext(raw_md, options) {
        match ev {
            Event::Start(Tag::MetadataBlock(MetadataBlockKind::YamlStyle)) => {
                in_meta = true;
                meta_buf.clear();
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                in_meta = false;
                for line in meta_buf.lines() {
                    let t = line.trim();
                    if !t.is_empty() {
                        frontmatter_lines.push(t.to_string());
                    }
                }
            }
            Event::Text(ref t) if in_meta => {
                meta_buf.push_str(t);
            }
            other => {
                if let Event::Start(Tag::Heading {
                    level: HeadingLevel::H1,
                    ..
                }) = &other
                {
                    in_h1 = true;
                    h1_buf.clear();
                } else if let Event::End(TagEnd::Heading(HeadingLevel::H1)) = &other {
                    in_h1 = false;
                    if first_heading_title.is_none() && !h1_buf.trim().is_empty() {
                        first_heading_title = Some(h1_buf.trim().to_string());
                    }
                } else if in_h1 {
                    if let Event::Text(ref t) | Event::Code(ref t) = other {
                        h1_buf.push_str(t);
                    }
                }
                events.push(other);
            }
        }
    }

    let mut body_html = String::new();
    if !frontmatter_lines.is_empty() {
        body_html.push_str("<aside class=\"frontmatter\"><dl>");
        for line in &frontmatter_lines {
            if let Some((k, v)) = line.split_once(':') {
                body_html.push_str("<dt>");
                push_html_escaped(k.trim(), &mut body_html);
                body_html.push_str("</dt><dd>");
                push_html_escaped(v.trim(), &mut body_html);
                body_html.push_str("</dd>");
            }
        }
        body_html.push_str("</dl></aside>\n");
    }

    pulldown_cmark::html::push_html(&mut body_html, events.into_iter());

    let fm_title = frontmatter_lines.iter().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case("title") {
            let val = v.trim().trim_matches('"').trim_matches('\'');
            (!val.is_empty()).then(|| val.to_string())
        } else {
            None
        }
    });
    let resolved_title = title
        .filter(|t| !t.trim().is_empty())
        .map(ToString::to_string)
        .or(fm_title)
        .or(first_heading_title)
        .unwrap_or_else(|| "Markdown Document".to_string());

    let mut esc_title = String::new();
    push_html_escaped(&resolved_title, &mut esc_title);

    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{esc_title}</title>\n<style>body{{font-family:system-ui,-apple-system,sans-serif;max-width:860px;margin:2rem auto;padding:0 1.25rem;line-height:1.6}}pre,code{{font-family:ui-monospace,monospace}}pre{{padding:0.9rem;overflow-x:auto;border-radius:6px;background:#f4f6f8}}table{{border-collapse:collapse;width:100%}}th,td{{border:1px solid #d0d7de;padding:0.4rem 0.7rem}}blockquote{{border-left:4px solid #d0d7de;margin:0;padding:0.2rem 1rem;color:#57606a}}.frontmatter{{background:#f6f8fa;border:1px solid #d0d7de;border-radius:6px;padding:0.6rem 1rem;margin-bottom:1.2rem}}.frontmatter dl{{display:grid;grid-template-columns:auto 1fr;gap:0.25rem 0.75rem;margin:0}}.frontmatter dt{{font-weight:600}}.frontmatter dd{{margin:0}}</style>\n</head>\n<body>\n{body_html}\n</body>\n</html>"
    )
}

fn push_html_escaped(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal Sink & Structured Table / Link / Footnote State
// ---------------------------------------------------------------------------

struct PendingLinkSpan {
    label: String,
    href: String,
}

#[derive(Default)]
struct TableCellData {
    spans: Vec<Span<'static>>,
}

#[derive(Default)]
struct TableData {
    headers: Vec<Vec<Span<'static>>>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    alignments: Vec<TableAlign>,
}

struct MdSink<'a> {
    lines: Vec<Line<'static>>,
    controls: Vec<InteractiveControl>,
    graphics: Vec<RenderedGraphic>,
    title: Option<String>,
    max_width: usize,
    theme: &'a HtmlTheme,
    focused_control: Option<usize>,
    show_link_urls: bool,
    truncated: bool,
}

impl<'a> MdSink<'a> {
    fn new(
        width: usize,
        theme: &'a HtmlTheme,
        focused_control: Option<usize>,
        show_link_urls: bool,
    ) -> Self {
        Self {
            lines: Vec::new(),
            controls: Vec::new(),
            graphics: Vec::new(),
            title: None,
            max_width: width.max(12),
            theme,
            focused_control,
            show_link_urls,
            truncated: false,
        }
    }

    fn no_color(&self) -> bool {
        self.theme.text == Color::Reset && self.theme.accent == Color::Reset
    }

    fn body_style(&self) -> Style {
        if self.no_color() {
            Style::default()
        } else {
            Style::default().fg(self.theme.text)
        }
    }

    fn muted_style(&self) -> Style {
        if self.no_color() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.theme.muted)
        }
    }

    fn border_style(&self) -> Style {
        if self.no_color() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.theme.border)
        }
    }

    fn code_style(&self) -> Style {
        if self.no_color() {
            Style::default()
        } else {
            Style::default()
                .fg(self.theme.code_fg)
                .bg(self.theme.code_bg)
        }
    }

    fn link_style(&self) -> Style {
        if self.no_color() {
            Style::default().add_modifier(Modifier::UNDERLINED)
        } else {
            Style::default()
                .fg(self.theme.link)
                .add_modifier(Modifier::UNDERLINED)
        }
    }

    fn focused_style(&self, base: Style) -> Style {
        if self.theme.focus_bg == Color::Reset && self.theme.focus_fg == Color::Reset {
            base.add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.theme.focus_fg)
                .bg(self.theme.focus_bg)
                .add_modifier(Modifier::BOLD)
        }
    }

    fn quote_bar_style(&self) -> Style {
        if self.no_color() {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.theme.quote_border)
        }
    }

    fn quote_text_style(&self) -> Style {
        if self.no_color() {
            Style::default().add_modifier(Modifier::ITALIC)
        } else {
            Style::default()
                .fg(self.theme.muted)
                .add_modifier(Modifier::ITALIC)
        }
    }

    fn heading_style(&self, level: u8) -> Style {
        if self.no_color() {
            return match level {
                1 => Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                2 | 3 => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default().add_modifier(Modifier::BOLD | Modifier::DIM),
            };
        }
        match level {
            1 => Style::default()
                .fg(self.theme.heading)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            2 => Style::default()
                .fg(self.theme.heading)
                .add_modifier(Modifier::BOLD),
            3 => Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(self.theme.text)
                .add_modifier(Modifier::BOLD),
        }
    }

    fn heading_prefix(&self, level: u8) -> Span<'static> {
        let (glyph, style) = match level {
            1 => (
                "▌ ",
                if self.no_color() {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD)
                },
            ),
            2 => (
                "▎ ",
                if self.no_color() {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(self.theme.heading)
                        .add_modifier(Modifier::BOLD)
                },
            ),
            3 => (
                "▸ ",
                if self.no_color() {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.accent)
                },
            ),
            _ => ("▪ ", self.muted_style()),
        };
        Span::styled(glyph, style)
    }

    fn inline_code_span(&self, text: &str) -> Span<'static> {
        if self.no_color() {
            Span::styled(format!("`{text}`"), self.code_style())
        } else {
            Span::styled(format!(" {text} "), self.code_style())
        }
    }

    fn checkbox_span(&self, checked: bool) -> Span<'static> {
        if self.no_color() {
            if checked {
                Span::styled("[x] ", Style::default().add_modifier(Modifier::BOLD))
            } else {
                Span::styled("[ ] ", Style::default().add_modifier(Modifier::DIM))
            }
        } else if checked {
            Span::styled("☑ ", Style::default().fg(self.theme.accent))
        } else {
            Span::styled("☐ ", self.muted_style())
        }
    }

    fn push_line(&mut self, line: Line<'static>) {
        if self.lines.len() >= MAX_RENDERED_LINES {
            self.truncated = true;
            return;
        }
        self.lines.push(line);
    }

    fn push_blank_line(&mut self) {
        if self.lines.is_empty() || self.truncated {
            return;
        }
        let last_empty = self
            .lines
            .last()
            .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()));
        if !last_empty {
            self.push_line(Line::default());
        }
    }

    fn quote_prefix_spans(&self, quote_depth: usize) -> Vec<Span<'static>> {
        let mut out = Vec::with_capacity(quote_depth);
        for _ in 0..quote_depth {
            out.push(Span::styled("│ ", self.quote_bar_style()));
        }
        out
    }

    fn push_rule(&mut self, quote_depth: usize) {
        self.push_blank_line();
        let mut spans = self.quote_prefix_spans(quote_depth);
        let used_w: usize = spans.iter().map(|s| s.content.width()).sum();
        let rule_w = self.max_width.saturating_sub(used_w).clamp(4, 60);
        spans.push(Span::styled("─".repeat(rule_w), self.border_style()));
        self.push_line(Line::from(spans));
        self.push_blank_line();
    }

    fn push_metadata_strip(&mut self, yaml: &str) {
        let entries: Vec<(String, String)> = yaml
            .replace("\r\n", "\n")
            .lines()
            .filter_map(|line| {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    return None;
                }
                let (k, v) = t.split_once(':')?;
                let key = k.trim().to_string();
                let val = v.trim().to_string();
                if key.is_empty() {
                    None
                } else {
                    Some((key, val))
                }
            })
            .collect();

        if entries.is_empty() {
            return;
        }

        if self.title.is_none() {
            if let Some((_, val)) = entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("title"))
            {
                let cleaned = val.trim_matches('"').trim_matches('\'').trim();
                if !cleaned.is_empty() {
                    self.title = Some(cleaned.to_string());
                }
            }
        }

        let bar = Span::styled("┄ ", self.border_style());
        let key_style = Style::default()
            .fg(self.theme.accent)
            .add_modifier(Modifier::BOLD);
        let val_style = self.muted_style();

        let mut strip_spans: Vec<Span<'static>> = Vec::new();
        for (idx, (k, v)) in entries.iter().enumerate() {
            if idx > 0 {
                strip_spans.push(Span::styled("  ·  ", self.border_style()));
            }
            strip_spans.push(Span::styled(format!("{k}: "), key_style));
            strip_spans.push(Span::styled(v.clone(), val_style));
        }

        let wrapped = wrap_styled_spans(
            &strip_spans,
            self.max_width,
            WrapPrefix {
                first: std::slice::from_ref(&bar),
                cont: std::slice::from_ref(&bar),
            },
        );
        for line in wrapped {
            self.push_line(line);
        }
        self.push_blank_line();
    }

    fn push_boxed_block(&mut self, label: &str, body: &str, quote_depth: usize) {
        self.push_blank_line();
        let mut header_spans = self.quote_prefix_spans(quote_depth);
        header_spans.push(Span::styled(format!("┌─ {label}"), self.muted_style()));
        self.push_line(Line::from(header_spans));

        let trimmed = body.replace("\r\n", "\n");
        let trimmed = trimmed.trim_matches('\n');
        let code_style = self.code_style();
        let mut pfx = self.quote_prefix_spans(quote_depth);
        pfx.push(Span::styled("│ ", self.border_style()));

        if trimmed.is_empty() {
            self.push_line(Line::from(pfx));
        } else {
            let wrapped = wrap_styled_spans(
                &[Span::styled(trimmed.to_string(), code_style)],
                self.max_width,
                WrapPrefix {
                    first: &pfx,
                    cont: &pfx,
                },
            );
            for line in wrapped {
                self.push_line(line);
                if self.truncated {
                    return;
                }
            }
        }

        let mut footer_spans = self.quote_prefix_spans(quote_depth);
        footer_spans.push(Span::styled("└─", self.muted_style()));
        self.push_line(Line::from(footer_spans));
        self.push_blank_line();
    }

    fn push_table(&mut self, table: &TableData, quote_depth: usize) {
        let num_cols = table
            .headers
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if num_cols == 0 {
            return;
        }
        self.push_blank_line();

        let prefix = self.quote_prefix_spans(quote_depth);
        let header_style = if self.no_color() {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.theme.heading)
                .add_modifier(Modifier::BOLD)
        };
        let spec = TableSpec {
            headers: &table.headers,
            rows: &table.rows,
            alignments: &table.alignments,
            border: TableBorder::Pipe,
            prefix: &prefix,
            border_style: self.border_style(),
            header_style,
            cell_style: self.body_style(),
        };
        let rendered = render_wrapped_table(&spec, self.max_width);
        for line in rendered {
            self.push_line(line);
            if self.truncated {
                return;
            }
        }
        self.push_blank_line();
    }

    fn push_html_block(&mut self, raw_html: &str, quote_depth: usize) {
        let trimmed = raw_html.trim();
        if trimmed.is_empty() || (trimmed.starts_with("<!--") && trimmed.ends_with("-->")) {
            return;
        }
        let mut doc = DomDocument::parse(trimmed);
        // Auto-open `<details>` elements in static Markdown previews so inner content is readable.
        for det_id in doc.query_selector_all("details") {
            doc.set_attr(det_id, "open", "");
        }

        let pfx = self.quote_prefix_spans(quote_depth);
        let pfx_w: usize = pfx.iter().map(|s| s.content.width()).sum();
        let avail = self.max_width.saturating_sub(pfx_w).max(12) as u16;
        let rendered = render_dom(&doc, avail, self.theme, None, None, self.show_link_urls);
        if rendered.lines.is_empty() && rendered.graphics.is_empty() {
            return;
        }

        self.push_blank_line();
        let base_line = self.lines.len();
        for mut line in rendered.lines {
            if !pfx.is_empty() {
                let mut prefixed = pfx.clone();
                prefixed.extend(line.spans);
                line = Line::from(prefixed);
            }
            self.push_line(line);
            if self.truncated {
                return;
            }
        }
        for mut ctrl in rendered.controls {
            let idx = self.controls.len();
            ctrl.index = idx;
            ctrl.line += base_line;
            ctrl.col_start = ctrl.col_start.saturating_add(pfx_w as u16);
            ctrl.col_end = ctrl
                .col_end
                .saturating_add(pfx_w as u16)
                .min(self.max_width as u16);
            self.controls.push(ctrl);
        }
        for mut g in rendered.graphics {
            g.line_start += base_line;
            self.graphics.push(g);
        }
        self.push_blank_line();
    }

    fn push_wrapped_spans_with_links(
        &mut self,
        first_prefix: Vec<Span<'static>>,
        cont_prefix: Vec<Span<'static>>,
        spans: Vec<Span<'static>>,
        links: Vec<PendingLinkSpan>,
    ) {
        if spans.is_empty() {
            return;
        }
        let mut wrapped = wrap_styled_spans(
            &spans,
            self.max_width,
            WrapPrefix {
                first: &first_prefix,
                cont: &cont_prefix,
            },
        );
        let base_line = self.lines.len();

        // Register interactive controls for each link emitted in this paragraph and apply focus style.
        if !links.is_empty() {
            let mut scan_line = 0usize;
            let mut scan_span = 0usize;
            for link in links {
                let mut found: Option<(usize, usize, u16, u16)> = None;
                'search: for (li, line) in wrapped.iter().enumerate().skip(scan_line) {
                    let start_si = if li == scan_line { scan_span } else { 0 };
                    let mut col = 0usize;
                    for (si, sp) in line.spans.iter().enumerate() {
                        let sw = UnicodeWidthStr::width(sp.content.as_ref());
                        if si >= start_si && sp.style.add_modifier.contains(Modifier::UNDERLINED) {
                            let c_start = col.min(self.max_width) as u16;
                            let c_end = (col + sw).min(self.max_width) as u16;
                            found = Some((li, si, c_start, c_end));
                            break 'search;
                        }
                        col += sw;
                    }
                }
                if let Some((li, si, col_start, col_end)) = found {
                    let ctrl_idx = self.controls.len();
                    if self.focused_control == Some(ctrl_idx) {
                        let orig_style = wrapped[li].spans[si].style;
                        wrapped[li].spans[si].style = self.focused_style(orig_style);
                    }
                    self.controls.push(InteractiveControl {
                        index: ctrl_idx,
                        node_id: DomNodeId(ctrl_idx),
                        kind: ControlKind::Link { href: link.href },
                        label: link.label,
                        line: base_line + li,
                        col_start,
                        col_end,
                    });
                    scan_line = li;
                    scan_span = si + 1;
                }
            }
        }

        for line in wrapped {
            self.push_line(line);
            if self.truncated {
                return;
            }
        }
    }

    fn finish(mut self) -> RenderedHtml {
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
                .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        {
            self.lines.pop();
        }
        if self.truncated {
            self.lines.push(Line::from(Span::styled(
                "… (preview truncated)",
                self.muted_style(),
            )));
        }
        RenderedHtml {
            title: self.title,
            lines: self.lines,
            controls: self.controls,
            graphics: self.graphics,
            external_scripts: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
enum ListState {
    Unordered,
    Ordered(u64),
}

struct ActiveLink {
    url: String,
    label_buf: String,
}

struct ActiveImage {
    src: String,
    alt_buf: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InlineHtmlFrame {
    Style,
    Link,
    Summary,
}

fn push_or_merge_span(spans: &mut Vec<Span<'static>>, text: String, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.style == style {
            last.content.to_mut().push_str(&text);
            return;
        }
    }
    spans.push(Span::styled(text, style));
}

fn format_link_suffix(label: &str, url: &str, show_link_urls: bool) -> Option<String> {
    let trimmed_url = url.trim();
    if trimmed_url.is_empty()
        || trimmed_url
            .to_ascii_lowercase()
            .starts_with("javascript:")
    {
        return None;
    }
    let trimmed_label = label.trim();
    let same = trimmed_label.eq_ignore_ascii_case(trimmed_url)
        || trimmed_label
            .trim_end_matches('/')
            .eq_ignore_ascii_case(trimmed_url.trim_end_matches('/'));
    if same {
        return None;
    }
    if show_link_urls {
        Some(format!(" ({trimmed_url})"))
    } else {
        Some(" ↗".to_string())
    }
}

fn image_label(alt: &str, src: &str) -> String {
    let trimmed = alt.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let from_src = src
        .trim()
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
    if !from_src.is_empty() {
        from_src.to_string()
    } else {
        "image".to_string()
    }
}

fn heading_level_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn map_alignment(a: Alignment) -> TableAlign {
    match a {
        Alignment::Right => TableAlign::Right,
        Alignment::Center => TableAlign::Center,
        Alignment::None | Alignment::Left => TableAlign::Left,
    }
}

fn render_markdown_into(raw: &str, out: &mut MdSink<'_>) {
    let options = markdown_parser_options();
    let parser = TextMergeStream::new(Parser::new_ext(raw, options));

    let mut style_stack: Vec<Style> = vec![out.body_style()];
    let mut inline_buf: Vec<Span<'static>> = Vec::new();
    let mut pending_links: Vec<PendingLinkSpan> = Vec::new();
    let mut quote_depth: usize = 0;
    let mut list_stack: Vec<ListState> = Vec::new();
    let mut pending_list_marker: Option<(usize, String, Option<bool>)> = None;
    let mut active_heading: Option<u8> = None;
    let mut heading_text_buf = String::new();
    let mut in_code_block: Option<(String, String)> = None;
    let mut in_html_block: Option<String> = None;
    let mut in_metadata_block: Option<String> = None;
    let mut active_link: Option<ActiveLink> = None;
    let mut active_image: Option<ActiveImage> = None;
    let mut inline_html_stack: Vec<(String, InlineHtmlFrame)> = Vec::new();

    // Footnote definitions collected and emitted at bottom of document.
    let mut footnotes: Vec<(String, Vec<Span<'static>>)> = Vec::new();
    let mut active_footnote: Option<(String, Vec<Span<'static>>)> = None;

    // Styled multiline table state
    let mut active_table: Option<TableData> = None;
    let mut in_table_head = false;
    let mut cur_row: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur_cell: Option<TableCellData> = None;

    let flush_inline = |inline_buf: &mut Vec<Span<'static>>,
                        pending_links: &mut Vec<PendingLinkSpan>,
                        out: &mut MdSink<'_>,
                        quote_depth: usize,
                        pending_list_marker: &mut Option<(usize, String, Option<bool>)>,
                        list_depth: usize,
                        active_heading: Option<u8>| {
        if inline_buf.is_empty() && pending_list_marker.is_none() {
            pending_links.clear();
            return;
        }
        let spans = std::mem::take(inline_buf);
        let links = std::mem::take(pending_links);
        let mut first_prefix = out.quote_prefix_spans(quote_depth);
        let mut cont_prefix = out.quote_prefix_spans(quote_depth);

        if let Some(level) = active_heading {
            first_prefix.push(out.heading_prefix(level));
            cont_prefix.push(Span::raw("  "));
        } else if let Some((depth, marker, task)) = pending_list_marker.take() {
            let indent = " ".repeat(depth.saturating_sub(1) * 2);
            if !indent.is_empty() {
                first_prefix.push(Span::raw(indent.clone()));
            }
            let marker_w = if let Some(checked) = task {
                let cb = out.checkbox_span(checked);
                let w = cb.content.width();
                first_prefix.push(cb);
                w
            } else {
                let style = if out.no_color() {
                    Style::default()
                } else {
                    Style::default().fg(out.theme.accent)
                };
                let text = format!("{marker} ");
                let w = text.width();
                first_prefix.push(Span::styled(text, style));
                w
            };
            cont_prefix.push(Span::raw(" ".repeat(indent.len() + marker_w)));
        } else if list_depth > 0 {
            let indent = " ".repeat(list_depth * 2);
            first_prefix.push(Span::raw(indent.clone()));
            cont_prefix.push(Span::raw(indent));
        }

        if spans.is_empty() {
            out.push_line(Line::from(first_prefix));
        } else {
            out.push_wrapped_spans_with_links(first_prefix, cont_prefix, spans, links);
        }
    };

    for event in parser {
        if out.truncated {
            break;
        }

        // Capture YAML frontmatter raw text cleanly without emitting `---` rules.
        if let Some(ref mut meta_buf) = in_metadata_block {
            match event {
                Event::End(TagEnd::MetadataBlock(_)) => {
                    let yaml = in_metadata_block.take().unwrap_or_default();
                    out.push_metadata_strip(&yaml);
                }
                Event::Text(t) => meta_buf.push_str(&t),
                Event::SoftBreak | Event::HardBreak => meta_buf.push('\n'),
                _ => {}
            }
            continue;
        }

        // Collect footnote definitions to emit at the bottom of the document.
        if let Some((_, ref mut fn_spans)) = active_footnote {
            match event {
                Event::End(TagEnd::FootnoteDefinition) => {
                    if let Some(done) = active_footnote.take() {
                        footnotes.push(done);
                    }
                }
                Event::Start(Tag::Strong) => {
                    let top = *style_stack.last().unwrap_or(&out.body_style());
                    style_stack.push(top.add_modifier(Modifier::BOLD));
                }
                Event::Start(Tag::Emphasis) => {
                    let top = *style_stack.last().unwrap_or(&out.body_style());
                    style_stack.push(top.add_modifier(Modifier::ITALIC));
                }
                Event::End(TagEnd::Strong | TagEnd::Emphasis) => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                }
                Event::Text(t) => {
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    push_or_merge_span(fn_spans, t.into_string(), style);
                }
                Event::Code(c) => {
                    fn_spans.push(out.inline_code_span(&c));
                }
                Event::SoftBreak => {
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    push_or_merge_span(fn_spans, " ".to_string(), style);
                }
                Event::HardBreak => {
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    push_or_merge_span(fn_spans, "\n".to_string(), style);
                }
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::MetadataBlock(MetadataBlockKind::YamlStyle)
                | Tag::MetadataBlock(MetadataBlockKind::PlusesStyle) => {
                    in_metadata_block = Some(String::new());
                }
                Tag::FootnoteDefinition(label) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    active_footnote = Some((label.into_string(), Vec::new()));
                }
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    out.push_blank_line();
                    let lvl = heading_level_u8(level);
                    active_heading = Some(lvl);
                    heading_text_buf.clear();
                    style_stack.push(out.heading_style(lvl));
                }
                Tag::BlockQuote(kind) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    if quote_depth == 0 {
                        out.push_blank_line();
                    }
                    quote_depth += 1;
                    style_stack.push(out.quote_text_style());
                    if let Some(alert) = kind {
                        let badge = match alert {
                            BlockQuoteKind::Note => "NOTE",
                            BlockQuoteKind::Tip => "TIP",
                            BlockQuoteKind::Important => "IMPORTANT",
                            BlockQuoteKind::Warning => "WARNING",
                            BlockQuoteKind::Caution => "CAUTION",
                        };
                        let mut spans = out.quote_prefix_spans(quote_depth);
                        let badge_style = if out.no_color() {
                            Style::default().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(out.theme.accent)
                                .add_modifier(Modifier::BOLD)
                        };
                        spans.push(Span::styled(format!("[{badge}]"), badge_style));
                        out.push_line(Line::from(spans));
                    }
                }
                Tag::CodeBlock(kind) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    let lang = match kind {
                        CodeBlockKind::Indented => String::new(),
                        CodeBlockKind::Fenced(info) => info.into_string(),
                    };
                    in_code_block = Some((lang, String::new()));
                }
                Tag::HtmlBlock => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    in_html_block = Some(String::new());
                }
                Tag::List(start) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    if list_stack.is_empty() {
                        out.push_blank_line();
                    }
                    let state = match start {
                        Some(n) => ListState::Ordered(n),
                        None => ListState::Unordered,
                    };
                    list_stack.push(state);
                }
                Tag::Item => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    let depth = list_stack.len().max(1);
                    let marker = match list_stack.last_mut() {
                        Some(ListState::Ordered(n)) => {
                            let s = format!("{n}.");
                            *n += 1;
                            s
                        }
                        _ => {
                            let bullets = ["•", "◦", "▪"];
                            bullets[(depth - 1) % bullets.len()].to_string()
                        }
                    };
                    pending_list_marker = Some((depth, marker, None));
                }
                Tag::Table(alignments) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    active_table = Some(TableData {
                        headers: Vec::new(),
                        rows: Vec::new(),
                        alignments: alignments.into_iter().map(map_alignment).collect(),
                    });
                }
                Tag::TableHead => {
                    in_table_head = true;
                    cur_row.clear();
                }
                Tag::TableRow => {
                    cur_row.clear();
                }
                Tag::TableCell => {
                    cur_cell = Some(TableCellData::default());
                }
                Tag::Strong => {
                    let top = *style_stack.last().unwrap_or(&out.body_style());
                    style_stack.push(top.add_modifier(Modifier::BOLD));
                }
                Tag::Emphasis => {
                    let top = *style_stack.last().unwrap_or(&out.body_style());
                    style_stack.push(top.add_modifier(Modifier::ITALIC));
                }
                Tag::Strikethrough => {
                    let top = *style_stack.last().unwrap_or(&out.body_style());
                    style_stack.push(top.add_modifier(Modifier::CROSSED_OUT));
                }
                Tag::Link { dest_url, .. } => {
                    active_link = Some(ActiveLink {
                        url: dest_url.into_string(),
                        label_buf: String::new(),
                    });
                    style_stack.push(out.link_style());
                }
                Tag::Image { dest_url, .. } => {
                    active_image = Some(ActiveImage {
                        src: dest_url.into_string(),
                        alt_buf: String::new(),
                    });
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    if list_stack.is_empty() {
                        out.push_blank_line();
                    }
                }
                TagEnd::Heading(_) => {
                    if active_heading == Some(1)
                        && out.title.is_none()
                        && !heading_text_buf.trim().is_empty()
                    {
                        out.title = Some(heading_text_buf.trim().to_string());
                    }
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    active_heading = None;
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                    out.push_blank_line();
                }
                TagEnd::BlockQuote(_) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    quote_depth = quote_depth.saturating_sub(1);
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                    if quote_depth == 0 {
                        out.push_blank_line();
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, code)) = in_code_block.take() {
                        let lbl = lang.split_whitespace().next().unwrap_or("");
                        let header = if lbl.is_empty() { "code" } else { lbl };
                        out.push_boxed_block(header, &code, quote_depth);
                    }
                }
                TagEnd::HtmlBlock => {
                    if let Some(html) = in_html_block.take() {
                        out.push_html_block(&html, quote_depth);
                    }
                }
                TagEnd::List(_) => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    list_stack.pop();
                    if list_stack.is_empty() {
                        out.push_blank_line();
                    }
                }
                TagEnd::Item => {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                }
                TagEnd::Table => {
                    if let Some(table) = active_table.take() {
                        out.push_table(&table, quote_depth);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(table) = active_table.as_mut() {
                        table.headers = std::mem::take(&mut cur_row);
                    }
                    in_table_head = false;
                }
                TagEnd::TableRow => {
                    if let Some(table) = active_table.as_mut() {
                        let row = std::mem::take(&mut cur_row);
                        if in_table_head {
                            table.headers = row;
                        } else {
                            table.rows.push(row);
                        }
                    }
                }
                TagEnd::TableCell => {
                    if let Some(cell) = cur_cell.take() {
                        cur_row.push(cell.spans);
                    }
                }
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                }
                TagEnd::Link => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                    if let Some(link) = active_link.take() {
                        finalize_link(
                            link,
                            &mut cur_cell,
                            &mut inline_buf,
                            &mut pending_links,
                            out,
                        );
                    }
                }
                TagEnd::Image => {
                    if let Some(img) = active_image.take() {
                        let tag_text = format!("[🖼 {}]", image_label(&img.alt_buf, &img.src));
                        if let Some(ref mut cell) = cur_cell {
                            cell.spans.push(Span::styled(tag_text, out.muted_style()));
                        } else {
                            inline_buf.push(Span::styled(tag_text, out.muted_style()));
                        }
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, ref mut buf)) = in_code_block {
                    buf.push_str(&text);
                } else if let Some(ref mut html_buf) = in_html_block {
                    html_buf.push_str(&text);
                } else if let Some(ref mut img) = active_image {
                    img.alt_buf.push_str(&text);
                } else {
                    if active_heading == Some(1) {
                        heading_text_buf.push_str(&text);
                    }
                    if let Some(ref mut link) = active_link {
                        link.label_buf.push_str(&text);
                    }
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    if let Some(ref mut cell) = cur_cell {
                        push_or_merge_span(&mut cell.spans, text.into_string(), style);
                    } else {
                        push_or_merge_span(&mut inline_buf, text.into_string(), style);
                    }
                }
            }
            Event::Code(code) => {
                if let Some(ref mut img) = active_image {
                    img.alt_buf.push_str(&code);
                } else {
                    if active_heading == Some(1) {
                        heading_text_buf.push_str(&code);
                    }
                    if let Some(ref mut link) = active_link {
                        link.label_buf.push_str(&code);
                    }
                    let span = out.inline_code_span(&code);
                    if let Some(ref mut cell) = cur_cell {
                        cell.spans.push(span);
                    } else {
                        inline_buf.push(span);
                    }
                }
            }
            Event::SoftBreak => {
                if let Some((_, ref mut buf)) = in_code_block {
                    buf.push('\n');
                } else if let Some(ref mut cell) = cur_cell {
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    push_or_merge_span(&mut cell.spans, " ".to_string(), style);
                } else {
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    push_or_merge_span(&mut inline_buf, " ".to_string(), style);
                }
            }
            Event::HardBreak => {
                if let Some(ref mut cell) = cur_cell {
                    let style = *style_stack.last().unwrap_or(&out.body_style());
                    push_or_merge_span(&mut cell.spans, "\n".to_string(), style);
                } else {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                }
            }
            Event::Rule => {
                flush_inline(
                    &mut inline_buf,
                    &mut pending_links,
                    out,
                    quote_depth,
                    &mut pending_list_marker,
                    list_stack.len(),
                    active_heading,
                );
                out.push_rule(quote_depth);
            }
            Event::TaskListMarker(checked) => {
                if let Some((_, _, ref mut task)) = pending_list_marker {
                    *task = Some(checked);
                } else {
                    inline_buf.push(out.checkbox_span(checked));
                }
            }
            Event::Html(html) => {
                if let Some(ref mut html_buf) = in_html_block {
                    html_buf.push_str(&html);
                } else {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    out.push_html_block(&html, quote_depth);
                }
            }
            Event::InlineHtml(html) => {
                let mut flush_cb = |buf: &mut Vec<Span<'static>>,
                                    links: &mut Vec<PendingLinkSpan>,
                                    sink: &mut MdSink<'_>,
                                    marker: &mut Option<(usize, String, Option<bool>)>| {
                    flush_inline(
                        buf,
                        links,
                        sink,
                        quote_depth,
                        marker,
                        list_stack.len(),
                        active_heading,
                    );
                };
                handle_inline_html(
                    &html,
                    &mut style_stack,
                    &mut inline_html_stack,
                    &mut active_link,
                    &mut cur_cell,
                    &mut inline_buf,
                    &mut pending_links,
                    out,
                    quote_depth,
                    &mut pending_list_marker,
                    &mut flush_cb,
                );
            }
            Event::FootnoteReference(name) => {
                let badge = Span::styled(format!("[^{name}]"), out.muted_style());
                if let Some(ref mut cell) = cur_cell {
                    cell.spans.push(badge);
                } else {
                    inline_buf.push(badge);
                }
            }
            Event::InlineMath(math) => {
                let math_style = if out.no_color() {
                    Style::default().add_modifier(Modifier::ITALIC)
                } else {
                    Style::default()
                        .fg(out.theme.code_fg)
                        .bg(out.theme.code_bg)
                        .add_modifier(Modifier::ITALIC)
                };
                let span = Span::styled(math.trim().to_string(), math_style);
                if let Some(ref mut cell) = cur_cell {
                    cell.spans.push(span);
                } else {
                    inline_buf.push(span);
                }
            }
            Event::DisplayMath(math) => {
                if let Some(ref mut cell) = cur_cell {
                    cell.spans.push(out.inline_code_span(math.trim()));
                } else {
                    flush_inline(
                        &mut inline_buf,
                        &mut pending_links,
                        out,
                        quote_depth,
                        &mut pending_list_marker,
                        list_stack.len(),
                        active_heading,
                    );
                    out.push_boxed_block("math", &math, quote_depth);
                }
            }
        }
    }

    flush_inline(
        &mut inline_buf,
        &mut pending_links,
        out,
        quote_depth,
        &mut pending_list_marker,
        list_stack.len(),
        active_heading,
    );

    // Emit collected footnotes at the bottom of the document.
    if !footnotes.is_empty() && !out.truncated {
        out.push_rule(0);
        for (label, spans) in footnotes {
            let pfx_text = format!("[^{label}]: ");
            let pfx_w = UnicodeWidthStr::width(pfx_text.as_str());
            let first = vec![Span::styled(
                pfx_text,
                Style::default()
                    .fg(out.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )];
            let cont = vec![Span::raw(" ".repeat(pfx_w))];
            out.push_wrapped_spans_with_links(first, cont, spans, Vec::new());
            if out.truncated {
                break;
            }
        }
    }
}

fn finalize_link(
    link: ActiveLink,
    cur_cell: &mut Option<TableCellData>,
    inline_buf: &mut Vec<Span<'static>>,
    pending_links: &mut Vec<PendingLinkSpan>,
    out: &MdSink<'_>,
) {
    let label = if link.label_buf.trim().is_empty() {
        link.url.clone()
    } else {
        link.label_buf.trim().to_string()
    };

    if let Some(ref mut cell) = cur_cell {
        if link.label_buf.trim().is_empty() && !link.url.trim().is_empty() {
            cell.spans.push(Span::styled(link.url.clone(), out.link_style()));
        } else if let Some(suffix) = format_link_suffix(&link.label_buf, &link.url, out.show_link_urls) {
            cell.spans.push(Span::styled(suffix, out.muted_style()));
        }
    } else {
        if link.label_buf.trim().is_empty() && !link.url.trim().is_empty() {
            inline_buf.push(Span::styled(link.url.clone(), out.link_style()));
        } else if let Some(suffix) = format_link_suffix(&link.label_buf, &link.url, out.show_link_urls) {
            inline_buf.push(Span::styled(suffix, out.muted_style()));
        }
        if !link.url.trim().is_empty() {
            pending_links.push(PendingLinkSpan {
                label,
                href: link.url,
            });
        }
    }
}

/// Parses `Event::InlineHtml` using `scraper::Html::parse_fragment` so attributes (`href`, `src`,
/// `alt`, `style`) and inline semantic tags (`<a>`, `<img>`, `<details>`, `<summary>`, `<mark>`,
/// `<sub>`, `<sup>`, `<kbd>`, `<span>`, `<br>`) render cleanly without raw HTML leakage.
#[allow(clippy::too_many_arguments)]
fn handle_inline_html<F>(
    raw_html: &str,
    style_stack: &mut Vec<Style>,
    inline_html_stack: &mut Vec<(String, InlineHtmlFrame)>,
    active_link: &mut Option<ActiveLink>,
    cur_cell: &mut Option<TableCellData>,
    inline_buf: &mut Vec<Span<'static>>,
    pending_links: &mut Vec<PendingLinkSpan>,
    out: &mut MdSink<'_>,
    quote_depth: usize,
    pending_list_marker: &mut Option<(usize, String, Option<bool>)>,
    flush_inline: &mut F,
) where
    F: FnMut(
        &mut Vec<Span<'static>>,
        &mut Vec<PendingLinkSpan>,
        &mut MdSink<'_>,
        &mut Option<(usize, String, Option<bool>)>,
    ),
{
    let trimmed = raw_html.trim();
    if trimmed.is_empty() || trimmed.starts_with("<!--") {
        return;
    }

    // Handle closing tag (e.g. `</a>`, `</summary>`, `</span>`, `</mark>`)
    if let Some(after_slash) = trimmed.strip_prefix("</") {
        let close_tag = after_slash
            .trim_end_matches('>')
            .trim()
            .to_ascii_lowercase();
        if let Some(pos) = inline_html_stack
            .iter()
            .rposition(|(t, _)| *t == close_tag)
        {
            let (_, frame) = inline_html_stack.remove(pos);
            match frame {
                InlineHtmlFrame::Style => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                }
                InlineHtmlFrame::Link => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                    if let Some(link) = active_link.take() {
                        finalize_link(link, cur_cell, inline_buf, pending_links, out);
                    }
                }
                InlineHtmlFrame::Summary => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                    if cur_cell.is_none() {
                        flush_inline(inline_buf, pending_links, out, pending_list_marker);
                    }
                }
            }
        } else if close_tag == "details" && cur_cell.is_none() {
            flush_inline(inline_buf, pending_links, out, pending_list_marker);
        }
        return;
    }

    // Parse opening/void/self-contained tag with `scraper::Html::parse_fragment`.
    let frag = Html::parse_fragment(trimmed);
    let mut found_el = None;
    for node_ref in frag.tree.root().descendants() {
        if let Node::Element(el) = node_ref.value() {
            let name = el.name().to_ascii_lowercase();
            if name != "html" && name != "body" && name != "head" {
                found_el = Some(el);
                break;
            }
        }
    }
    let Some(el) = found_el else {
        return;
    };

    let tag = el.name().to_ascii_lowercase();
    let is_self_closing = trimmed.ends_with("/>");

    match tag.as_str() {
        "br" => {
            if let Some(ref mut cell) = cur_cell {
                let style = *style_stack.last().unwrap_or(&out.body_style());
                push_or_merge_span(&mut cell.spans, "\n".to_string(), style);
            } else {
                flush_inline(inline_buf, pending_links, out, pending_list_marker);
            }
        }
        "hr" => {
            if cur_cell.is_none() {
                flush_inline(inline_buf, pending_links, out, pending_list_marker);
                out.push_rule(quote_depth);
            }
        }
        "img" => {
            let alt = el.attr("alt").unwrap_or("");
            let src = el.attr("src").unwrap_or("");
            let label = format!("[🖼 {}]", image_label(alt, src));
            if let Some(ref mut cell) = cur_cell {
                cell.spans.push(Span::styled(label, out.muted_style()));
            } else {
                inline_buf.push(Span::styled(label, out.muted_style()));
            }
        }
        "a" => {
            let href = el.attr("href").unwrap_or("").trim().to_string();
            if !is_self_closing {
                *active_link = Some(ActiveLink {
                    url: href,
                    label_buf: String::new(),
                });
                style_stack.push(out.link_style());
                inline_html_stack.push((tag, InlineHtmlFrame::Link));
            }
        }
        "details" => {
            if cur_cell.is_none() {
                flush_inline(inline_buf, pending_links, out, pending_list_marker);
            }
        }
        "summary" => {
            if cur_cell.is_none() {
                flush_inline(inline_buf, pending_links, out, pending_list_marker);
            }
            let sum_style = Style::default()
                .fg(out.theme.accent)
                .add_modifier(Modifier::BOLD);
            if let Some(ref mut cell) = cur_cell {
                cell.spans.push(Span::styled("▾ ", sum_style));
            } else {
                inline_buf.push(Span::styled("▾ ", sum_style));
            }
            if !is_self_closing {
                style_stack.push(sum_style);
                inline_html_stack.push((tag, InlineHtmlFrame::Summary));
            }
        }
        "b" | "strong" => {
            if !is_self_closing {
                let top = *style_stack.last().unwrap_or(&out.body_style());
                style_stack.push(top.add_modifier(Modifier::BOLD));
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "i" | "em" => {
            if !is_self_closing {
                let top = *style_stack.last().unwrap_or(&out.body_style());
                style_stack.push(top.add_modifier(Modifier::ITALIC));
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "s" | "del" | "strike" => {
            if !is_self_closing {
                let top = *style_stack.last().unwrap_or(&out.body_style());
                style_stack.push(top.add_modifier(Modifier::CROSSED_OUT));
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "u" | "ins" => {
            if !is_self_closing {
                let top = *style_stack.last().unwrap_or(&out.body_style());
                style_stack.push(top.add_modifier(Modifier::UNDERLINED));
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "code" | "kbd" | "samp" => {
            if !is_self_closing {
                style_stack.push(out.code_style());
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "mark" => {
            if !is_self_closing {
                let mark_style = if out.no_color() {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(out.theme.focus_fg)
                        .bg(out.theme.focus_bg)
                };
                style_stack.push(mark_style);
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "sub" | "sup" => {
            if !is_self_closing {
                let top = *style_stack.last().unwrap_or(&out.body_style());
                style_stack.push(top.add_modifier(Modifier::DIM | Modifier::ITALIC));
                inline_html_stack.push((tag, InlineHtmlFrame::Style));
            }
        }
        "span" if !is_self_closing => {
            let mut next_style = *style_stack.last().unwrap_or(&out.body_style());
            if let Some(style_attr) = el.attr("style") {
                for decl in style_attr.split(';') {
                    if let Some((prop, val)) = decl.split_once(':') {
                        if prop.trim().eq_ignore_ascii_case("color") && !out.no_color() {
                            if let Some(c) = parse_css_color(val) {
                                next_style = next_style.fg(c);
                            }
                        }
                    }
                }
            }
            style_stack.push(next_style);
            inline_html_stack.push((tag, InlineHtmlFrame::Style));
        }
        _ => {}
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
    fn markdown_yaml_frontmatter_renders_as_metadata_not_hr() {
        let md = "---\ntitle: Spec\ntags: [a, b]\n---\n# H1\n\nBody text.";
        let r = render_markdown_document(md, 80, &HtmlTheme::dark(), false);
        let text = rendered_text(&r);
        assert_eq!(r.title.as_deref(), Some("Spec"));
        assert!(text.contains("title: Spec"), "missing metadata strip:\n{text}");
        assert!(text.contains("tags: [a, b]"));
        assert!(text.contains("H1"));
        assert!(!text.contains("---"), "raw frontmatter --- leaked:\n{text}");
        assert!(!text.contains("────"), "frontmatter misparsed as HR:\n{text}");
    }

    #[test]
    fn markdown_math_and_footnotes_rendered_cleanly() {
        let md = "Inline $E=mc^2$ and ref[^1].\n\n$$\na + b\n= c\n$$\n\n[^1]: note body here";
        let r = render_markdown_document(md, 80, &HtmlTheme::dark(), false);
        let text = rendered_text(&r);
        assert!(text.contains("E=mc^2"));
        assert!(text.contains("[^1]"));
        assert!(text.contains("note body here"));
        assert!(!text.contains('$'), "raw $ leaked:\n{text}");

        let a_line = r.lines.iter().position(|l| l.to_string().contains("a + b")).unwrap();
        let c_line = r.lines.iter().position(|l| l.to_string().contains("= c")).unwrap();
        assert!(c_line > a_line, "DisplayMath must span multiple lines:\n{text}");
    }

    #[test]
    fn markdown_inline_html_attributes_and_styled_table_cells() {
        let md = r#"See <a href="https://example.com">Link</a> and <img src="a.png" alt="Logo">

<details><summary>More</summary>Inner</details>

| Feature | Notes |
| :--- | :--- |
| **Bold** `code`<br>Line2 | <mark>Marked</mark> |
"#;
        let r = render_markdown_document(md, 80, &HtmlTheme::dark(), false);
        let text = rendered_text(&r);

        assert!(text.contains("Link"));
        assert!(text.contains("[🖼 Logo]"));
        assert!(text.contains("More"));
        assert!(text.contains("Inner"));
        assert!(text.contains("Bold"));
        assert!(text.contains("code"));
        assert!(text.contains("Line2"));
        assert!(text.contains("Marked"));

        // Zero raw HTML tag leakage
        assert!(!text.contains("<a "), "raw <a> leaked:\n{text}");
        assert!(!text.contains("<br"), "raw <br> leaked:\n{text}");
        assert!(!text.contains("<details"), "raw <details> leaked:\n{text}");
        assert!(!text.contains("<mark"), "raw <mark> leaked:\n{text}");

        // Styled table cell preserves BOLD modifier and wraps `<br>` onto second row
        let has_bold_span = r.lines.iter().any(|l| {
            l.spans.iter().any(|s| {
                s.content.contains("Bold") && s.style.add_modifier.contains(Modifier::BOLD)
            })
        });
        assert!(has_bold_span, "table cell lost BOLD modifier");
        let bold_line = r.lines.iter().position(|l| l.to_string().contains("Bold")).unwrap();
        let line2_line = r.lines.iter().position(|l| l.to_string().contains("Line2")).unwrap();
        assert!(line2_line > bold_line, "table cell <br> must split lines");

        // Link registered as interactive control with valid coordinates
        let link_ctrl = r
            .controls
            .iter()
            .find(|c| matches!(&c.kind, ControlKind::Link { href } if href == "https://example.com"))
            .expect("inline HTML <a href> should register InteractiveControl");
        assert!(link_ctrl.col_end > link_ctrl.col_start);
        assert!(link_ctrl.col_end <= 80);
    }
}
