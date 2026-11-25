//! Pure-Rust HTML5 `<canvas>` 2D rendering surface backed by `resvg::tiny_skia`.

use std::f32::consts::{PI, TAU};

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use resvg::tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke,
    StrokeDash, Transform,
};
use resvg::usvg;
use unicode_width::UnicodeWidthStr;

use crate::graphics::{shared_fontdb, MAX_RASTER_DIM_PX};

/// Default HTML5 `<canvas>` width in pixels.
pub const DEFAULT_CANVAS_WIDTH: u32 = 300;
/// Default HTML5 `<canvas>` height in pixels.
pub const DEFAULT_CANVAS_HEIGHT: u32 = 150;

#[derive(Debug, Clone)]
struct CanvasState {
    fill_style_str: String,
    stroke_style_str: String,
    fill_rgba: [u8; 4],
    stroke_rgba: [u8; 4],
    line_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    global_alpha: f32,
    line_dash: Vec<f32>,
    transform: Transform,
    font: String,
    text_align: String,
    text_baseline: String,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            fill_style_str: "#000000".to_string(),
            stroke_style_str: "#000000".to_string(),
            fill_rgba: [0, 0, 0, 255],
            stroke_rgba: [0, 0, 0, 255],
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            global_alpha: 1.0,
            line_dash: Vec::new(),
            transform: Transform::identity(),
            font: "10px sans-serif".to_string(),
            text_align: "start".to_string(),
            text_baseline: "alphabetic".to_string(),
        }
    }
}

/// Lazily allocated 2D `<canvas>` backing store with `tiny-skia` rasterization.
pub struct CanvasSurface {
    pixmap: Pixmap,
    pub has_drawn: bool,
    state: CanvasState,
    state_stack: Vec<CanvasState>,
    path_builder: PathBuilder,
    has_subpath: bool,
}

impl std::fmt::Debug for CanvasSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasSurface")
            .field("width", &self.pixmap.width())
            .field("height", &self.pixmap.height())
            .field("has_drawn", &self.has_drawn)
            .finish()
    }
}

impl CanvasSurface {
    /// Creates a transparent canvas surface clamped to `1..=2048` on each axis.
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.clamp(1, MAX_RASTER_DIM_PX);
        let h = height.clamp(1, MAX_RASTER_DIM_PX);
        let pixmap = Pixmap::new(w, h).expect("clamped canvas pixmap allocation");
        Self {
            pixmap,
            has_drawn: false,
            state: CanvasState::default(),
            state_stack: Vec::new(),
            path_builder: PathBuilder::new(),
            has_subpath: false,
        }
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// Resizes the canvas buffer, clearing pixels and resetting 2D state per HTML5 spec.
    pub fn resize(&mut self, width: u32, height: u32) {
        let w = width.clamp(1, MAX_RASTER_DIM_PX);
        let h = height.clamp(1, MAX_RASTER_DIM_PX);
        if let Some(pm) = Pixmap::new(w, h) {
            self.pixmap = pm;
        }
        self.has_drawn = false;
        self.state = CanvasState::default();
        self.state_stack.clear();
        self.path_builder = PathBuilder::new();
        self.has_subpath = false;
    }

    pub fn fill_style(&self) -> &str {
        &self.state.fill_style_str
    }

    pub fn stroke_style(&self) -> &str {
        &self.state.stroke_style_str
    }

    pub fn set_fill_style(&mut self, css: &str) {
        if let Some(rgba) = parse_canvas_color(css) {
            self.state.fill_rgba = rgba;
            self.state.fill_style_str = css.trim().to_string();
        }
    }

    pub fn set_stroke_style(&mut self, css: &str) {
        if let Some(rgba) = parse_canvas_color(css) {
            self.state.stroke_rgba = rgba;
            self.state.stroke_style_str = css.trim().to_string();
        }
    }

    pub fn line_width(&self) -> f32 {
        self.state.line_width
    }

    pub fn set_line_width(&mut self, w: f32) {
        if w.is_finite() && w > 0.0 {
            self.state.line_width = w;
        }
    }

    pub fn set_line_cap(&mut self, cap: &str) {
        self.state.line_cap = match cap.trim().to_ascii_lowercase().as_str() {
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => LineCap::Butt,
        };
    }

    pub fn set_line_join(&mut self, join: &str) {
        self.state.line_join = match join.trim().to_ascii_lowercase().as_str() {
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Miter,
        };
    }

    pub fn global_alpha(&self) -> f32 {
        self.state.global_alpha
    }

    pub fn set_global_alpha(&mut self, alpha: f32) {
        if alpha.is_finite() && (0.0..=1.0).contains(&alpha) {
            self.state.global_alpha = alpha;
        }
    }

    pub fn set_line_dash(&mut self, segments: Vec<f32>) {
        if segments.iter().all(|v| v.is_finite() && *v >= 0.0) {
            if segments.len() % 2 == 1 {
                let mut doubled = segments.clone();
                doubled.extend(segments);
                self.state.line_dash = doubled;
            } else {
                self.state.line_dash = segments;
            }
        }
    }

    pub fn set_font(&mut self, font: &str) {
        let trimmed = font.trim();
        if !trimmed.is_empty() {
            self.state.font = trimmed.to_string();
        }
    }

    pub fn font(&self) -> &str {
        &self.state.font
    }

    pub fn set_text_align(&mut self, align: &str) {
        let lower = align.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "left" | "right" | "center" | "start" | "end") {
            self.state.text_align = lower;
        }
    }

    pub fn set_text_baseline(&mut self, baseline: &str) {
        let lower = baseline.trim().to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "top" | "hanging" | "middle" | "alphabetic" | "ideographic" | "bottom"
        ) {
            self.state.text_baseline = lower;
        }
    }

    pub fn save(&mut self) {
        if self.state_stack.len() < 64 {
            self.state_stack.push(self.state.clone());
        }
    }

    pub fn restore(&mut self) {
        if let Some(prev) = self.state_stack.pop() {
            self.state = prev;
        }
    }

    pub fn translate(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.state.transform = self.state.transform.pre_translate(x, y);
        }
    }

    pub fn scale(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.state.transform = self.state.transform.pre_scale(x, y);
        }
    }

    pub fn rotate(&mut self, radians: f32) {
        if radians.is_finite() {
            self.state.transform = self.state.transform.pre_rotate(radians.to_degrees());
        }
    }

    pub fn set_transform(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        if [a, b, c, d, e, f].iter().all(|v| v.is_finite()) {
            self.state.transform = Transform::from_row(a, b, c, d, e, f);
        }
    }

    pub fn reset_transform(&mut self) {
        self.state.transform = Transform::identity();
    }

    fn make_paint(&self, rgba: [u8; 4]) -> Paint<'static> {
        let mut paint = Paint::default();
        let alpha = ((rgba[3] as f32 / 255.0) * self.state.global_alpha).clamp(0.0, 1.0);
        paint.set_color(Color::from_rgba(
            rgba[0] as f32 / 255.0,
            rgba[1] as f32 / 255.0,
            rgba[2] as f32 / 255.0,
            alpha,
        ).unwrap_or(Color::BLACK));
        paint.anti_alias = true;
        paint
    }

    fn make_stroke(&self) -> Stroke {
        let mut stroke = Stroke {
            width: self.state.line_width,
            line_cap: self.state.line_cap,
            line_join: self.state.line_join,
            ..Stroke::default()
        };
        if !self.state.line_dash.is_empty() && self.state.line_dash.iter().any(|&v| v > 0.0) {
            stroke.dash = StrokeDash::new(self.state.line_dash.clone(), 0.0);
        }
        stroke
    }

    fn map_pt(&self, x: f32, y: f32) -> (f32, f32) {
        let mut pt = resvg::tiny_skia::Point::from_xy(x, y);
        self.state.transform.map_point(&mut pt);
        (pt.x, pt.y)
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(rect) = normalize_rect(x, y, w, h) else {
            return;
        };
        let Some(path) = PathBuilder::from_rect(rect).transform(self.state.transform) else {
            return;
        };
        let paint = self.make_paint(self.state.fill_rgba);
        self.pixmap
            .fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        self.has_drawn = true;
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(rect) = normalize_rect(x, y, w, h) else {
            return;
        };
        let path = PathBuilder::from_rect(rect);
        let paint = self.make_paint(self.state.stroke_rgba);
        let stroke = self.make_stroke();
        self.pixmap
            .stroke_path(&path, &paint, &stroke, self.state.transform, None);
        self.has_drawn = true;
    }

    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let Some(rect) = normalize_rect(x, y, w, h) else {
            return;
        };
        let Some(path) = PathBuilder::from_rect(rect).transform(self.state.transform) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(Color::TRANSPARENT);
        paint.blend_mode = BlendMode::Clear;
        self.pixmap
            .fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    pub fn begin_path(&mut self) {
        self.path_builder = PathBuilder::new();
        self.has_subpath = false;
    }

    pub fn close_path(&mut self) {
        if self.has_subpath {
            self.path_builder.close();
        }
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let (tx, ty) = self.map_pt(x, y);
        self.path_builder.move_to(tx, ty);
        self.has_subpath = true;
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let (tx, ty) = self.map_pt(x, y);
        if !self.has_subpath {
            self.path_builder.move_to(tx, ty);
            self.has_subpath = true;
        } else {
            self.path_builder.line_to(tx, ty);
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.close_path();
    }

    pub fn quadratic_curve_to(&mut self, cpx: f32, cpy: f32, x: f32, y: f32) {
        if ![cpx, cpy, x, y].iter().all(|v| v.is_finite()) {
            return;
        }
        let (tcx, tcy) = self.map_pt(cpx, cpy);
        let (tx, ty) = self.map_pt(x, y);
        if !self.has_subpath {
            self.path_builder.move_to(tcx, tcy);
            self.has_subpath = true;
        }
        self.path_builder.quad_to(tcx, tcy, tx, ty);
    }

    pub fn bezier_curve_to(
        &mut self,
        cp1x: f32,
        cp1y: f32,
        cp2x: f32,
        cp2y: f32,
        x: f32,
        y: f32,
    ) {
        if ![cp1x, cp1y, cp2x, cp2y, x, y].iter().all(|v| v.is_finite()) {
            return;
        }
        let (t1x, t1y) = self.map_pt(cp1x, cp1y);
        let (t2x, t2y) = self.map_pt(cp2x, cp2y);
        let (tx, ty) = self.map_pt(x, y);
        if !self.has_subpath {
            self.path_builder.move_to(t1x, t1y);
            self.has_subpath = true;
        }
        self.path_builder.cubic_to(t1x, t1y, t2x, t2y, tx, ty);
    }

    pub fn arc(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        start_angle: f32,
        end_angle: f32,
        counterclockwise: bool,
    ) {
        if radius < 0.0 {
            return;
        }
        self.ellipse(x, y, radius, radius, 0.0, start_angle, end_angle, counterclockwise);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        rotation: f32,
        start_angle: f32,
        end_angle: f32,
        counterclockwise: bool,
    ) {
        if ![cx, cy, rx, ry, rotation, start_angle, end_angle]
            .iter()
            .all(|v| v.is_finite())
            || rx < 0.0
            || ry < 0.0
        {
            return;
        }
        let mut sweep = end_angle - start_angle;
        if counterclockwise {
            if sweep <= -TAU {
                sweep = -TAU;
            } else {
                while sweep >= 0.0 {
                    sweep -= TAU;
                }
            }
        } else if sweep >= TAU {
            sweep = TAU;
        } else {
            while sweep <= 0.0 {
                sweep += TAU;
            }
        }

        let steps = ((sweep.abs() / (PI / 24.0)).ceil() as usize).clamp(8, 128);
        let cos_r = rotation.cos();
        let sin_r = rotation.sin();

        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let angle = start_angle + sweep * t;
            let lx = rx * angle.cos();
            let ly = ry * angle.sin();
            let px = cx + lx * cos_r - ly * sin_r;
            let py = cy + lx * sin_r + ly * cos_r;
            if i == 0 && !self.has_subpath {
                self.move_to(px, py);
            } else {
                self.line_to(px, py);
            }
        }
    }

    pub fn fill(&mut self, even_odd: bool) {
        let Some(path) = self.path_builder.clone().finish() else {
            return;
        };
        let paint = self.make_paint(self.state.fill_rgba);
        let rule = if even_odd {
            FillRule::EvenOdd
        } else {
            FillRule::Winding
        };
        self.pixmap
            .fill_path(&path, &paint, rule, Transform::identity(), None);
        self.has_drawn = true;
    }

    pub fn stroke(&mut self) {
        let Some(path) = self.path_builder.clone().finish() else {
            return;
        };
        let paint = self.make_paint(self.state.stroke_rgba);
        let stroke = self.make_stroke();
        self.pixmap
            .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        self.has_drawn = true;
    }

    pub fn fill_text(&mut self, text: &str, x: f32, y: f32) {
        self.render_svg_text(text, x, y, true);
    }

    pub fn stroke_text(&mut self, text: &str, x: f32, y: f32) {
        self.render_svg_text(text, x, y, false);
    }

    fn render_svg_text(&mut self, text: &str, x: f32, y: f32, is_fill: bool) {
        if text.is_empty() || !x.is_finite() || !y.is_finite() {
            return;
        }
        let parsed_font = parse_css_font(&self.state.font);
        let anchor = match self.state.text_align.as_str() {
            "center" => "middle",
            "right" | "end" => "end",
            _ => "start",
        };
        let baseline_dy = match self.state.text_baseline.as_str() {
            "top" | "hanging" => parsed_font.size_px * 0.85,
            "middle" => parsed_font.size_px * 0.35,
            "bottom" | "ideographic" => -parsed_font.size_px * 0.15,
            _ => 0.0,
        };
        let rgba = if is_fill {
            self.state.fill_rgba
        } else {
            self.state.stroke_rgba
        };
        let color_hex = format!("#{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2]);
        let opacity = ((rgba[3] as f32 / 255.0) * self.state.global_alpha).clamp(0.0, 1.0);
        let paint_attrs = if is_fill {
            format!("fill=\"{color_hex}\" fill-opacity=\"{opacity:.3}\" stroke=\"none\"")
        } else {
            format!(
                "fill=\"none\" stroke=\"{color_hex}\" stroke-opacity=\"{opacity:.3}\" stroke-width=\"{:.2}\"",
                self.state.line_width
            )
        };

        let t = self.state.transform;
        let escaped_text = xml_escape(text);
        let escaped_family = xml_escape(&parsed_font.family);
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\">\
             <text x=\"{:.2}\" y=\"{:.2}\" font-family=\"{}\" font-size=\"{:.2}\" font-weight=\"{}\" font-style=\"{}\" \
             text-anchor=\"{}\" {} transform=\"matrix({:.4} {:.4} {:.4} {:.4} {:.4} {:.4})\">{}</text>\
             </svg>",
            self.pixmap.width(),
            self.pixmap.height(),
            x,
            y + baseline_dy,
            escaped_family,
            parsed_font.size_px,
            parsed_font.weight,
            parsed_font.style,
            anchor,
            paint_attrs,
            t.sx,
            t.ky,
            t.kx,
            t.sy,
            t.tx,
            t.ty,
            escaped_text,
        );

        let opts = usvg::Options {
            fontdb: shared_fontdb(),
            font_family: parsed_font.family,
            ..usvg::Options::default()
        };
        if let Ok(tree) = usvg::Tree::from_data(svg.as_bytes(), &opts) {
            resvg::render(&tree, Transform::identity(), &mut self.pixmap.as_mut());
            self.has_drawn = true;
        }
    }

    pub fn measure_text(&self, text: &str) -> f32 {
        let parsed = parse_css_font(&self.state.font);
        UnicodeWidthStr::width(text) as f32 * parsed.size_px * 0.6
    }

    pub fn encode_png(&self) -> Result<Vec<u8>> {
        self.pixmap
            .encode_png()
            .map_err(|e| anyhow!("canvas PNG encode failed: {e}"))
    }

    pub fn to_data_url(&self) -> Result<String> {
        let png = self.encode_png()?;
        Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
    }
}

fn normalize_rect(x: f32, y: f32, w: f32, h: f32) -> Option<Rect> {
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 {
        return None;
    }
    let (rx, rw) = if w >= 0.0 { (x, w) } else { (x + w, -w) };
    let (ry, rh) = if h >= 0.0 { (y, h) } else { (y + h, -h) };
    Rect::from_xywh(rx, ry, rw, rh)
}

struct ParsedFont {
    size_px: f32,
    weight: &'static str,
    style: &'static str,
    family: String,
}

fn parse_css_font(raw: &str) -> ParsedFont {
    let mut size_px = 10.0f32;
    let mut weight = "normal";
    let mut style = "normal";
    let mut family_parts = Vec::new();
    let mut found_size = false;

    for token in raw.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if !found_size {
            if lower == "bold" || lower == "bolder" || matches!(lower.as_str(), "600" | "700" | "800" | "900") {
                weight = "bold";
                continue;
            }
            if lower == "italic" || lower == "oblique" {
                style = "italic";
                continue;
            }
            if lower == "normal" {
                continue;
            }
            let size_part = lower.split('/').next().unwrap_or(&lower);
            if let Some(num) = size_part.strip_suffix("px").and_then(|n| n.parse::<f32>().ok()) {
                size_px = num.clamp(4.0, 256.0);
                found_size = true;
                continue;
            }
            if let Some(num) = size_part.strip_suffix("pt").and_then(|n| n.parse::<f32>().ok()) {
                size_px = (num * 1.333).clamp(4.0, 256.0);
                found_size = true;
                continue;
            }
            if let Some(num) = size_part.strip_suffix("rem").or_else(|| size_part.strip_suffix("em")).and_then(|n| n.parse::<f32>().ok()) {
                size_px = (num * 16.0).clamp(4.0, 256.0);
                found_size = true;
                continue;
            }
        } else {
            family_parts.push(token);
        }
    }

    let family = if family_parts.is_empty() {
        "sans-serif".to_string()
    } else {
        family_parts
            .join(" ")
            .split(',')
            .next()
            .unwrap_or("sans-serif")
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string()
    };

    ParsedFont {
        size_px,
        weight,
        style,
        family,
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Parses `#hex`, `rgb()`, `rgba()`, `hsl()`, `hsla()`, and standard CSS named colors.
pub fn parse_canvas_color(raw: &str) -> Option<[u8; 4]> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            3 => Some([
                u8::from_str_radix(&hex[0..1], 16).ok()? * 17,
                u8::from_str_radix(&hex[1..2], 16).ok()? * 17,
                u8::from_str_radix(&hex[2..3], 16).ok()? * 17,
                255,
            ]),
            4 => Some([
                u8::from_str_radix(&hex[0..1], 16).ok()? * 17,
                u8::from_str_radix(&hex[1..2], 16).ok()? * 17,
                u8::from_str_radix(&hex[2..3], 16).ok()? * 17,
                u8::from_str_radix(&hex[3..4], 16).ok()? * 17,
            ]),
            6 => Some([
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                255,
            ]),
            8 => Some([
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                u8::from_str_radix(&hex[6..8], 16).ok()?,
            ]),
            _ => None,
        };
    }

    if let Some(inner) = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner
            .split([',', '/', ' '])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() >= 3 {
            let r = parse_rgb_channel(parts[0])?;
            let g = parse_rgb_channel(parts[1])?;
            let b = parse_rgb_channel(parts[2])?;
            let a = if let Some(a_str) = parts.get(3) {
                parse_alpha_channel(a_str)?
            } else {
                255
            };
            return Some([r, g, b, a]);
        }
    }

    if let Some(inner) = s
        .strip_prefix("hsla(")
        .or_else(|| s.strip_prefix("hsl("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner
            .split([',', '/', ' '])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() >= 3 {
            let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
            let sat = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let light = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let a = if let Some(a_str) = parts.get(3) {
                parse_alpha_channel(a_str)?
            } else {
                255
            };
            let (r, g, b) = hsl_to_rgb(h, sat.clamp(0.0, 1.0), light.clamp(0.0, 1.0));
            return Some([r, g, b, a]);
        }
    }

    match s.as_str() {
        "transparent" => Some([0, 0, 0, 0]),
        "black" => Some([0, 0, 0, 255]),
        "white" => Some([255, 255, 255, 255]),
        "red" => Some([255, 0, 0, 255]),
        "crimson" => Some([220, 20, 60, 255]),
        "green" => Some([0, 128, 0, 255]),
        "lime" => Some([0, 255, 0, 255]),
        "blue" => Some([0, 0, 255, 255]),
        "dodgerblue" => Some([30, 144, 255, 255]),
        "steelblue" => Some([70, 130, 180, 255]),
        "navy" => Some([0, 0, 128, 255]),
        "yellow" => Some([255, 255, 0, 255]),
        "gold" => Some([255, 215, 0, 255]),
        "orange" => Some([255, 165, 0, 255]),
        "coral" => Some([255, 127, 80, 255]),
        "cyan" | "aqua" => Some([0, 255, 255, 255]),
        "teal" => Some([0, 128, 128, 255]),
        "magenta" | "fuchsia" => Some([255, 0, 255, 255]),
        "purple" => Some([128, 0, 128, 255]),
        "indigo" => Some([75, 0, 130, 255]),
        "pink" => Some([255, 192, 203, 255]),
        "brown" => Some([165, 42, 42, 255]),
        "gray" | "grey" => Some([128, 128, 128, 255]),
        "lightgray" | "lightgrey" => Some([211, 211, 211, 255]),
        "darkgray" | "darkgrey" => Some([169, 169, 169, 255]),
        "silver" => Some([192, 192, 192, 255]),
        _ => None,
    }
}

fn parse_rgb_channel(s: &str) -> Option<u8> {
    if let Some(pct) = s.strip_suffix('%') {
        let v = pct.parse::<f32>().ok()?;
        Some((v.clamp(0.0, 100.0) * 2.55).round() as u8)
    } else {
        let v = s.parse::<f32>().ok()?;
        Some(v.clamp(0.0, 255.0).round() as u8)
    }
}

fn parse_alpha_channel(s: &str) -> Option<u8> {
    if let Some(pct) = s.strip_suffix('%') {
        let v = pct.parse::<f32>().ok()?;
        Some((v.clamp(0.0, 100.0) * 2.55).round() as u8)
    } else {
        let v = s.parse::<f32>().ok()?;
        Some((v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_draws_rect_arc_text_and_exports_png() {
        let mut surface = CanvasSurface::new(120, 80);
        assert!(!surface.has_drawn);
        surface.set_fill_style("#e11d48");
        surface.fill_rect(0.0, 0.0, 60.0, 80.0);
        surface.set_stroke_style("rgb(0, 128, 255)");
        surface.set_line_width(3.0);
        surface.begin_path();
        surface.arc(90.0, 40.0, 20.0, 0.0, TAU, false);
        surface.stroke();
        surface.set_fill_style("hsl(120, 100%, 40%)");
        surface.set_font("bold 14px sans-serif");
        surface.fill_text("OK", 10.0, 30.0);

        assert!(surface.has_drawn);
        let px = surface.pixmap.pixel(10, 10).unwrap();
        assert!(px.red() > 200);

        let png = surface.encode_png().expect("encode canvas png");
        assert!(png.starts_with(b"\x89PNG"));
        let url = surface.to_data_url().expect("toDataURL");
        assert!(url.starts_with("data:image/png;base64,"));
    }
}
