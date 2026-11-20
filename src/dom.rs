//! Mutable arena-backed DOM parsed from HTML via `scraper`.

use scraper::node::Node;
use scraper::Html;

/// Arena index identifying a node within a [`DomDocument`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DomNodeId(pub usize);

/// Kind of DOM node stored in the arena.
#[derive(Debug, Clone)]
pub enum DomNodeKind {
    Document,
    Element(ElementData),
    Text(String),
}

/// Mutable state for an HTML element node.
#[derive(Debug, Clone)]
pub struct ElementData {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub open: bool,
    pub checked: bool,
    pub value: String,
    pub selected_index: usize,
    pub hidden: bool,
    pub display_none: bool,
    pub inline_color: Option<String>,
    /// Tracks explicit inline `style="display: ..."` value (lowercased) to override stylesheet rules.
    pub(crate) inline_display: Option<String>,
}

impl ElementData {
    pub fn new(tag: &str) -> Self {
        Self {
            tag: tag.to_ascii_lowercase(),
            attrs: Vec::new(),
            open: false,
            checked: false,
            value: String::new(),
            selected_index: 0,
            hidden: false,
            display_none: false,
            inline_color: None,
            inline_display: None,
        }
    }

    pub fn get_attr(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.attrs
            .iter()
            .find(|(k, _)| *k == lower)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_attr(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.attrs.iter().any(|(k, _)| *k == lower)
    }

    pub fn input_type(&self) -> &str {
        self.get_attr("type").unwrap_or("text")
    }

    pub fn has_event_handler_attr(&self) -> bool {
        self.attrs
            .iter()
            .any(|(k, v)| k.starts_with("on") && k.len() > 2 && !v.trim().is_empty())
    }

    pub(crate) fn sync_from_attrs(&mut self) {
        self.open = self.has_attr("open");
        self.checked = self.has_attr("checked");
        self.hidden = self.has_attr("hidden");
        if self.tag == "input" {
            if let Some(v) = self.get_attr("value") {
                self.value = v.to_string();
            }
        }
        self.parse_inline_style();
    }

    pub(crate) fn parse_inline_style(&mut self) {
        self.display_none = false;
        self.inline_display = None;
        self.inline_color = None;
        let Some(style) = self.get_attr("style").map(|s| s.to_string()) else {
            return;
        };
        for decl in style.split(';') {
            let Some((prop, val)) = decl.split_once(':') else {
                continue;
            };
            let prop = prop.trim().to_ascii_lowercase();
            let val = val
                .trim()
                .trim_end_matches("!important")
                .trim()
                .to_string();
            if prop == "display" {
                let lower = val.to_ascii_lowercase();
                self.display_none = lower == "none";
                self.inline_display = Some(lower);
            } else if prop == "color" && !val.is_empty() {
                self.inline_color = Some(val);
            }
        }
    }
}

/// A single node in the arena.
#[derive(Debug, Clone)]
pub struct DomNode {
    pub id: DomNodeId,
    pub parent: Option<DomNodeId>,
    pub children: Vec<DomNodeId>,
    pub kind: DomNodeKind,
}

#[derive(Debug, Clone)]
struct StyleRule {
    selector: String,
    display_none: Option<bool>,
    color: Option<String>,
}

/// Mutable arena-based HTML DOM document.
#[derive(Debug, Clone)]
pub struct DomDocument {
    nodes: Vec<DomNode>,
    root: DomNodeId,
    scripts: Vec<String>,
    style_rules: Vec<StyleRule>,
    title_override: Option<String>,
}

impl DomDocument {
    /// Parses a full HTML string into a mutable [`DomDocument`].
    pub fn parse(html: &str) -> Self {
        let parsed = Html::parse_document(html);
        let mut doc = Self {
            nodes: vec![DomNode {
                id: DomNodeId(0),
                parent: None,
                children: Vec::new(),
                kind: DomNodeKind::Document,
            }],
            root: DomNodeId(0),
            scripts: Vec::new(),
            style_rules: Vec::new(),
            title_override: None,
        };

        doc.ingest_scraper_tree(DomNodeId(0), &parsed, false);

        doc.finalize_subtree(DomNodeId(0), true);
        doc
    }

    fn ingest_scraper_tree(&mut self, root_parent: DomNodeId, html: &Html, unwrap_html_root: bool) {
        let mut stack = Vec::new();
        let top_children: Vec<_> = html.tree.root().children().collect();
        for child in top_children.into_iter().rev() {
            if unwrap_html_root {
                if let Node::Element(el) = child.value() {
                    if el.name().eq_ignore_ascii_case("html") {
                        let grand: Vec<_> = child.children().collect();
                        for g in grand.into_iter().rev() {
                            stack.push((root_parent, g));
                        }
                        continue;
                    }
                }
            }
            stack.push((root_parent, child));
        }

        while let Some((parent, node_ref)) = stack.pop() {
            match node_ref.value() {
                Node::Element(el) => {
                    let mut data = ElementData::new(el.name());
                    for (name, value) in el.attrs() {
                        data.attrs
                            .push((name.to_ascii_lowercase(), value.to_string()));
                    }
                    data.sync_from_attrs();

                    let id = self.alloc_node(Some(parent), DomNodeKind::Element(data));
                    self.nodes[parent.0].children.push(id);
                    let children: Vec<_> = node_ref.children().collect();
                    for child in children.into_iter().rev() {
                        stack.push((id, child));
                    }
                }
                Node::Text(text) => {
                    let s = text.text.to_string();
                    let id = self.alloc_node(Some(parent), DomNodeKind::Text(s));
                    self.nodes[parent.0].children.push(id);
                }
                Node::Document | Node::Fragment => {
                    let children: Vec<_> = node_ref.children().collect();
                    for child in children.into_iter().rev() {
                        stack.push((parent, child));
                    }
                }
                Node::Doctype(_) | Node::Comment(_) | Node::ProcessingInstruction(_) => {}
            }
        }
    }

    fn finalize_subtree(&mut self, root: DomNodeId, collect_scripts_and_styles: bool) {
        let mut stack = vec![root];
        let mut order = Vec::new();
        while let Some(id) = stack.pop() {
            order.push(id);
            let children = self.nodes[id.0].children.clone();
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        }

        for id in order {
            let tag = match &self.nodes[id.0].kind {
                DomNodeKind::Element(el) => el.tag.clone(),
                _ => continue,
            };

            if tag == "textarea" {
                let txt = self.text_content(id);
                if let Some(el) = self.element_mut(id) {
                    el.value = txt;
                }
            } else if tag == "select" {
                self.sync_select_state(id);
            } else if collect_scripts_and_styles && tag == "script" {
                let is_inline_js = self.element(id).is_some_and(|el| {
                    if el.has_attr("src") {
                        return false;
                    }
                    match el.get_attr("type").map(|t| t.trim().to_ascii_lowercase()) {
                        None => true,
                        Some(t) if t.is_empty() => true,
                        Some(t)
                            if t == "text/javascript"
                                || t == "application/javascript"
                                || t == "module" =>
                        {
                            true
                        }
                        _ => false,
                    }
                });
                if is_inline_js {
                    let body = self.text_content(id);
                    if !body.trim().is_empty() {
                        self.scripts.push(body);
                    }
                }
            } else if collect_scripts_and_styles && tag == "style" {
                let css = self.text_content(id);
                self.parse_stylesheet(&css);
            }
        }
    }

    fn sync_select_state(&mut self, select_id: DomNodeId) {
        let options = self.collect_option_nodes(select_id);
        if options.is_empty() {
            if let Some(el) = self.element_mut(select_id) {
                el.selected_index = 0;
                el.value.clear();
            }
            return;
        }
        let selected_idx = options
            .iter()
            .position(|&opt_id| self.element(opt_id).is_some_and(|e| e.has_attr("selected")))
            .unwrap_or(0);
        let selected_val = self.option_value(options[selected_idx]);
        if let Some(el) = self.element_mut(select_id) {
            el.selected_index = selected_idx;
            el.value = selected_val;
        }
    }

    fn parse_stylesheet(&mut self, css: &str) {
        // Strip /* ... */ comments.
        let mut clean = String::with_capacity(css.len());
        let mut chars = css.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                while let Some(cc) = chars.next() {
                    if cc == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            } else {
                clean.push(c);
            }
        }

        for block in clean.split('}') {
            let Some((selectors_str, body)) = block.split_once('{') else {
                continue;
            };
            let selectors_str = selectors_str.trim();
            if selectors_str.is_empty() || selectors_str.starts_with('@') {
                continue;
            }
            let mut display_none = None;
            let mut color = None;
            for decl in body.split(';') {
                let Some((prop, val)) = decl.split_once(':') else {
                    continue;
                };
                let prop = prop.trim().to_ascii_lowercase();
                let val = val
                    .trim()
                    .trim_end_matches("!important")
                    .trim()
                    .to_string();
                if prop == "display" {
                    display_none = Some(val.eq_ignore_ascii_case("none"));
                } else if prop == "color" && !val.is_empty() {
                    color = Some(val);
                }
            }
            if display_none.is_some() || color.is_some() {
                for sel in selectors_str.split(',') {
                    let sel = sel.trim();
                    if !sel.is_empty() {
                        self.style_rules.push(StyleRule {
                            selector: sel.to_string(),
                            display_none,
                            color: color.clone(),
                        });
                    }
                }
            }
        }
    }

    fn alloc_node(&mut self, parent: Option<DomNodeId>, kind: DomNodeKind) -> DomNodeId {
        let id = DomNodeId(self.nodes.len());
        self.nodes.push(DomNode {
            id,
            parent,
            children: Vec::new(),
            kind,
        });
        id
    }

    pub fn root(&self) -> DomNodeId {
        self.root
    }

    pub fn node(&self, id: DomNodeId) -> Option<&DomNode> {
        self.nodes.get(id.0)
    }

    pub fn node_mut(&mut self, id: DomNodeId) -> Option<&mut DomNode> {
        self.nodes.get_mut(id.0)
    }

    pub fn element(&self, id: DomNodeId) -> Option<&ElementData> {
        match &self.nodes.get(id.0)?.kind {
            DomNodeKind::Element(el) => Some(el),
            _ => None,
        }
    }

    pub fn element_mut(&mut self, id: DomNodeId) -> Option<&mut ElementData> {
        match &mut self.nodes.get_mut(id.0)?.kind {
            DomNodeKind::Element(el) => Some(el),
            _ => None,
        }
    }

    /// Returns document `<title>` text, if present and non-empty.
    pub fn title(&self) -> Option<String> {
        if let Some(ref override_title) = self.title_override {
            return Some(override_title.clone());
        }
        let title_id = self.query_selector("title")?;
        let text = self.text_content(title_id);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    pub fn set_title(&mut self, title: &str) {
        self.title_override = Some(title.to_string());
    }

    /// Inline `<script>` bodies collected during parsing.
    pub fn inline_scripts(&self) -> &[String] {
        &self.scripts
    }

    /// Returns `true` if the document contains any inline `<script>` tags or `on*` event handler attributes.
    pub fn has_scripts_or_handlers(&self) -> bool {
        if !self.scripts.is_empty() {
            return true;
        }
        self.descendants_preorder(self.root).into_iter().any(|id| {
            self.element(id)
                .is_some_and(ElementData::has_event_handler_attr)
        })
    }

    /// Returns whether an element is effectively hidden via `hidden`, inline `display:none`, or `<style>` rule.
    pub fn is_effectively_hidden(&self, id: DomNodeId) -> bool {
        let Some(el) = self.element(id) else {
            return false;
        };
        if el.hidden {
            return true;
        }
        if let Some(ref disp) = el.inline_display {
            return disp == "none";
        }
        if el.display_none {
            return true;
        }
        let mut hidden_by_css = false;
        for rule in &self.style_rules {
            if let Some(dn) = rule.display_none {
                if self.matches_selector(id, &rule.selector) {
                    hidden_by_css = dn;
                }
            }
        }
        hidden_by_css
    }

    /// Effective CSS color from inline style or `<style>` rule.
    pub fn effective_color(&self, id: DomNodeId) -> Option<String> {
        let el = self.element(id)?;
        if el.inline_color.is_some() {
            return el.inline_color.clone();
        }
        let mut result = None;
        for rule in &self.style_rules {
            if let Some(ref c) = rule.color {
                if self.matches_selector(id, &rule.selector) {
                    result = Some(c.clone());
                }
            }
        }
        result
    }

    fn descendants_preorder(&self, root: DomNodeId) -> Vec<DomNodeId> {
        let mut out = Vec::new();
        let mut stack = Vec::new();
        if let Some(node) = self.nodes.get(root.0) {
            for &c in node.children.iter().rev() {
                stack.push(c);
            }
        }
        while let Some(id) = stack.pop() {
            out.push(id);
            if let Some(node) = self.nodes.get(id.0) {
                for &c in node.children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    pub fn get_element_by_id(&self, id_attr: &str) -> Option<DomNodeId> {
        for id in self.descendants_preorder(self.root) {
            if let Some(el) = self.element(id) {
                if el.get_attr("id") == Some(id_attr) {
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn query_selector(&self, selector: &str) -> Option<DomNodeId> {
        self.query_selector_from(self.root, selector)
    }

    pub fn query_selector_all(&self, selector: &str) -> Vec<DomNodeId> {
        self.query_selector_all_from(self.root, selector)
    }

    pub fn query_selector_from(&self, root: DomNodeId, selector: &str) -> Option<DomNodeId> {
        let parts: Vec<&str> = selector
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return None;
        }
        self.descendants_preorder(root)
            .into_iter()
            .find(|&id| self.element(id).is_some() && parts.iter().any(|s| self.matches_selector(id, s)))
    }

    pub fn query_selector_all_from(&self, root: DomNodeId, selector: &str) -> Vec<DomNodeId> {
        let parts: Vec<&str> = selector
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        for id in self.descendants_preorder(root) {
            if self.element(id).is_some() && parts.iter().any(|s| self.matches_selector(id, s)) {
                matches.push(id);
            }
        }
        matches
    }

    fn matches_selector(&self, node_id: DomNodeId, selector: &str) -> bool {
        let tokens = tokenize_selector_chain(selector);
        if tokens.is_empty() {
            return false;
        }
        self.matches_chain(node_id, &tokens)
    }

    fn matches_chain(&self, node_id: DomNodeId, chain: &[SelectorStep]) -> bool {
        let Some((last, rest)) = chain.split_last() else {
            return true;
        };
        if !self.matches_compound(node_id, &last.compound) {
            return false;
        }
        if rest.is_empty() {
            return true;
        }
        match last.combinator_from_prev {
            Combinator::Child => {
                let Some(parent) = self.nodes[node_id.0].parent else {
                    return false;
                };
                self.element(parent).is_some() && self.matches_chain(parent, rest)
            }
            Combinator::Descendant | Combinator::None => {
                let mut cur = self.nodes[node_id.0].parent;
                while let Some(anc) = cur {
                    if self.element(anc).is_some() && self.matches_chain(anc, rest) {
                        return true;
                    }
                    cur = self.nodes[anc.0].parent;
                }
                false
            }
        }
    }

    fn matches_compound(&self, node_id: DomNodeId, compound: &str) -> bool {
        let Some(el) = self.element(node_id) else {
            return false;
        };
        let bytes = compound.as_bytes();
        let mut i = 0;
        let len = bytes.len();
        if len == 0 {
            return false;
        }

        // Optional leading tag or '*'
        if bytes[0] != b'#' && bytes[0] != b'.' && bytes[0] != b'[' {
            let start = i;
            while i < len && bytes[i] != b'#' && bytes[i] != b'.' && bytes[i] != b'[' {
                i += 1;
            }
            let tag = &compound[start..i];
            if tag != "*" && !el.tag.eq_ignore_ascii_case(tag) {
                return false;
            }
        }

        while i < len {
            match bytes[i] {
                b'#' => {
                    i += 1;
                    let start = i;
                    while i < len && bytes[i] != b'#' && bytes[i] != b'.' && bytes[i] != b'[' {
                        i += 1;
                    }
                    let expected = &compound[start..i];
                    if el.get_attr("id") != Some(expected) {
                        return false;
                    }
                }
                b'.' => {
                    i += 1;
                    let start = i;
                    while i < len && bytes[i] != b'#' && bytes[i] != b'.' && bytes[i] != b'[' {
                        i += 1;
                    }
                    let expected = &compound[start..i];
                    if !self.has_class(node_id, expected) {
                        return false;
                    }
                }
                b'[' => {
                    i += 1;
                    let start = i;
                    while i < len && bytes[i] != b']' {
                        i += 1;
                    }
                    let inner = compound[start..i].trim();
                    if i < len && bytes[i] == b']' {
                        i += 1;
                    }
                    if let Some((attr, raw_val)) = inner.split_once('=') {
                        let attr = attr.trim();
                        let val = raw_val
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'');
                        if el.get_attr(attr) != Some(val) {
                            return false;
                        }
                    } else if !el.has_attr(inner) {
                        return false;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }
        true
    }

    /// Concatenates all descendant text node contents.
    pub fn text_content(&self, id: DomNodeId) -> String {
        let mut out = String::new();
        self.collect_text(id, &mut out);
        out
    }

    fn collect_text(&self, id: DomNodeId, out: &mut String) {
        let Some(node) = self.nodes.get(id.0) else {
            return;
        };
        match &node.kind {
            DomNodeKind::Text(t) => out.push_str(t),
            DomNodeKind::Document | DomNodeKind::Element(_) => {
                for &child in &node.children {
                    self.collect_text(child, out);
                }
            }
        }
    }

    /// Replaces children of `id` with a single text node (or clears children when empty).
    pub fn set_text_content(&mut self, id: DomNodeId, text: &str) {
        if self.nodes.get(id.0).is_none() {
            return;
        }
        if let DomNodeKind::Text(ref mut existing) = self.nodes[id.0].kind {
            *existing = text.to_string();
            return;
        }
        let old_children = std::mem::take(&mut self.nodes[id.0].children);
        for c in old_children {
            self.nodes[c.0].parent = None;
        }
        if !text.is_empty() {
            let txt_id = self.alloc_node(Some(id), DomNodeKind::Text(text.to_string()));
            self.nodes[id.0].children.push(txt_id);
        }
        if let Some(el) = self.element_mut(id) {
            if el.tag == "textarea" {
                el.value = text.to_string();
            }
        }
    }

    /// Re-parses an HTML fragment and replaces `id`'s children with the parsed nodes.
    pub fn set_inner_html(&mut self, id: DomNodeId, html: &str) {
        if self.nodes.get(id.0).is_none() {
            return;
        }
        let old_children = std::mem::take(&mut self.nodes[id.0].children);
        for c in old_children {
            self.nodes[c.0].parent = None;
        }
        let frag = Html::parse_fragment(html);
        self.ingest_scraper_tree(id, &frag, true);
        if self.nodes[id.0].children.is_empty() && !html.is_empty() {
            let txt = self.alloc_node(Some(id), DomNodeKind::Text(html.to_string()));
            self.nodes[id.0].children.push(txt);
        }
        self.finalize_subtree(id, false);
    }

    /// Serializes an element's children back to HTML.
    pub fn inner_html(&self, id: DomNodeId) -> String {
        let Some(node) = self.nodes.get(id.0) else {
            return String::new();
        };
        let mut out = String::new();
        for &child in &node.children {
            self.serialize_node(child, &mut out);
        }
        out
    }

    fn serialize_node(&self, id: DomNodeId, out: &mut String) {
        let Some(node) = self.nodes.get(id.0) else {
            return;
        };
        match &node.kind {
            DomNodeKind::Text(t) => out.push_str(t),
            DomNodeKind::Document => {
                for &c in &node.children {
                    self.serialize_node(c, out);
                }
            }
            DomNodeKind::Element(el) => {
                out.push('<');
                out.push_str(&el.tag);
                for (k, v) in &el.attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&v.replace('"', "&quot;"));
                    out.push('"');
                }
                out.push('>');
                if !is_void_tag(&el.tag) {
                    for &c in &node.children {
                        self.serialize_node(c, out);
                    }
                    out.push_str("</");
                    out.push_str(&el.tag);
                    out.push('>');
                }
            }
        }
    }

    pub fn get_attr(&self, id: DomNodeId, name: &str) -> Option<&str> {
        self.element(id)?.get_attr(name)
    }

    pub fn set_attr(&mut self, id: DomNodeId, name: &str, value: &str) {
        let Some(el) = self.element_mut(id) else {
            return;
        };
        let lower = name.to_ascii_lowercase();
        if let Some((_, existing)) = el.attrs.iter_mut().find(|(k, _)| *k == lower) {
            *existing = value.to_string();
        } else {
            el.attrs.push((lower.clone(), value.to_string()));
        }
        match lower.as_str() {
            "open" => el.open = true,
            "checked" => el.checked = true,
            "hidden" => el.hidden = true,
            "value" => el.value = value.to_string(),
            "style" => el.parse_inline_style(),
            _ => {}
        }
    }

    pub fn remove_attr(&mut self, id: DomNodeId, name: &str) {
        let Some(el) = self.element_mut(id) else {
            return;
        };
        let lower = name.to_ascii_lowercase();
        el.attrs.retain(|(k, _)| *k != lower);
        match lower.as_str() {
            "open" => el.open = false,
            "checked" => el.checked = false,
            "hidden" => el.hidden = false,
            "style" => el.parse_inline_style(),
            _ => {}
        }
    }

    pub fn has_class(&self, id: DomNodeId, class_name: &str) -> bool {
        let Some(classes) = self.get_attr(id, "class") else {
            return false;
        };
        classes.split_whitespace().any(|c| c == class_name)
    }

    pub fn add_class(&mut self, id: DomNodeId, class_name: &str) {
        let class_name = class_name.trim();
        if class_name.is_empty() || self.has_class(id, class_name) {
            return;
        }
        let mut classes: Vec<String> = self
            .get_attr(id, "class")
            .unwrap_or("")
            .split_whitespace()
            .map(ToString::to_string)
            .collect();
        classes.push(class_name.to_string());
        self.set_attr(id, "class", &classes.join(" "));
    }

    pub fn remove_class(&mut self, id: DomNodeId, class_name: &str) {
        let class_name = class_name.trim();
        let Some(existing) = self.get_attr(id, "class") else {
            return;
        };
        let remaining: Vec<&str> = existing
            .split_whitespace()
            .filter(|&c| c != class_name)
            .collect();
        let joined = remaining.join(" ");
        if joined.is_empty() {
            self.remove_attr(id, "class");
        } else {
            self.set_attr(id, "class", &joined);
        }
    }

    /// Toggles `class_name` on `id` and returns whether the class is now present.
    pub fn toggle_class(&mut self, id: DomNodeId, class_name: &str) -> bool {
        if self.has_class(id, class_name) {
            self.remove_class(id, class_name);
            false
        } else {
            self.add_class(id, class_name);
            true
        }
    }

    /// Sets an inline CSS property (e.g. `display`, `color`) on an element's `style` attribute.
    pub fn set_style_property(&mut self, id: DomNodeId, prop: &str, val: &str) {
        let prop_lower = prop.trim().to_ascii_lowercase();
        let val_trimmed = val.trim();
        let existing = self.get_attr(id, "style").unwrap_or("").to_string();
        let mut decls: Vec<(String, String)> = existing
            .split(';')
            .filter_map(|part| {
                let (k, v) = part.split_once(':')?;
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().to_string();
                if k.is_empty() {
                    None
                } else {
                    Some((k, v))
                }
            })
            .collect();

        if val_trimmed.is_empty() {
            decls.retain(|(k, _)| *k != prop_lower);
        } else if let Some((_, v)) = decls.iter_mut().find(|(k, _)| *k == prop_lower) {
            *v = val_trimmed.to_string();
        } else {
            decls.push((prop_lower, val_trimmed.to_string()));
        }

        let rebuilt = decls
            .into_iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("; ");
        if rebuilt.is_empty() {
            self.remove_attr(id, "style");
        } else {
            self.set_attr(id, "style", &rebuilt);
        }
    }

    /// Gets an inline CSS property from an element's `style` attribute.
    pub fn get_style_property(&self, id: DomNodeId, prop: &str) -> String {
        let prop_lower = prop.trim().to_ascii_lowercase();
        let Some(existing) = self.get_attr(id, "style") else {
            return String::new();
        };
        for part in existing.split(';') {
            if let Some((k, v)) = part.split_once(':') {
                if k.trim().eq_ignore_ascii_case(&prop_lower) {
                    return v.trim().to_string();
                }
            }
        }
        String::new()
    }

    pub fn create_element(&mut self, tag: &str) -> DomNodeId {
        self.alloc_node(None, DomNodeKind::Element(ElementData::new(tag)))
    }

    pub fn create_text_node(&mut self, text: &str) -> DomNodeId {
        self.alloc_node(None, DomNodeKind::Text(text.to_string()))
    }

    pub fn append_child(&mut self, parent: DomNodeId, child: DomNodeId) -> bool {
        if self.nodes.get(parent.0).is_none()
            || self.nodes.get(child.0).is_none()
            || parent == child
        {
            return false;
        }
        if let Some(old_parent) = self.nodes[child.0].parent {
            self.nodes[old_parent.0].children.retain(|&c| c != child);
        }
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
        true
    }

    pub fn remove_node(&mut self, id: DomNodeId) -> bool {
        let Some(parent) = self.nodes.get(id.0).and_then(|n| n.parent) else {
            return false;
        };
        self.nodes[parent.0].children.retain(|&c| c != id);
        self.nodes[id.0].parent = None;
        true
    }

    /// Toggles a `<details>` element (accepts either the `<details>` or its `<summary>` child).
    pub fn toggle_details(&mut self, node_id: DomNodeId) -> bool {
        let details_id = match self.element(node_id).map(|e| e.tag.as_str()) {
            Some("details") => node_id,
            Some("summary") => match self.nodes[node_id.0].parent {
                Some(p) if self.element(p).is_some_and(|e| e.tag == "details") => p,
                _ => return false,
            },
            _ => return false,
        };
        let new_open = !self.element(details_id).is_some_and(|e| e.open);
        if new_open {
            self.set_attr(details_id, "open", "");
        } else {
            self.remove_attr(details_id, "open");
        }
        new_open
    }

    /// Toggles a checkbox or selects a radio button (unchecking same-`name` radio siblings).
    pub fn toggle_checkbox_or_radio(&mut self, id: DomNodeId) -> bool {
        let (is_radio, radio_name, cur_checked) = match self.element(id) {
            Some(el) if el.tag == "input" => {
                let kind = el.input_type().to_ascii_lowercase();
                if kind == "checkbox" {
                    (false, None, el.checked)
                } else if kind == "radio" {
                    (true, el.get_attr("name").map(ToString::to_string), el.checked)
                } else {
                    return false;
                }
            }
            _ => return false,
        };

        if !is_radio {
            let next = !cur_checked;
            if next {
                self.set_attr(id, "checked", "");
            } else {
                self.remove_attr(id, "checked");
            }
            return next;
        }

        // Radio group exclusivity: uncheck other radios with the same `name`.
        if let Some(ref name) = radio_name {
            if !name.is_empty() {
                let scope_root = self.ancestor_tag(id, "form").unwrap_or(self.root);
                let all_nodes = self.descendants_preorder(scope_root);
                for other_id in all_nodes {
                    if other_id == id {
                        continue;
                    }
                    let same_group = self.element(other_id).is_some_and(|el| {
                        el.tag == "input"
                            && el.input_type().eq_ignore_ascii_case("radio")
                            && el.get_attr("name") == Some(name.as_str())
                    });
                    if same_group {
                        self.remove_attr(other_id, "checked");
                    }
                }
            }
        }

        self.set_attr(id, "checked", "");
        true
    }

    fn collect_option_nodes(&self, select_id: DomNodeId) -> Vec<DomNodeId> {
        self.descendants_preorder(select_id)
            .into_iter()
            .filter(|&cid| self.element(cid).is_some_and(|e| e.tag == "option"))
            .collect()
    }

    fn option_value(&self, option_id: DomNodeId) -> String {
        if let Some(v) = self.get_attr(option_id, "value") {
            v.to_string()
        } else {
            self.text_content(option_id).trim().to_string()
        }
    }

    /// Returns `(value, label)` pairs for all `<option>` descendants of a `<select>`.
    pub fn select_options(&self, select_id: DomNodeId) -> Vec<(String, String)> {
        self.collect_option_nodes(select_id)
            .into_iter()
            .map(|opt_id| {
                let label = self.text_content(opt_id).trim().to_string();
                let val = self.option_value(opt_id);
                (val, label)
            })
            .collect()
    }

    /// Cycles a `<select>` to its next `<option>`, returning `(new_index, label)`.
    pub fn cycle_select(&mut self, id: DomNodeId) -> Option<(usize, String)> {
        if !self.element(id).is_some_and(|e| e.tag == "select") {
            return None;
        }
        let options = self.collect_option_nodes(id);
        if options.is_empty() {
            return None;
        }
        let cur = self.element(id)?.selected_index;
        let next_idx = (cur + 1) % options.len();
        self.set_select_index(id, next_idx)
    }

    pub fn set_select_index(&mut self, id: DomNodeId, index: usize) -> Option<(usize, String)> {
        let options = self.collect_option_nodes(id);
        if options.is_empty() {
            return None;
        }
        let clamped = index.min(options.len() - 1);
        for (i, &opt_id) in options.iter().enumerate() {
            if i == clamped {
                self.set_attr(opt_id, "selected", "");
            } else {
                self.remove_attr(opt_id, "selected");
            }
        }
        let new_val = self.option_value(options[clamped]);
        let label = self.text_content(options[clamped]).trim().to_string();
        if let Some(el) = self.element_mut(id) {
            el.selected_index = clamped;
            el.value = new_val;
        }
        Some((clamped, label))
    }

    /// Updates a text input or textarea's value.
    pub fn set_input_value(&mut self, id: DomNodeId, value: &str) {
        let Some(tag) = self.element(id).map(|e| e.tag.clone()) else {
            return;
        };
        if tag == "textarea" {
            self.set_text_content(id, value);
        } else if tag == "select" {
            let options = self.collect_option_nodes(id);
            if let Some(idx) = options.iter().position(|&o| self.option_value(o) == value) {
                self.set_select_index(id, idx);
            } else if let Some(el) = self.element_mut(id) {
                el.value = value.to_string();
            }
        } else {
            self.set_attr(id, "value", value);
        }
    }

    /// Finds the nearest ancestor element with the given tag name.
    pub fn ancestor_tag(&self, id: DomNodeId, tag: &str) -> Option<DomNodeId> {
        let mut cur = self.nodes.get(id.0)?.parent;
        while let Some(p) = cur {
            if self
                .element(p)
                .is_some_and(|e| e.tag.eq_ignore_ascii_case(tag))
            {
                return Some(p);
            }
            cur = self.nodes[p.0].parent;
        }
        None
    }

    /// Collects `(action, fields)` for the `<form>` enclosing `control_id` (or `control_id` itself if it's a `<form>`).
    pub fn collect_form_data(&self, control_id: DomNodeId) -> Option<(String, Vec<(String, String)>)> {
        let form_id = if self.element(control_id).is_some_and(|e| e.tag == "form") {
            control_id
        } else {
            self.ancestor_tag(control_id, "form")?
        };
        let action = self.get_attr(form_id, "action").unwrap_or("").to_string();
        let mut fields = Vec::new();
        for id in self.descendants_preorder(form_id) {
            let Some(el) = self.element(id) else {
                continue;
            };
            let Some(name) = el.get_attr("name").filter(|n| !n.is_empty()) else {
                continue;
            };
            match el.tag.as_str() {
                "input" => {
                    let t = el.input_type().to_ascii_lowercase();
                    if t == "checkbox" || t == "radio" {
                        if el.checked {
                            let val = if el.value.is_empty() {
                                "on".to_string()
                            } else {
                                el.value.clone()
                            };
                            fields.push((name.to_string(), val));
                        }
                    } else if t != "button" && t != "submit" && t != "reset" {
                        fields.push((name.to_string(), el.value.clone()));
                    }
                }
                "textarea" | "select" => {
                    fields.push((name.to_string(), el.value.clone()));
                }
                _ => {}
            }
        }
        Some((action, fields))
    }
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Combinator {
    None,
    Descendant,
    Child,
}

#[derive(Debug, Clone)]
struct SelectorStep {
    combinator_from_prev: Combinator,
    compound: String,
}

fn tokenize_selector_chain(selector: &str) -> Vec<SelectorStep> {
    let mut steps = Vec::new();
    let mut chars = selector.trim().chars().peekable();
    let mut pending_comb = Combinator::None;

    while chars.peek().is_some() {
        // Consume whitespace / `>` combinators
        let mut saw_space = false;
        let mut saw_gt = false;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_whitespace() {
                saw_space = true;
                chars.next();
            } else if c == '>' {
                saw_gt = true;
                chars.next();
            } else {
                break;
            }
        }
        if !steps.is_empty() {
            pending_comb = if saw_gt {
                Combinator::Child
            } else if saw_space {
                Combinator::Descendant
            } else {
                Combinator::None
            };
        }

        // Read one compound selector (respecting brackets `[...]`)
        let mut compound = String::new();
        let mut in_brackets = false;
        while let Some(&c) = chars.peek() {
            if c == '[' {
                in_brackets = true;
                compound.push(c);
                chars.next();
            } else if c == ']' {
                in_brackets = false;
                compound.push(c);
                chars.next();
            } else if !in_brackets && (c.is_ascii_whitespace() || c == '>') {
                break;
            } else {
                compound.push(c);
                chars.next();
            }
        }

        if !compound.is_empty() {
            steps.push(SelectorStep {
                combinator_from_prev: pending_comb,
                compound,
            });
        }
    }

    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_query_selectors() {
        let html = r#"
            <html>
              <head><title>Demo Page</title></head>
              <body>
                <div id="main" class="box primary">
                  <p class="intro" data-role="lead">Hello</p>
                  <button data-action="go">Go</button>
                </div>
              </body>
            </html>
        "#;
        let doc = DomDocument::parse(html);
        assert_eq!(doc.title().as_deref(), Some("Demo Page"));
        assert!(!doc.has_scripts_or_handlers());

        let main_id = doc.get_element_by_id("main").unwrap();
        assert!(doc.has_class(main_id, "box"));
        assert!(doc.has_class(main_id, "primary"));

        assert_eq!(doc.query_selector("#main"), Some(main_id));
        assert_eq!(doc.query_selector("div.primary"), Some(main_id));

        let lead = doc.query_selector("div#main p[data-role=\"lead\"]").unwrap();
        assert_eq!(doc.text_content(lead), "Hello");

        let both = doc.query_selector_all("p.intro, button[data-action]");
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn details_checkbox_radio_and_select_mutations() {
        let html = r#"
            <details id="d"><summary id="s">More</summary><p>Secret</p></details>
            <input id="cb" type="checkbox" />
            <input id="r1" type="radio" name="grp" value="a" checked />
            <input id="r2" type="radio" name="grp" value="b" />
            <select id="sel">
              <option value="one">One</option>
              <option value="two" selected>Two</option>
              <option value="three">Three</option>
            </select>
        "#;
        let mut doc = DomDocument::parse(html);

        let d = doc.get_element_by_id("d").unwrap();
        let s = doc.get_element_by_id("s").unwrap();
        assert!(!doc.element(d).unwrap().open);
        assert!(doc.toggle_details(s));
        assert!(doc.element(d).unwrap().open);
        assert!(!doc.toggle_details(d));
        assert!(!doc.element(d).unwrap().open);

        let cb = doc.get_element_by_id("cb").unwrap();
        assert!(!doc.element(cb).unwrap().checked);
        assert!(doc.toggle_checkbox_or_radio(cb));
        assert!(doc.element(cb).unwrap().checked);

        let r1 = doc.get_element_by_id("r1").unwrap();
        let r2 = doc.get_element_by_id("r2").unwrap();
        assert!(doc.element(r1).unwrap().checked);
        assert!(!doc.element(r2).unwrap().checked);
        doc.toggle_checkbox_or_radio(r2);
        assert!(!doc.element(r1).unwrap().checked);
        assert!(doc.element(r2).unwrap().checked);

        let sel = doc.get_element_by_id("sel").unwrap();
        assert_eq!(doc.element(sel).unwrap().selected_index, 1);
        assert_eq!(doc.element(sel).unwrap().value, "two");
        let (idx, label) = doc.cycle_select(sel).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(label, "Three");
        assert_eq!(doc.element(sel).unwrap().value, "three");
        let (idx, label) = doc.cycle_select(sel).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(label, "One");
    }

    #[test]
    fn set_inner_html_re_parses_children() {
        let mut doc = DomDocument::parse("<div id=\"box\">Old</div>");
        let box_id = doc.get_element_by_id("box").unwrap();
        doc.set_inner_html(box_id, "<span class=\"hi\">New</span><button id=\"b2\">OK</button>");
        assert_eq!(doc.text_content(box_id), "NewOK");
        let b2 = doc.get_element_by_id("b2").unwrap();
        assert_eq!(doc.text_content(b2), "OK");
    }
}
