//! High-level drop-in `HtmlViewSession` state for BiteFoundry terminal apps.

#[cfg(feature = "graphics")]
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[cfg(feature = "graphics")]
use crate::canvas::CanvasSurface;
use crate::dom::{DomDocument, DomNodeId};
#[cfg(feature = "graphics")]
use crate::graphics::RasterizedImage;
#[cfg(feature = "js")]
use crate::js::JsSession;
use crate::markdown::{
    markdown_to_html_document, render_markdown_document_with_focus, render_raw_markdown,
};
#[cfg(feature = "graphics")]
use crate::render::render_dom_with_graphics;
#[cfg(not(feature = "graphics"))]
use crate::render::render_dom;
use crate::render::{render_raw_html, ControlKind, HtmlTheme, RenderedHtml};
use crate::sniff::ContentKind;

/// Source document format handled by [`HtmlViewSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Html,
    Markdown,
}

/// Shared mutable DOM + side-effect buffers observed by both the TUI session and QuickJS bridge.
#[derive(Debug)]
pub struct DomSharedState {
    pub doc: DomDocument,
    pub alerts: Vec<String>,
    pub open_urls: Vec<String>,
    pub console_logs: Vec<String>,
    pub dirty: bool,
    #[cfg(feature = "graphics")]
    pub canvases: HashMap<DomNodeId, CanvasSurface>,
}

impl DomSharedState {
    pub fn new(doc: DomDocument) -> Self {
        Self {
            doc,
            alerts: Vec::new(),
            open_urls: Vec::new(),
            console_logs: Vec::new(),
            dirty: false,
            #[cfg(feature = "graphics")]
            canvases: HashMap::new(),
        }
    }
}

/// Side effect produced by interacting with an HTML page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlEffect {
    None,
    DomChanged,
    OpenUrl(String),
    Toast(String),
    FormSubmitted {
        action: String,
        fields: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderCacheKey {
    width: u16,
    raw_mode: bool,
    focused_control: Option<usize>,
    editing_input: Option<DomNodeId>,
    show_link_urls: bool,
    dom_version: u64,
    theme: HtmlTheme,
}

/// High-level interactive HTML viewer session.
///
/// Allocates `0 MB` of JS runtime when given static HTML; lazily spawns QuickJS only when
/// inline `<script>` tags or `on*` event attributes exist, and frees it immediately on `Drop`.
pub struct HtmlViewSession {
    title: Option<String>,
    source_kind: SourceKind,
    raw_html: String,
    shared: Arc<Mutex<DomSharedState>>,
    #[cfg(feature = "js")]
    js: Option<JsSession>,
    pub raw_mode: bool,
    pub scroll_y: u16,
    pub focused_control: Option<usize>,
    pub editing_input: Option<DomNodeId>,
    pub last_status: Option<String>,
    pub js_active: bool,
    dom_version: u64,
    cached_render: Option<RenderedHtml>,
    cache_key: Option<RenderCacheKey>,
    #[cfg(feature = "graphics")]
    graphic_png_cache: HashMap<(u64, DomNodeId), RasterizedImage>,
    pending_init_effects: Vec<HtmlEffect>,
}

impl HtmlViewSession {
    /// Parses `raw_html` and instantiates QuickJS only if scripts or `on*` handlers exist.
    pub fn new(title: Option<String>, raw_html: String) -> Self {
        let doc = DomDocument::parse(&raw_html);
        let resolved_title = title.or_else(|| doc.title());
        let needs_js = doc.has_scripts_or_handlers();
        let shared = Arc::new(Mutex::new(DomSharedState::new(doc)));

        #[cfg(feature = "js")]
        let (js, js_active) = if needs_js {
            (Some(JsSession::new(Arc::clone(&shared))), true)
        } else {
            (None, false)
        };

        #[cfg(not(feature = "js"))]
        let js_active = {
            let _ = needs_js;
            false
        };

        let mut session = Self {
            title: resolved_title,
            source_kind: SourceKind::Html,
            raw_html,
            shared,
            #[cfg(feature = "js")]
            js,
            raw_mode: false,
            scroll_y: 0,
            focused_control: None,
            editing_input: None,
            last_status: None,
            js_active,
            dom_version: 0,
            cached_render: None,
            cache_key: None,
            #[cfg(feature = "graphics")]
            graphic_png_cache: HashMap::new(),
            pending_init_effects: Vec::new(),
        };

        let init_effects = session.drain_shared_effects();
        if let Some(HtmlEffect::Toast(msg)) = init_effects
            .iter()
            .rev()
            .find(|e| matches!(e, HtmlEffect::Toast(_)))
        {
            session.last_status = Some(msg.clone());
        }
        session.pending_init_effects = init_effects;

        #[cfg(feature = "js")]
        if let Some(err) = session.last_js_error().map(ToString::to_string) {
            session.last_status = Some(format!("JS error: {err}"));
        }

        session
    }

    /// Creates a native Markdown viewing session (`0 MB` JS runtime, `js_active = false`).
    pub fn new_markdown(title: Option<String>, raw_md: String) -> Self {
        let shared = Arc::new(Mutex::new(DomSharedState::new(DomDocument::parse(""))));
        Self {
            title,
            source_kind: SourceKind::Markdown,
            raw_html: raw_md,
            shared,
            #[cfg(feature = "js")]
            js: None,
            raw_mode: false,
            scroll_y: 0,
            focused_control: None,
            editing_input: None,
            last_status: None,
            js_active: false,
            dom_version: 0,
            cached_render: None,
            cache_key: None,
            #[cfg(feature = "graphics")]
            graphic_png_cache: HashMap::new(),
            pending_init_effects: Vec::new(),
        }
    }

    /// Creates an HTML or Markdown session based on a classified [`ContentKind`].
    pub fn from_content(title: Option<String>, kind: ContentKind, raw: String) -> Self {
        match kind {
            ContentKind::Markdown => Self::new_markdown(title, raw),
            _ => Self::new(title, raw),
        }
    }

    pub fn source_kind(&self) -> SourceKind {
        self.source_kind
    }

    /// Returns standalone HTML ready to serve in an external browser via `NativeHtmlServer`.
    pub fn serve_html(&self) -> String {
        match self.source_kind {
            SourceKind::Html => self.raw_html.clone(),
            SourceKind::Markdown => {
                markdown_to_html_document(self.title.as_deref(), &self.raw_html)
            }
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn raw_html(&self) -> &str {
        &self.raw_html
    }

    /// Returns any non-fatal JavaScript error captured from page scripts.
    pub fn last_js_error(&self) -> Option<&str> {
        #[cfg(feature = "js")]
        {
            self.js.as_ref().and_then(JsSession::last_js_error)
        }
        #[cfg(not(feature = "js"))]
        {
            None
        }
    }

    /// Returns any console log entries emitted by page scripts.
    pub fn console_logs(&self) -> Vec<String> {
        self.shared
            .lock()
            .map(|s| s.console_logs.clone())
            .unwrap_or_default()
    }

    /// Drains any effects produced during initial `<script>` execution (e.g., startup `alert()`).
    pub fn take_pending_effects(&mut self) -> Vec<HtmlEffect> {
        std::mem::take(&mut self.pending_init_effects)
    }

    /// Renders (or returns cached render of) the document for the given width and theme.
    pub fn render(
        &mut self,
        width: u16,
        theme: &HtmlTheme,
        show_link_urls: bool,
    ) -> &RenderedHtml {
        let key = RenderCacheKey {
            width,
            raw_mode: self.raw_mode,
            focused_control: self.focused_control,
            editing_input: self.editing_input,
            show_link_urls,
            dom_version: self.dom_version,
            theme: theme.clone(),
        };

        let needs_rebuild = self.cached_render.is_none() || self.cache_key.as_ref() != Some(&key);
        if needs_rebuild {
            let rendered = match (self.source_kind, self.raw_mode) {
                (SourceKind::Markdown, true) => {
                    let mut r = render_raw_markdown(&self.raw_html, width as usize, theme);
                    if r.title.is_none() {
                        r.title = self.title.clone();
                    }
                    r
                }
                (SourceKind::Markdown, false) => render_markdown_document_with_focus(
                    &self.raw_html,
                    width as usize,
                    theme,
                    self.focused_control,
                    show_link_urls,
                ),
                (SourceKind::Html, true) => RenderedHtml {
                    title: self.title.clone(),
                    lines: render_raw_html(&self.raw_html, width, theme),
                    controls: Vec::new(),
                    graphics: Vec::new(),
                    external_scripts: Vec::new(),
                },
                (SourceKind::Html, false) => {
                    let doc_guard = self.shared.lock().expect("DOM state lock poisoned");
                    #[cfg(feature = "graphics")]
                    {
                        self.graphic_png_cache
                            .retain(|&(ver, _), _| ver == self.dom_version);
                        render_dom_with_graphics(
                            &doc_guard.doc,
                            width,
                            theme,
                            self.focused_control,
                            self.editing_input,
                            show_link_urls,
                            Some(&doc_guard.canvases),
                            Some(&mut self.graphic_png_cache),
                            self.dom_version,
                        )
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        render_dom(
                            &doc_guard.doc,
                            width,
                            theme,
                            self.focused_control,
                            self.editing_input,
                            show_link_urls,
                        )
                    }
                }
            };

            if let Some(idx) = self.focused_control {
                if rendered.controls.is_empty() {
                    self.focused_control = None;
                } else if idx >= rendered.controls.len() {
                    self.focused_control = Some(rendered.controls.len() - 1);
                }
            }

            if let Some(ref doc_title) = rendered.title {
                if self.title.is_none() {
                    self.title = Some(doc_title.clone());
                }
            }

            self.cached_render = Some(rendered);
            self.cache_key = Some(RenderCacheKey {
                focused_control: self.focused_control,
                ..key
            });
        }

        self.cached_render.as_ref().expect("cached render present")
    }

    /// Toggles between Rendered view and Raw HTML source view.
    pub fn toggle_raw_mode(&mut self) -> bool {
        self.raw_mode = !self.raw_mode;
        self.editing_input = None;
        self.invalidate_cache();
        self.raw_mode
    }

    /// Moves focus to the next interactive control.
    pub fn focus_next_control(&mut self) {
        if self.raw_mode {
            return;
        }
        let count = self.control_count();
        if count == 0 {
            self.focused_control = None;
            return;
        }
        self.focused_control = Some(match self.focused_control {
            None => 0,
            Some(i) => (i + 1) % count,
        });
        self.invalidate_cache();
    }

    /// Moves focus to the previous interactive control.
    pub fn focus_prev_control(&mut self) {
        if self.raw_mode {
            return;
        }
        let count = self.control_count();
        if count == 0 {
            self.focused_control = None;
            return;
        }
        self.focused_control = Some(match self.focused_control {
            None => count - 1,
            Some(0) => count - 1,
            Some(i) => i - 1,
        });
        self.invalidate_cache();
    }

    /// Activates the currently focused interactive control and returns resulting effects.
    pub fn activate_focused(&mut self) -> Vec<HtmlEffect> {
        if self.raw_mode {
            return Vec::new();
        }
        let Some(focus_idx) = self.focused_control else {
            return Vec::new();
        };
        let Some((node_id, kind)) = self
            .cached_render
            .as_ref()
            .and_then(|r| r.controls.get(focus_idx))
            .map(|c| (c.node_id, c.kind.clone()))
        else {
            return Vec::new();
        };

        self.activate_control(node_id, kind)
    }

    /// Finds the interactive control at `(line_index, col)`, focuses it, and activates it.
    pub fn click_at(&mut self, line_index: usize, col: u16) -> Vec<HtmlEffect> {
        if self.raw_mode {
            return Vec::new();
        }
        let hit = self.cached_render.as_ref().and_then(|r| {
            r.controls
                .iter()
                .find(|c| c.line == line_index && col >= c.col_start && col < c.col_end)
                .map(|c| (c.index, c.node_id, c.kind.clone()))
        });

        let Some((idx, node_id, kind)) = hit else {
            if self.editing_input.is_some() {
                return self.finish_editing();
            }
            return Vec::new();
        };

        if self.editing_input.is_some() && self.editing_input != Some(node_id) {
            let _ = self.finish_editing();
        }
        self.focused_control = Some(idx);
        self.invalidate_cache();
        self.activate_control(node_id, kind)
    }

    /// Appends a character to the currently active text input / textarea.
    pub fn input_char(&mut self, ch: char) {
        let Some(node_id) = self.editing_input else {
            return;
        };
        {
            if let Ok(mut st) = self.shared.lock() {
                if let Some(el) = st.doc.element(node_id) {
                    let mut next = el.value.clone();
                    next.push(ch);
                    st.doc.set_input_value(node_id, &next);
                    st.dirty = true;
                }
            }
        }
        #[cfg(feature = "js")]
        if let Some(ref mut js) = self.js {
            js.dispatch_input(node_id);
        }
        let _ = self.drain_shared_effects();
        self.dom_version += 1;
        self.invalidate_cache();
    }

    /// Deletes the last character from the currently active text input / textarea.
    pub fn backspace(&mut self) {
        let Some(node_id) = self.editing_input else {
            return;
        };
        {
            if let Ok(mut st) = self.shared.lock() {
                if let Some(el) = st.doc.element(node_id) {
                    let mut next = el.value.clone();
                    next.pop();
                    st.doc.set_input_value(node_id, &next);
                    st.dirty = true;
                }
            }
        }
        #[cfg(feature = "js")]
        if let Some(ref mut js) = self.js {
            js.dispatch_input(node_id);
        }
        let _ = self.drain_shared_effects();
        self.dom_version += 1;
        self.invalidate_cache();
    }

    /// Exits text input editing mode and fires `change` / `onchange`.
    pub fn finish_editing(&mut self) -> Vec<HtmlEffect> {
        let Some(node_id) = self.editing_input.take() else {
            return Vec::new();
        };
        self.last_status = None;

        #[cfg(feature = "js")]
        if let Some(ref mut js) = self.js {
            js.dispatch_change(node_id);
        }
        #[cfg(not(feature = "js"))]
        let _ = node_id;

        let mut effects = self.drain_shared_effects();
        if !effects.contains(&HtmlEffect::DomChanged) {
            effects.push(HtmlEffect::DomChanged);
        }
        self.dom_version += 1;
        self.invalidate_cache();
        effects
    }

    fn activate_control(&mut self, node_id: DomNodeId, kind: ControlKind) -> Vec<HtmlEffect> {
        let mut effects = Vec::new();
        let mut dom_changed = false;

        match kind {
            ControlKind::Link { href } => {
                let prevented = self.dispatch_js_click(node_id);
                if !prevented && !href.is_empty() {
                    if let Some(js_code) = href.strip_prefix("javascript:") {
                        #[cfg(feature = "js")]
                        if let Some(ref mut js) = self.js {
                            let _ = js.eval_script(js_code);
                        }
                        #[cfg(not(feature = "js"))]
                        let _ = js_code;
                    } else {
                        effects.push(HtmlEffect::OpenUrl(href));
                    }
                }
            }
            ControlKind::Button => {
                let prevented = self.dispatch_js_click(node_id);
                let is_submit = self.shared.lock().ok().is_some_and(|st| {
                    let Some(el) = st.doc.element(node_id) else {
                        return false;
                    };
                    if st.doc.ancestor_tag(node_id, "form").is_none() {
                        return false;
                    }
                    if el.tag == "input" {
                        el.input_type().eq_ignore_ascii_case("submit")
                    } else if el.tag == "button" {
                        let t = el.get_attr("type").unwrap_or("submit");
                        t.eq_ignore_ascii_case("submit")
                    } else {
                        false
                    }
                });

                if !prevented && is_submit {
                    let form_id = self
                        .shared
                        .lock()
                        .ok()
                        .and_then(|st| st.doc.ancestor_tag(node_id, "form"));
                    if let Some(fid) = form_id {
                        let submit_prevented = self.dispatch_js_submit(fid);
                        if !submit_prevented {
                            if let Ok(st) = self.shared.lock() {
                                if let Some((action, fields)) = st.doc.collect_form_data(fid) {
                                    effects.push(HtmlEffect::FormSubmitted { action, fields });
                                }
                            }
                        }
                    }
                }
            }
            ControlKind::DetailsSummary { .. } => {
                if let Ok(mut st) = self.shared.lock() {
                    st.doc.toggle_details(node_id);
                    dom_changed = true;
                }
                let _ = self.dispatch_js_click(node_id);
            }
            ControlKind::Checkbox { .. } | ControlKind::Radio { .. } => {
                if let Ok(mut st) = self.shared.lock() {
                    st.doc.toggle_checkbox_or_radio(node_id);
                    dom_changed = true;
                }
                let _ = self.dispatch_js_click(node_id);
                self.dispatch_js_input_change(node_id);
            }
            ControlKind::Select { .. } => {
                if let Ok(mut st) = self.shared.lock() {
                    if st.doc.cycle_select(node_id).is_some() {
                        dom_changed = true;
                    }
                }
                self.dispatch_js_input_change(node_id);
            }
            ControlKind::TextInput { .. } => {
                self.editing_input = Some(node_id);
                self.last_status = Some("Editing input (Enter to finish)".to_string());
                let _ = self.dispatch_js_click(node_id);
                self.invalidate_cache();
            }
        }

        let shared_effects = self.drain_shared_effects();
        for eff in shared_effects {
            if eff == HtmlEffect::DomChanged {
                dom_changed = true;
            } else {
                if let HtmlEffect::Toast(ref msg) = eff {
                    self.last_status = Some(msg.clone());
                }
                effects.push(eff);
            }
        }

        #[cfg(feature = "js")]
        if let Some(err) = self.last_js_error().map(ToString::to_string) {
            self.last_status = Some(format!("JS error: {err}"));
        }

        if dom_changed {
            self.dom_version += 1;
            self.invalidate_cache();
            effects.push(HtmlEffect::DomChanged);
        }

        effects
    }

    fn dispatch_js_click(&mut self, node_id: DomNodeId) -> bool {
        #[cfg(feature = "js")]
        if let Some(ref mut js) = self.js {
            return js.dispatch_click(node_id);
        }
        #[cfg(not(feature = "js"))]
        let _ = node_id;
        false
    }

    fn dispatch_js_input_change(&mut self, node_id: DomNodeId) {
        #[cfg(feature = "js")]
        if let Some(ref mut js) = self.js {
            js.dispatch_input_change(node_id, None);
        }
        #[cfg(not(feature = "js"))]
        let _ = node_id;
    }

    fn dispatch_js_submit(&mut self, form_id: DomNodeId) -> bool {
        #[cfg(feature = "js")]
        if let Some(ref mut js) = self.js {
            return js.dispatch_submit(form_id);
        }
        #[cfg(not(feature = "js"))]
        let _ = form_id;
        false
    }

    fn drain_shared_effects(&mut self) -> Vec<HtmlEffect> {
        let mut effects = Vec::new();
        if let Ok(mut st) = self.shared.lock() {
            if st.dirty {
                st.dirty = false;
                effects.push(HtmlEffect::DomChanged);
            }
            for msg in st.alerts.drain(..) {
                effects.push(HtmlEffect::Toast(msg));
            }
            for url in st.open_urls.drain(..) {
                effects.push(HtmlEffect::OpenUrl(url));
            }
        }
        effects
    }

    fn control_count(&mut self) -> usize {
        if self.cached_render.is_none() {
            let theme = HtmlTheme::dark();
            self.render(80, &theme, false);
        }
        self.cached_render
            .as_ref()
            .map(|r| r.controls.len())
            .unwrap_or(0)
    }

    fn invalidate_cache(&mut self) {
        self.cache_key = None;
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
    fn raw_toggle_focus_cycling_and_text_input_editing() {
        let html = r#"
            <form action="/submit">
              <input type="text" name="user" value="hi" />
              <button type="submit">Send</button>
            </form>
            <a href="https://bitefoundry.dev">Docs</a>
        "#;
        let mut session = HtmlViewSession::new(Some("Form".into()), html.into());
        assert!(!session.js_active);

        let theme = HtmlTheme::dark();
        let r = session.render(80, &theme, false);
        assert_eq!(r.controls.len(), 3);

        // Focus text input and edit it
        session.focus_next_control();
        assert_eq!(session.focused_control, Some(0));
        session.activate_focused();
        assert!(session.editing_input.is_some());
        session.input_char('!');
        session.backspace();
        session.input_char('2');
        let finish_effs = session.finish_editing();
        assert!(finish_effs.contains(&HtmlEffect::DomChanged));
        assert!(session.editing_input.is_none());

        // Focus submit button and submit form
        session.focus_next_control();
        assert_eq!(session.focused_control, Some(1));
        let submit_effs = session.activate_focused();
        assert!(submit_effs.contains(&HtmlEffect::FormSubmitted {
            action: "/submit".into(),
            fields: vec![("user".into(), "hi2".into())],
        }));

        // Focus link and activate
        session.focus_next_control();
        assert_eq!(session.focused_control, Some(2));
        let link_effs = session.activate_focused();
        assert!(link_effs.contains(&HtmlEffect::OpenUrl("https://bitefoundry.dev".into())));

        // Toggle raw mode
        assert!(session.toggle_raw_mode());
        let raw_render = session.render(80, &theme, false);
        assert!(rendered_text(raw_render).contains("<form action=\"/submit\">"));
        assert!(raw_render.controls.is_empty());
        assert!(!session.toggle_raw_mode());
    }

    #[test]
    fn click_at_hits_exact_button_coordinates() {
        let html = r#"<p>Click <details><summary>Open Me</summary><p>Revealed</p></details></p>"#;
        let mut session = HtmlViewSession::new(None, html.into());
        let theme = HtmlTheme::dark();
        let r0 = session.render(80, &theme, false);
        let ctrl = r0.controls[0].clone();
        assert!(!rendered_text(r0).contains("Revealed"));

        // Clicking outside control does nothing
        let miss = session.click_at(ctrl.line, ctrl.col_end + 5);
        assert!(miss.is_empty());

        // Clicking inside control opens details
        let hit = session.click_at(ctrl.line, ctrl.col_start + 1);
        assert!(hit.contains(&HtmlEffect::DomChanged));
        let r1 = session.render(80, &theme, false);
        assert!(rendered_text(r1).contains("Revealed"));
    }
}
