//! Ratatui interactive HTML renderer with exact hit-map coordinates.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::dom::{DomDocument, DomNodeId, DomNodeKind, ElementData};

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

/// Complete rendered output for a document at a given viewport width.
#[derive(Debug, Clone)]
pub struct RenderedHtml {
    pub title: Option<String>,
    pub lines: Vec<Line<'static>>,
    pub controls: Vec<InteractiveControl>,
}

/// Renders a [`DomDocument`] into wrapped Ratatui lines and clickable control coordinates.
pub fn render_dom(
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
        cur_spans: Vec::new(),
        cur_col: 0,
        prefix_stack: Vec::new(),
        prefix_emitted: false,
    };

    let base_style = Style::default().fg(theme.text);
    ctx.render_children(doc.root(), base_style);
    ctx.flush_line(false);

    // Trim trailing empty lines.
    while ctx
        .lines
        .last()
        .is_some_and(|line| line.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        ctx.lines.pop();
    }

    RenderedHtml {
        title: doc.title(),
        lines: ctx.lines,
        controls: ctx.controls,
    }
}

/// Renders raw HTML source with lightweight syntax coloring for the Rendered ↔ Raw toggle.
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
        // Tokenize tags vs text on each line, then wrap cleanly to `max_width`.
        let mut tokens: Vec<(String, Style)> = Vec::new();
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
                    tokens.push((buf, muted_style));
                } else {
                    // Split tag name and attributes
                    if let Some(ws) = buf.find(char::is_whitespace) {
                        tokens.push((buf[..ws].to_string(), tag_style));
                        let rest = &buf[ws..];
                        if let Some(stripped) = rest.strip_suffix('>') {
                            tokens.push((stripped.to_string(), attr_style));
                            tokens.push((">".to_string(), tag_style));
                        } else {
                            tokens.push((rest.to_string(), attr_style));
                        }
                    } else {
                        tokens.push((buf, tag_style));
                    }
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
                tokens.push((buf, text_style));
            }
        }

        // Wrap tokens to max_width
        let mut cur_spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0usize;
        for (text, style) in tokens {
            let mut seg = String::new();
            for ch in text.chars() {
                let w = ch.width().unwrap_or(1);
                if col + w > max_width && col > 0 {
                    if !seg.is_empty() {
                        cur_spans.push(Span::styled(std::mem::take(&mut seg), style));
                    }
                    lines.push(Line::from(std::mem::take(&mut cur_spans)));
                    col = 0;
                }
                seg.push(ch);
                col += w;
            }
            if !seg.is_empty() {
                cur_spans.push(Span::styled(seg, style));
            }
        }
        lines.push(Line::from(cur_spans));
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
    cur_spans: Vec<Span<'static>>,
    cur_col: usize,
    prefix_stack: Vec<(String, Style)>,
    prefix_emitted: bool,
}

impl<'a> RenderCtx<'a> {
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

        // Split into alternating whitespace and non-whitespace tokens.
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
                // Don't emit leading whitespace at the start of a line (after prefix).
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
                // Word wider than line: split by character width.
                if self.cur_col > self.prefix_width() && self.cur_col + 1 > self.width {
                    self.flush_line(false);
                    self.ensure_prefix();
                }
                let mut seg = String::new();
                for ch in token.chars() {
                    let cw = ch.width().unwrap_or(1);
                    if self.cur_col + cw > self.width && self.cur_col > self.prefix_width() {
                        if !seg.is_empty() {
                            self.cur_spans
                                .push(Span::styled(std::mem::take(&mut seg), style));
                        }
                        self.flush_line(false);
                        self.ensure_prefix();
                    }
                    seg.push(ch);
                    self.cur_col += cw;
                }
                if !seg.is_empty() {
                    self.cur_spans.push(Span::styled(seg, style));
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

        let line = self.lines.len();
        let col_start = self.cur_col as u16;
        let col_end = (self.cur_col + w) as u16;

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
        // Skip non-visual and hidden elements.
        if matches!(
            el.tag.as_str(),
            "script" | "style" | "noscript" | "template" | "head" | "meta" | "link" | "title"
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
                let raw = self.doc.text_content(id);
                let code_style = Style::default()
                    .fg(self.theme.code_fg)
                    .bg(self.theme.code_bg);
                self.prefix_stack.push((
                    "  │ ".to_string(),
                    Style::default().fg(self.theme.border),
                ));
                for line in raw.replace("\r\n", "\n").trim_matches('\n').split('\n') {
                    self.ensure_prefix();
                    let avail = self.width.saturating_sub(self.prefix_width()).max(4);
                    let mut seg = String::new();
                    let mut seg_w = 0usize;
                    for ch in line.chars() {
                        let cw = ch.width().unwrap_or(1);
                        if seg_w + cw > avail && seg_w > 0 {
                            self.cur_spans
                                .push(Span::styled(std::mem::take(&mut seg), code_style));
                            self.flush_line(false);
                            self.ensure_prefix();
                            seg_w = 0;
                        }
                        seg.push(ch);
                        seg_w += cw;
                    }
                    self.cur_spans.push(Span::styled(seg, code_style));
                    self.flush_line(true);
                }
                self.prefix_stack.pop();
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
                self.render_table(id, style);
            }
            "img" => {
                let alt = el.get_attr("alt").unwrap_or("").trim();
                let txt = if alt.is_empty() {
                    "[🖼 image]".to_string()
                } else {
                    format!("[🖼 {alt}]")
                };
                self.push_inline_text(&txt, Style::default().fg(self.theme.muted));
            }
            "a" => {
                let href = el.get_attr("href").unwrap_or("").to_string();
                let raw_label = self.doc.text_content(id);
                let label = collapse_html_ws(&raw_label).trim().to_string();
                let shown = if label.is_empty() {
                    if href.is_empty() {
                        "link".to_string()
                    } else {
                        href.clone()
                    }
                } else {
                    label.clone()
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
                let raw_label = self.doc.text_content(id);
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
                    let lbl = collapse_html_ws(&self.doc.text_content(sid))
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
                let value = el.value.clone();
                let editing = self.editing_input == Some(id);
                let shown = format_input_box(&value, el.get_attr("placeholder"), editing, 20);
                let input_style = Style::default()
                    .fg(self.theme.control_fg)
                    .bg(self.theme.control_bg);
                self.emit_control(
                    id,
                    ControlKind::TextInput { value },
                    el.get_attr("name").unwrap_or("textarea").to_string(),
                    shown,
                    input_style,
                );
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
                let shown = format_input_box(&display_val, el.get_attr("placeholder"), editing, 14);
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

    fn render_table(&mut self, table_id: DomNodeId, style: Style) {
        self.ensure_block_break(true);
        let mut rows: Vec<(bool, Vec<String>)> = Vec::new();
        self.collect_table_rows(table_id, &mut rows);
        if rows.is_empty() {
            return;
        }

        let num_cols = rows.iter().map(|(_, r)| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return;
        }

        let mut col_widths = vec![1usize; num_cols];
        for (_, row) in &rows {
            for (i, cell) in row.iter().enumerate() {
                col_widths[i] = col_widths[i].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }

        let avail = self.width.saturating_sub(self.prefix_width());
        let sep_total = (num_cols.saturating_sub(1)) * 3;
        let max_cells_w = avail.saturating_sub(sep_total).max(num_cols * 3);
        let sum_w: usize = col_widths.iter().sum();
        if sum_w > max_cells_w {
            for w in &mut col_widths {
                *w = ((*w * max_cells_w) / sum_w).max(3);
            }
        }

        let border_style = Style::default().fg(self.theme.border);
        let header_style = Style::default()
            .fg(self.theme.heading)
            .add_modifier(Modifier::BOLD);

        for (row_idx, (is_header, row)) in rows.iter().enumerate() {
            self.ensure_prefix();
            let cell_style = if *is_header { header_style } else { style };
            for (c_idx, &col_w) in col_widths.iter().enumerate() {
                if c_idx > 0 {
                    self.cur_spans
                        .push(Span::styled(" │ ".to_string(), border_style));
                }
                let raw = row.get(c_idx).map(String::as_str).unwrap_or("");
                let fitted = pad_or_truncate(raw, col_w);
                self.cur_spans.push(Span::styled(fitted, cell_style));
            }
            self.flush_line(false);

            if *is_header && row_idx == 0 {
                self.ensure_prefix();
                for (c_idx, &col_w) in col_widths.iter().enumerate() {
                    if c_idx > 0 {
                        self.cur_spans
                            .push(Span::styled("─┼─".to_string(), border_style));
                    }
                    self.cur_spans
                        .push(Span::styled("─".repeat(col_w), border_style));
                }
                self.flush_line(false);
            }
        }
        self.ensure_block_break(true);
    }

    fn collect_table_rows(&self, node_id: DomNodeId, rows: &mut Vec<(bool, Vec<String>)>) {
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
                                let txt = collapse_html_ws(&self.doc.text_content(cell_id))
                                    .trim()
                                    .to_string();
                                cells.push(txt);
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

fn format_input_box(value: &str, placeholder: Option<&str>, editing: bool, min_w: usize) -> String {
    let inner = if !value.is_empty() {
        if editing {
            format!("{value}▏")
        } else {
            value.to_string()
        }
    } else if editing {
        "▏".to_string()
    } else if let Some(ph) = placeholder.filter(|p| !p.trim().is_empty()) {
        ph.trim().to_string()
    } else {
        String::new()
    };
    let w = UnicodeWidthStr::width(inner.as_str());
    let pad = min_w.saturating_sub(w);
    format!("[ {inner}{} ]", "_".repeat(pad))
}

fn pad_or_truncate(s: &str, target_w: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w <= target_w {
        format!("{s}{}", " ".repeat(target_w - w))
    } else {
        let mut out = String::new();
        let mut cur = 0usize;
        let limit = target_w.saturating_sub(1);
        for ch in s.chars() {
            let cw = ch.width().unwrap_or(1);
            if cur + cw > limit {
                break;
            }
            out.push(ch);
            cur += cw;
        }
        out.push('…');
        let used = cur + 1;
        if used < target_w {
            out.push_str(&" ".repeat(target_w - used));
        }
        out
    }
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

fn parse_css_color(raw: &str) -> Option<Color> {
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
}
