//! Pure Ratatui lossless text layout primitives: styled wrapping, wrapped tables,
//! CJK display-width padding, and cursor-following single-line input windowing.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Prefix spans prepended to the first and continuation lines of [`wrap_styled_spans`].
#[derive(Debug, Clone, Copy, Default)]
pub struct WrapPrefix<'a> {
    pub first: &'a [Span<'static>],
    pub cont: &'a [Span<'static>],
}

impl<'a> WrapPrefix<'a> {
    pub const fn empty() -> Self {
        Self {
            first: &[],
            cont: &[],
        }
    }
}

/// Border style for [`render_wrapped_table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableBorder {
    /// Box-drawing grid (`┌─┬─┐` / `│ x │` / `├─┼─┤` / `└─┴─┘`).
    Box,
    /// Lightweight pipe grid (`x │ y` / `─┼─`).
    Pipe,
}

/// Horizontal column alignment for [`render_wrapped_table`] and [`pad_display_width`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlign {
    Left,
    Center,
    Right,
}

/// Specification passed to [`render_wrapped_table`].
#[derive(Debug, Clone)]
pub struct TableSpec<'a> {
    pub headers: &'a [Vec<Span<'static>>],
    pub rows: &'a [Vec<Vec<Span<'static>>>],
    pub alignments: &'a [TableAlign],
    pub border: TableBorder,
    pub prefix: &'a [Span<'static>],
    pub border_style: Style,
    pub header_style: Style,
    pub cell_style: Style,
}

/// Horizontal viewport slice of a single-line editable input keeping the cursor visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowedInput {
    pub before_cursor: String,
    pub after_cursor: String,
    pub clipped_left: bool,
    pub clipped_right: bool,
    pub cursor_col: usize,
}

/// Computes display width of a slice of styled spans.
fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Expands `\r\n` / `\r` to `\n` and `\t` to 4 ASCII spaces.
fn normalize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' => out.push_str("    "),
            _ => out.push(ch),
        }
    }
    out
}

/// Splits a string into grapheme-like clusters so zero-width combining characters,
/// variation selectors, and ZWJ sequences remain attached to their base character.
fn split_clusters(s: &str) -> Vec<&str> {
    let mut clusters = Vec::new();
    let mut indices = s.char_indices().peekable();
    while let Some((start, ch)) = indices.next() {
        let mut end = start + ch.len_utf8();
        let mut after_zwj = ch == '\u{200D}';
        while let Some(&(next_idx, next_ch)) = indices.peek() {
            let next_w = UnicodeWidthChar::width(next_ch).unwrap_or(0);
            if after_zwj {
                // Attach the character immediately following ZWJ as part of the same cluster.
                indices.next();
                end = next_idx + next_ch.len_utf8();
                after_zwj = next_ch == '\u{200D}';
            } else if next_ch == '\u{200D}' {
                indices.next();
                end = next_idx + next_ch.len_utf8();
                after_zwj = true;
            } else if next_w == 0 {
                indices.next();
                end = next_idx + next_ch.len_utf8();
            } else {
                break;
            }
        }
        clusters.push(&s[start..end]);
    }
    clusters
}

/// Clamps overly deep leading indentation in `prefix` so the content budget stays at least 2 cols
/// when `max_width >= 4` (or at least 1 col when `max_width >= 2`).
fn clamp_prefix(prefix: &[Span<'static>], max_width: usize) -> (Vec<Span<'static>>, usize) {
    if prefix.is_empty() || max_width == 0 {
        return (Vec::new(), 0);
    }
    let min_content = if max_width >= 4 {
        2
    } else if max_width >= 2 {
        1
    } else {
        0
    };
    let max_prefix_w = max_width.saturating_sub(min_content);
    let total_w = spans_display_width(prefix);
    if total_w <= max_prefix_w {
        return (prefix.to_vec(), total_w);
    }

    // First pass: trim leading ASCII spaces from the prefix to preserve semantic trailing markers
    // such as `• `, `│ `, `┃ `.
    let mut excess = total_w - max_prefix_w;
    let mut trimmed = Vec::with_capacity(prefix.len());
    let mut still_leading = true;
    for span in prefix {
        if still_leading && excess > 0 {
            let content = span.content.as_ref();
            let leading_spaces = content.bytes().take_while(|&b| b == b' ').count();
            let drop = leading_spaces.min(excess);
            excess -= drop;
            let rest = &content[drop..];
            if !rest.is_empty() {
                trimmed.push(Span::styled(rest.to_string(), span.style));
                if drop < content.len() {
                    still_leading = false;
                }
            }
        } else {
            trimmed.push(span.clone());
        }
    }

    let cur_w = spans_display_width(&trimmed);
    if cur_w <= max_prefix_w {
        return (trimmed, cur_w);
    }

    // Second pass: if still wider than `max_prefix_w`, truncate from the right by cluster width.
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in trimmed {
        let mut buf = String::new();
        for cluster in split_clusters(span.content.as_ref()) {
            let cw = UnicodeWidthStr::width(cluster);
            if used + cw > max_prefix_w {
                break;
            }
            buf.push_str(cluster);
            used += cw;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, span.style));
        }
        if used >= max_prefix_w {
            break;
        }
    }
    let final_w = spans_display_width(&out);
    (out, final_w)
}

fn push_or_merge_span(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.style == style {
            last.content.to_mut().push_str(text);
            return;
        }
    }
    spans.push(Span::styled(text.to_string(), style));
}

fn trim_trailing_ws_after(spans: &mut Vec<Span<'static>>, prefix_len: usize) {
    while spans.len() > prefix_len {
        let Some(last) = spans.last_mut() else {
            break;
        };
        let trimmed = last.content.trim_end_matches(' ');
        if trimmed.len() == last.content.len() {
            break;
        }
        if trimmed.is_empty() {
            spans.pop();
        } else {
            let owned = trimmed.to_string();
            last.content = owned.into();
            break;
        }
    }
}

/// Splits a newline-free string into alternating whitespace runs and non-whitespace tokens.
fn split_word_tokens(s: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut cur_ws: Option<bool> = None;
    for (idx, ch) in s.char_indices() {
        let is_ws = ch.is_whitespace();
        if let Some(prev) = cur_ws {
            if prev != is_ws {
                tokens.push(&s[start..idx]);
                start = idx;
            }
        }
        cur_ws = Some(is_ws);
    }
    if start < s.len() {
        tokens.push(&s[start..]);
    }
    tokens
}

/// Wraps styled spans losslessly to `max_width` display columns while preserving styles,
/// explicit `\n` breaks, `\t` expansion (4 cols), CJK widths, and hanging prefixes.
///
/// Guarantees `line.width() <= max_width` for every returned line (for all `max_width >= 2`,
/// or for `max_width == 1` when no width-2 characters are present; returns empty line for `0`).
pub fn wrap_styled_spans(
    spans: &[Span<'static>],
    max_width: usize,
    prefix: WrapPrefix<'_>,
) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![Line::default()];
    }

    let (first_prefix, first_w) = clamp_prefix(prefix.first, max_width);
    let (cont_prefix, cont_w) = clamp_prefix(prefix.cont, max_width);
    let cont_budget = max_width.saturating_sub(cont_w);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur_spans: Vec<Span<'static>> = first_prefix;
    let mut cur_prefix_len = cur_spans.len();
    let mut cur_w = first_w;
    let mut has_content = false;
    let mut just_soft_wrapped = false;
    let mut saw_explicit_newline = false;

    let emit_line = |out: &mut Vec<Line<'static>>,
                     cur_spans: &mut Vec<Span<'static>>,
                     cur_prefix_len: &mut usize,
                     cur_w: &mut usize,
                     has_content: &mut bool,
                     trim_ws: bool| {
        if trim_ws {
            trim_trailing_ws_after(cur_spans, *cur_prefix_len);
        }
        let finished = std::mem::replace(cur_spans, cont_prefix.clone());
        *cur_prefix_len = cur_spans.len();
        *cur_w = cont_w;
        *has_content = false;
        out.push(Line::from(finished));
    };

    for span in spans {
        let style = span.style;
        let normalized = normalize_text(span.content.as_ref());
        let segments = normalized.split('\n');
        let mut first_seg = true;

        for seg in segments {
            if !first_seg {
                // Explicit newline: always emit the preceding line and start a fresh line.
                emit_line(
                    &mut out,
                    &mut cur_spans,
                    &mut cur_prefix_len,
                    &mut cur_w,
                    &mut has_content,
                    false,
                );
                just_soft_wrapped = false;
                saw_explicit_newline = true;
            }
            first_seg = false;

            for token in split_word_tokens(seg) {
                if token.is_empty() {
                    continue;
                }
                let is_ws = token.chars().all(char::is_whitespace);
                let tok_w = UnicodeWidthStr::width(token);

                if is_ws {
                    if just_soft_wrapped {
                        // Drop whitespace that immediately follows a soft wrap boundary.
                        continue;
                    }
                    if cur_w + tok_w <= max_width {
                        push_or_merge_span(&mut cur_spans, token, style);
                        cur_w += tok_w;
                        has_content = true;
                    } else if has_content {
                        emit_line(
                            &mut out,
                            &mut cur_spans,
                            &mut cur_prefix_len,
                            &mut cur_w,
                            &mut has_content,
                            true,
                        );
                        just_soft_wrapped = true;
                    } else {
                        // Explicit leading whitespace wider than line: fit what we can.
                        let avail = max_width.saturating_sub(cur_w);
                        let mut chunk = String::new();
                        let mut cw_sum = 0usize;
                        for ch in token.chars() {
                            let cw = ch.width().unwrap_or(0);
                            if cw_sum + cw > avail {
                                break;
                            }
                            chunk.push(ch);
                            cw_sum += cw;
                        }
                        if !chunk.is_empty() {
                            push_or_merge_span(&mut cur_spans, &chunk, style);
                            cur_w += cw_sum;
                            has_content = true;
                        }
                    }
                    continue;
                }

                just_soft_wrapped = false;

                // Fast path: whole token fits on the current line.
                if cur_w + tok_w <= max_width {
                    push_or_merge_span(&mut cur_spans, token, style);
                    cur_w += tok_w;
                    has_content = true;
                    continue;
                }

                // Word-first wrap: if token fits on a fresh continuation line, move it whole.
                if has_content && tok_w <= cont_budget {
                    emit_line(
                        &mut out,
                        &mut cur_spans,
                        &mut cur_prefix_len,
                        &mut cur_w,
                        &mut has_content,
                        true,
                    );
                    push_or_merge_span(&mut cur_spans, token, style);
                    cur_w += tok_w;
                    has_content = true;
                    continue;
                }

                // Lossless cluster-by-cluster fallback (handles CJK without spaces, long URLs,
                // and `style.bg.is_some()` chips wider than `content_budget`).
                let mut chunk = String::new();
                for cluster in split_clusters(token) {
                    let next_chunk_w = {
                        let mut probe = String::with_capacity(chunk.len() + cluster.len());
                        probe.push_str(&chunk);
                        probe.push_str(cluster);
                        UnicodeWidthStr::width(probe.as_str())
                    };
                    let avail = max_width.saturating_sub(cur_w);

                    if next_chunk_w > avail {
                        if !chunk.is_empty() {
                            let chunk_w = UnicodeWidthStr::width(chunk.as_str());
                            push_or_merge_span(&mut cur_spans, &chunk, style);
                            cur_w += chunk_w;
                            chunk.clear();
                            has_content = true;
                        }
                        if has_content {
                            emit_line(
                                &mut out,
                                &mut cur_spans,
                                &mut cur_prefix_len,
                                &mut cur_w,
                                &mut has_content,
                                true,
                            );
                        }
                    }

                    chunk.push_str(cluster);
                }

                if !chunk.is_empty() {
                    let chunk_w = UnicodeWidthStr::width(chunk.as_str());
                    push_or_merge_span(&mut cur_spans, &chunk, style);
                    cur_w += chunk_w;
                    has_content = true;
                }
            }
        }
    }

    if has_content || out.is_empty() || saw_explicit_newline {
        if just_soft_wrapped {
            trim_trailing_ws_after(&mut cur_spans, cur_prefix_len);
        }
        out.push(Line::from(cur_spans));
    }

    out
}

/// Pads `text` with ASCII spaces to `target_w` display columns using `unicode_width`
/// according to `align`. Never truncates or panics if `text` is already wider than `target_w`.
pub fn pad_display_width(text: &str, target_w: usize, align: TableAlign) -> String {
    let w = UnicodeWidthStr::width(text);
    let pad = target_w.saturating_sub(w);
    match align {
        TableAlign::Left => format!("{text}{}", " ".repeat(pad)),
        TableAlign::Right => format!("{}{text}", " ".repeat(pad)),
        TableAlign::Center => {
            let left = pad / 2;
            let right = pad.saturating_sub(left);
            format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
        }
    }
}

/// Measures natural single-cell width (max display width across explicit `\n` lines)
/// and detects whether any character in the cell has display width >= 2.
fn measure_cell(spans: &[Span<'static>]) -> (usize, bool) {
    let mut max_line_w = 1usize;
    let mut cur_line_w = 0usize;
    let mut has_wide = false;

    for span in spans {
        let norm = normalize_text(span.content.as_ref());
        let mut first = true;
        for seg in norm.split('\n') {
            if !first {
                max_line_w = max_line_w.max(cur_line_w);
                cur_line_w = 0;
            }
            first = false;
            for cluster in split_clusters(seg) {
                let cw = UnicodeWidthStr::width(cluster);
                if cw >= 2 {
                    has_wide = true;
                }
                cur_line_w += cw;
            }
        }
    }
    max_line_w = max_line_w.max(cur_line_w);
    (max_line_w, has_wide)
}

fn patch_cell_spans(spans: &[Span<'static>], base_style: Style) -> Vec<Span<'static>> {
    spans
        .iter()
        .map(|s| Span::styled(s.content.clone(), base_style.patch(s.style)))
        .collect()
}

fn render_stacked_table_fallback(spec: &TableSpec<'_>, max_width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let cols = spec
        .headers
        .len()
        .max(spec.rows.iter().map(Vec::len).max().unwrap_or(0));
    let (clamped_prefix, prefix_w) = clamp_prefix(spec.prefix, max_width);
    let sep_w = max_width.saturating_sub(prefix_w).clamp(1, 24);

    if spec.rows.is_empty() {
        for header in spec.headers {
            let patched = patch_cell_spans(header, spec.header_style);
            out.extend(wrap_styled_spans(
                &patched,
                max_width,
                WrapPrefix {
                    first: &clamped_prefix,
                    cont: &clamped_prefix,
                },
            ));
        }
        return out;
    }

    for (r_idx, row) in spec.rows.iter().enumerate() {
        if r_idx > 0 {
            let mut sep = clamped_prefix.clone();
            sep.push(Span::styled("─".repeat(sep_w), spec.border_style));
            out.push(Line::from(sep));
        }
        for c in 0..cols {
            let mut combined: Vec<Span<'static>> = Vec::new();
            let has_header = spec
                .headers
                .get(c)
                .is_some_and(|h| h.iter().any(|s| !s.content.trim().is_empty()));
            if has_header {
                if let Some(h) = spec.headers.get(c) {
                    combined.extend(patch_cell_spans(h, spec.header_style));
                    combined.push(Span::styled(": ", spec.border_style));
                }
            }
            if let Some(cell) = row.get(c) {
                combined.extend(patch_cell_spans(cell, spec.cell_style));
            }
            if combined.is_empty() {
                continue;
            }
            out.extend(wrap_styled_spans(
                &combined,
                max_width,
                WrapPrefix {
                    first: &clamped_prefix,
                    cont: &clamped_prefix,
                },
            ));
        }
    }

    out
}

/// Renders a table with multi-line wrapped styled cells and strict total-width enforcement
/// (`line.width() <= max_width`). Automatically falls back to stacked `Header: Value` blocks
/// on extreme narrow viewports so zero cell content is ever lost.
pub fn render_wrapped_table(spec: &TableSpec<'_>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![Line::default()];
    }
    let cols = spec
        .headers
        .len()
        .max(spec.rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return Vec::new();
    }

    let prefix_w = spans_display_width(spec.prefix);
    let border_overhead = match spec.border {
        TableBorder::Box => 3 * cols + 1,
        TableBorder::Pipe => 3 * cols.saturating_sub(1),
    };

    let mut natural = vec![1usize; cols];
    let mut min_col = vec![1usize; cols];
    for (c, cell) in spec.headers.iter().enumerate() {
        let (w, wide) = measure_cell(cell);
        natural[c] = natural[c].max(w);
        if wide {
            min_col[c] = 2;
        }
    }
    for row in spec.rows {
        for (c, cell) in row.iter().enumerate().take(cols) {
            let (w, wide) = measure_cell(cell);
            natural[c] = natural[c].max(w);
            if wide {
                min_col[c] = 2;
            }
        }
    }

    let min_cells_needed: usize = min_col.iter().sum();
    if max_width < prefix_w + border_overhead + min_cells_needed {
        return render_stacked_table_fallback(spec, max_width);
    }

    let avail_cells = max_width - prefix_w - border_overhead;
    let mut col_widths = natural.clone();
    let sum_natural: usize = col_widths.iter().sum();

    if sum_natural > avail_cells {
        let dynamic_floor = (avail_cells / cols).clamp(1, 3);
        for c in 0..cols {
            let floor = dynamic_floor.max(min_col[c]);
            col_widths[c] = ((natural[c] * avail_cells) / sum_natural).max(floor);
        }

        // Shave widest columns above their minimum until exact fit.
        while col_widths.iter().sum::<usize>() > avail_cells {
            let Some((idx, _)) = col_widths
                .iter()
                .enumerate()
                .filter(|&(c, &w)| w > min_col[c])
                .max_by_key(|&(_, &w)| w)
            else {
                break;
            };
            col_widths[idx] -= 1;
        }

        // Re-allocate any rounding slack to columns that were shrunk below natural width.
        while col_widths.iter().sum::<usize>() < avail_cells {
            let Some((idx, _)) = col_widths
                .iter()
                .enumerate()
                .filter(|&(c, &w)| w < natural[c])
                .max_by_key(|&(c, &w)| natural[c] - w)
            else {
                break;
            };
            col_widths[idx] += 1;
        }
    }

    let mut out = Vec::new();

    if spec.border == TableBorder::Box {
        let mut top = spec.prefix.to_vec();
        top.push(Span::styled("┌", spec.border_style));
        for (c, &w) in col_widths.iter().enumerate() {
            if c > 0 {
                top.push(Span::styled("┬", spec.border_style));
            }
            top.push(Span::styled("─".repeat(w + 2), spec.border_style));
        }
        top.push(Span::styled("┐", spec.border_style));
        out.push(Line::from(top));
    }

    let render_row = |cells: &[Vec<Span<'static>>],
                      default_style: Style,
                      out: &mut Vec<Line<'static>>| {
        let mut wrapped_cols: Vec<Vec<Line<'static>>> = Vec::with_capacity(cols);
        for (c, &col_w) in col_widths.iter().enumerate() {
            let cell_spans = cells
                .get(c)
                .map(|s| patch_cell_spans(s, default_style))
                .unwrap_or_default();
            let lines = if cell_spans.is_empty() {
                vec![Line::default()]
            } else {
                wrap_styled_spans(&cell_spans, col_w, WrapPrefix::empty())
            };
            wrapped_cols.push(lines);
        }

        let row_height = wrapped_cols.iter().map(Vec::len).max().unwrap_or(1);
        for sub_row in 0..row_height {
            let mut spans = spec.prefix.to_vec();
            if spec.border == TableBorder::Box {
                spans.push(Span::styled("│ ", spec.border_style));
            }
            for (c, &col_w) in col_widths.iter().enumerate() {
                if c > 0 {
                    spans.push(Span::styled(" │ ", spec.border_style));
                }
                let align = spec
                    .alignments
                    .get(c)
                    .copied()
                    .unwrap_or(TableAlign::Left);
                let cell_line = wrapped_cols[c].get(sub_row);
                let cell_w = cell_line.map(Line::width).unwrap_or(0);
                let pad = col_w.saturating_sub(cell_w);
                let (left_pad, right_pad) = match align {
                    TableAlign::Left => (0, pad),
                    TableAlign::Right => (pad, 0),
                    TableAlign::Center => {
                        let l = pad / 2;
                        (l, pad.saturating_sub(l))
                    }
                };
                if left_pad > 0 {
                    spans.push(Span::styled(" ".repeat(left_pad), default_style));
                }
                if let Some(line) = cell_line {
                    spans.extend(line.spans.iter().cloned());
                }
                if right_pad > 0 {
                    spans.push(Span::styled(" ".repeat(right_pad), default_style));
                }
            }
            if spec.border == TableBorder::Box {
                spans.push(Span::styled(" │", spec.border_style));
            }
            out.push(Line::from(spans));
        }
    };

    if !spec.headers.is_empty() {
        render_row(spec.headers, spec.header_style, &mut out);
        match spec.border {
            TableBorder::Box => {
                let mut mid = spec.prefix.to_vec();
                mid.push(Span::styled("├", spec.border_style));
                for (c, &w) in col_widths.iter().enumerate() {
                    if c > 0 {
                        mid.push(Span::styled("┼", spec.border_style));
                    }
                    mid.push(Span::styled("─".repeat(w + 2), spec.border_style));
                }
                mid.push(Span::styled("┤", spec.border_style));
                out.push(Line::from(mid));
            }
            TableBorder::Pipe => {
                let mut mid = spec.prefix.to_vec();
                for (c, &w) in col_widths.iter().enumerate() {
                    if c > 0 {
                        mid.push(Span::styled("─┼─", spec.border_style));
                    }
                    mid.push(Span::styled("─".repeat(w), spec.border_style));
                }
                out.push(Line::from(mid));
            }
        }
    }

    for row in spec.rows {
        render_row(row, spec.cell_style, &mut out);
    }

    if spec.border == TableBorder::Box {
        let mut bot = spec.prefix.to_vec();
        bot.push(Span::styled("└", spec.border_style));
        for (c, &w) in col_widths.iter().enumerate() {
            if c > 0 {
                bot.push(Span::styled("┴", spec.border_style));
            }
            bot.push(Span::styled("─".repeat(w + 2), spec.border_style));
        }
        bot.push(Span::styled("┘", spec.border_style));
        out.push(Line::from(bot));
    }

    out
}

/// Takes the longest suffix of `clusters` whose total display width is `<= budget`.
fn take_suffix_clusters(clusters: &[&str], budget: usize) -> (String, usize) {
    let mut selected = Vec::new();
    let mut used = 0usize;
    for &cl in clusters.iter().rev() {
        let w = UnicodeWidthStr::width(cl);
        if used + w > budget {
            break;
        }
        selected.push(cl);
        used += w;
    }
    selected.reverse();
    let mut out = String::new();
    for cl in selected {
        out.push_str(cl);
    }
    (out, used)
}

/// Takes the longest prefix of `clusters` whose total display width is `<= budget`.
fn take_prefix_clusters(clusters: &[&str], budget: usize) -> (String, usize) {
    let mut out = String::new();
    let mut used = 0usize;
    for &cl in clusters {
        let w = UnicodeWidthStr::width(cl);
        if used + w > budget {
            break;
        }
        out.push_str(cl);
        used += w;
    }
    (out, used)
}

/// Computes a horizontal single-line viewport slice around `cursor_byte` (snapped safely to a
/// UTF-8 character boundary) such that the cursor (`▏`, width 1) and any `‹`/`›` overflow
/// indicators fit strictly inside `viewport_width` without splitting width-2 CJK characters.
pub fn windowed_single_line_input(
    text: &str,
    cursor_byte: usize,
    viewport_width: usize,
) -> WindowedInput {
    if viewport_width == 0 {
        return WindowedInput {
            before_cursor: String::new(),
            after_cursor: String::new(),
            clipped_left: !text.is_empty(),
            clipped_right: false,
            cursor_col: 0,
        };
    }

    let mut snapped = cursor_byte.min(text.len());
    while snapped > 0 && !text.is_char_boundary(snapped) {
        snapped -= 1;
    }

    let left_str = &text[..snapped];
    let right_str = &text[snapped..];
    let left_clusters = split_clusters(left_str);
    let right_clusters = split_clusters(right_str);
    let left_w = UnicodeWidthStr::width(left_str);
    let right_w = UnicodeWidthStr::width(right_str);

    // Reserve 1 column for cursor `▏`.
    let text_budget = viewport_width.saturating_sub(1);

    if left_w + right_w <= text_budget {
        return WindowedInput {
            before_cursor: left_str.to_string(),
            after_cursor: right_str.to_string(),
            clipped_left: false,
            clipped_right: false,
            cursor_col: left_w,
        };
    }

    if text_budget == 0 {
        return WindowedInput {
            before_cursor: String::new(),
            after_cursor: String::new(),
            clipped_left: false,
            clipped_right: false,
            cursor_col: 0,
        };
    }

    // Left is empty -> only right overflows.
    if left_w == 0 {
        let right_budget = text_budget.saturating_sub(1);
        let (after_cursor, _) = take_prefix_clusters(&right_clusters, right_budget);
        return WindowedInput {
            before_cursor: String::new(),
            after_cursor,
            clipped_left: false,
            clipped_right: true,
            cursor_col: 0,
        };
    }

    // Right is empty -> only left overflows.
    if right_w == 0 {
        let left_budget = text_budget.saturating_sub(1);
        let (before_cursor, used_l) = take_suffix_clusters(&left_clusters, left_budget);
        return WindowedInput {
            before_cursor,
            after_cursor: String::new(),
            clipped_left: true,
            clipped_right: false,
            cursor_col: 1 + used_l,
        };
    }

    // Both sides non-empty: check if left fits unclipped with 1 col reserved for `›`.
    if left_w < text_budget {
        let right_budget = text_budget - 1 - left_w;
        let (after_cursor, _) = take_prefix_clusters(&right_clusters, right_budget);
        return WindowedInput {
            before_cursor: left_str.to_string(),
            after_cursor,
            clipped_left: false,
            clipped_right: true,
            cursor_col: left_w,
        };
    }

    // Check if right fits unclipped with 1 col reserved for `‹`.
    if right_w < text_budget {
        let left_budget = text_budget - 1 - right_w;
        let (before_cursor, used_l) = take_suffix_clusters(&left_clusters, left_budget);
        return WindowedInput {
            before_cursor,
            after_cursor: right_str.to_string(),
            clipped_left: true,
            clipped_right: false,
            cursor_col: 1 + used_l,
        };
    }

    // Both sides clip: reserve 1 col for `‹` and 1 col for `›` when space permits.
    if text_budget < 2 {
        return WindowedInput {
            before_cursor: String::new(),
            after_cursor: String::new(),
            clipped_left: true,
            clipped_right: false,
            cursor_col: 1,
        };
    }

    let inner_budget = text_budget - 2;
    let left_target = (inner_budget * 2) / 3;
    let (mut before_cursor, mut used_l) = take_suffix_clusters(&left_clusters, left_target);
    let right_target = inner_budget.saturating_sub(used_l);
    let (after_cursor, used_r) = take_prefix_clusters(&right_clusters, right_target);

    // Give any leftover width (e.g., from a wide CJK boundary on the right) back to the left side.
    let remaining = inner_budget.saturating_sub(used_l + used_r);
    if remaining > 0 {
        let (expanded_left, expanded_w) =
            take_suffix_clusters(&left_clusters, used_l + remaining);
        before_cursor = expanded_left;
        used_l = expanded_w;
    }

    WindowedInput {
        before_cursor,
        after_cursor,
        clipped_left: true,
        clipped_right: true,
        cursor_col: 1 + used_l,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn lines_to_text(lines: &[Line<'_>]) -> String {
        lines
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

    fn normalize_visible(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn wrap_styled_spans_lossless_across_widths() {
        let korean_80 = "가나다라마바사아자차카타파하가나다라마바사아자차카타파하가나다라마바사아자차카타파하가나다라마바사아자차카타파하가나다라마바사아자차카타파하가나다라마바사아자차";
        assert!(korean_80.chars().count() >= 80);
        let long_url = "https://example.com/very/long/path/segment/alpha/beta/gamma/delta/epsilon/zeta/eta/theta/iota";
        assert!(long_url.len() >= 90);
        let chip_45 = "inline_code_chip_with_background_color_123456";
        assert!(chip_45.len() >= 45);
        let combining_emoji = "cafe\u{0301} 🚀 한글끝";

        let input_spans = vec![
            Span::raw(format!("{korean_80} ")),
            Span::styled(long_url.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled(
                chip_45.to_string(),
                Style::default().fg(Color::Yellow).bg(Color::DarkGray),
            ),
            Span::raw(format!("\r\n\tindented_after_crlf\n{combining_emoji}")),
        ];

        let expected_visible = normalize_visible(
            &input_spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
        );

        let first_p = [Span::styled("• ", Style::default().fg(Color::Cyan))];
        let cont_p = [Span::raw("  ")];
        let prefix = WrapPrefix {
            first: &first_p,
            cont: &cont_p,
        };

        for width in [16usize, 26, 40, 86, 88, 160] {
            let wrapped = wrap_styled_spans(&input_spans, width, prefix);
            assert!(!wrapped.is_empty());
            for (i, line) in wrapped.iter().enumerate() {
                assert!(
                    line.width() <= width,
                    "width={width} line {i} exceeded width: {} > {width} ({:?})",
                    line.width(),
                    line
                );
            }

            // Strip leading prefix from each line to verify lossless text preservation.
            let stripped: Vec<String> = wrapped
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
                    if i == 0 {
                        s.strip_prefix("• ").unwrap_or(&s).to_string()
                    } else {
                        s.strip_prefix("  ").unwrap_or(&s).to_string()
                    }
                })
                .collect();

            // Verify explicit `\r\n` and `\n` place text on distinct lines (use <=8-col markers
            // that fit unsplit even at `width = 16` with 2-col prefix + 4-col tab indent).
            let idx_indent = stripped
                .iter()
                .position(|l| l.contains("indented"))
                .expect("indented line missing");
            let idx_emoji = stripped
                .iter()
                .position(|l| l.contains("cafe\u{0301}"))
                .expect("combining emoji line missing");
            assert!(
                idx_emoji > idx_indent,
                "explicit newline must separate lines at width={width}"
            );

            // Rejoin wrapped fragments and verify every non-whitespace character is preserved.
            let joined_no_ws: String = stripped
                .join("")
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            let expected_no_ws: String = expected_visible
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            assert_eq!(
                joined_no_ws, expected_no_ws,
                "lossless invariant failed at width={width}"
            );
        }
    }

    #[test]
    fn render_wrapped_table_width_and_lossless_invariant() {
        let prefix = [Span::raw(" ")];

        // 2-column, 4-column, and 8-column Korean + English + embedded newline tables.
        let headers_2 = vec![vec![Span::raw("항목")], vec![Span::raw("상세 설명")]];
        let rows_2 = vec![
            vec![
                vec![Span::raw("첫번째")],
                vec![Span::raw("첫줄 내용\n둘째줄 상세한 한글 텍스트와 English words")],
            ],
            vec![
                vec![Span::raw("Alpha")],
                vec![Span::raw("Second row with multiple words and 긴한글단어")],
            ],
        ];

        let headers_4 = vec![
            vec![Span::raw("이름")],
            vec![Span::raw("상태")],
            vec![Span::raw("담당자")],
            vec![Span::raw("메모")],
        ];
        let rows_4 = vec![
            vec![
                vec![Span::raw("배포작업")],
                vec![Span::raw("진행중")],
                vec![Span::raw("홍길동")],
                vec![Span::raw("라인1\n라인2 긴 메모 설명")],
            ],
            vec![
                vec![Span::raw("Release")],
                vec![Span::raw("Done")],
                vec![Span::raw("Alice")],
                vec![Span::raw("Verified all checks")],
            ],
        ];

        let headers_8: Vec<Vec<Span<'static>>> = (1..=8)
            .map(|i| vec![Span::raw(format!("Col{i}"))])
            .collect();
        let rows_8 = vec![vec![
            vec![Span::raw("값1")],
            vec![Span::raw("값2")],
            vec![Span::raw("val3")],
            vec![Span::raw("val4")],
            vec![Span::raw("값5")],
            vec![Span::raw("val6")],
            vec![Span::raw("값7")],
            vec![Span::raw("끝값8")],
        ]];

        let cases = [
            (&headers_2, &rows_2, vec!["항목", "첫번째", "첫줄", "둘째줄", "Alpha", "긴한글단어"]),
            (&headers_4, &rows_4, vec!["이름", "배포작업", "홍길동", "라인1", "라인2", "Release", "Verified"]),
            (&headers_8, &rows_8, vec!["Col1", "Col8", "값1", "val4", "끝값8"]),
        ];

        for (headers, rows, must_contain) in cases {
            for border in [TableBorder::Box, TableBorder::Pipe] {
                for width in [24usize, 40, 86, 88, 160] {
                    let spec = TableSpec {
                        headers,
                        rows,
                        alignments: &[],
                        border,
                        prefix: &prefix,
                        border_style: Style::default().fg(Color::DarkGray),
                        header_style: Style::default().fg(Color::Cyan),
                        cell_style: Style::default().fg(Color::White),
                    };
                    let lines = render_wrapped_table(&spec, width);
                    assert!(!lines.is_empty());
                    for (i, line) in lines.iter().enumerate() {
                        assert!(
                            line.width() <= width,
                            "border={border:?} cols={} width={width} line {i} overflowed: {} > {width}",
                            headers.len(),
                            line.width()
                        );
                    }

                    // Check lossless token presence: in grid mode reconstruct each column vertically
                    // across wrapped sub-rows; in stacked fallback join lines directly.
                    let is_grid = lines
                        .iter()
                        .any(|l| l.to_string().contains('│'));
                    let reconstructed = if is_grid {
                        let mut per_col: Vec<String> = vec![String::new(); headers.len()];
                        for l in &lines {
                            let s = l.to_string();
                            if !s.contains('│') {
                                continue;
                            }
                            let parts: Vec<&str> = s.split('│').collect();
                            let slice = if border == TableBorder::Box && parts.len() >= headers.len() + 2 {
                                &parts[1..1 + headers.len()]
                            } else {
                                &parts[..parts.len().min(headers.len())]
                            };
                            for (c, seg) in slice.iter().enumerate() {
                                per_col[c].extend(seg.chars().filter(|ch| !ch.is_whitespace()));
                            }
                        }
                        per_col.join("\n")
                    } else {
                        lines_to_text(&lines)
                            .chars()
                            .filter(|c| !c.is_whitespace() && *c != '─')
                            .collect()
                    };
                    for tok in &must_contain {
                        assert!(
                            reconstructed.contains(tok),
                            "missing token '{tok}' at border={border:?} cols={} width={width}; output:\n{}",
                            headers.len(),
                            lines_to_text(&lines)
                        );
                    }

                    // In Box mode (when not in stacked fallback), every row's `│` column positions must match identically.
                    if border == TableBorder::Box {
                        let pipe_cols: Vec<Vec<usize>> = lines
                            .iter()
                            .filter_map(|l| {
                                let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                                if !text.contains('│') {
                                    return None;
                                }
                                let mut cols = Vec::new();
                                let mut col = 0usize;
                                for ch in text.chars() {
                                    if ch == '│' {
                                        cols.push(col);
                                    }
                                    col += ch.width().unwrap_or(0);
                                }
                                Some(cols)
                            })
                            .collect();
                        if pipe_cols.len() >= 2 {
                            for cols_row in &pipe_cols[1..] {
                                assert_eq!(
                                    cols_row, &pipe_cols[0],
                                    "Box vertical borders misaligned between CJK/ASCII rows at width={width}:\n{}",
                                    lines_to_text(&lines)
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn windowed_single_line_input_cursor_always_visible() {
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!("한글{i}_abc"));
        }
        assert!(text.chars().count() >= 120);

        // Pick a mid-Korean interior byte (intentionally non-boundary to test UTF-8 snapping)
        // as well as 0 and text.len().
        let mid_korean_interior = text.find('글').unwrap() + 1;
        assert!(!text.is_char_boundary(mid_korean_interior));

        for cursor in [0usize, mid_korean_interior, text.len() / 2, text.len()] {
            for vp in [16usize, 30, 80] {
                let w = windowed_single_line_input(&text, cursor, vp);
                let rendered = format!(
                    "{}{}▏{}{}",
                    if w.clipped_left { "‹" } else { "" },
                    w.before_cursor,
                    w.after_cursor,
                    if w.clipped_right { "›" } else { "" }
                );
                let total_w = UnicodeWidthStr::width(rendered.as_str());
                assert!(
                    total_w <= vp,
                    "cursor={cursor} vp={vp} total_w={total_w} > vp ({rendered:?})"
                );
                assert!(rendered.contains('▏'));
                assert!(
                    w.cursor_col < vp,
                    "cursor_col={} must be < vp={vp}",
                    w.cursor_col
                );
                let expected_cursor_col = (if w.clipped_left { 1 } else { 0 })
                    + UnicodeWidthStr::width(w.before_cursor.as_str());
                assert_eq!(w.cursor_col, expected_cursor_col);
            }
        }
    }

    #[test]
    fn pad_display_width_handles_cjk_accurately() {
        let padded = pad_display_width("한글", 6, TableAlign::Left);
        assert_eq!(padded, "한글  ");
        assert_eq!(UnicodeWidthStr::width(padded.as_str()), 6);

        let right = pad_display_width("한글", 6, TableAlign::Right);
        assert_eq!(right, "  한글");

        let center = pad_display_width("한글", 7, TableAlign::Center);
        assert_eq!(center, " 한글  ");
        assert_eq!(UnicodeWidthStr::width(center.as_str()), 7);
    }
}
