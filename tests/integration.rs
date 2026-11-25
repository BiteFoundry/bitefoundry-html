//! End-to-end integration tests for `bitefoundry-html`.

#[cfg(feature = "js")]
use bitefoundry_html::HtmlEffect;
use bitefoundry_html::{
    is_safe_raster_image, looks_like_html_bytes, looks_like_svg_bytes, render_raw_html,
    sniff_content_kind, ContentKind, ControlKind, HtmlTheme, HtmlViewSession,
};

fn rendered_text(lines: &[ratatui::text::Line<'_>]) -> String {
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

#[test]
fn integration_sniff_protects_against_spoofed_images() {
    // Valid raster magic
    assert!(is_safe_raster_image(
        Some("image/webp"),
        Some("pic.webp"),
        Some(b"RIFF\x10\x00\x00\x00WEBPVP8 ")
    ));
    // HTML error page disguised as .png
    let html_err = b"   <html><head><title>403</title></head><body>Denied</body></html>";
    assert!(looks_like_html_bytes(html_err));
    assert_eq!(
        sniff_content_kind(Some("image/png"), Some("photo.png"), Some(html_err)),
        ContentKind::Html
    );
    assert!(!is_safe_raster_image(
        Some("image/png"),
        Some("photo.png"),
        Some(html_err)
    ));
    // SVG disguised as .jpg
    let svg = b"<svg width=\"10\" height=\"10\"></svg>";
    assert!(looks_like_svg_bytes(svg));
    assert_eq!(
        sniff_content_kind(Some("image/jpeg"), Some("a.jpg"), Some(svg)),
        ContentKind::Svg
    );
}

#[test]
fn integration_interactive_controls_without_js() {
    let html = r#"
        <h1>Settings</h1>
        <p>Enable notifications: <input id="cb" type="checkbox" name="notify" /></p>
        <p>Theme:
          <input id="r1" type="radio" name="theme" value="dark" checked /> Dark
          <input id="r2" type="radio" name="theme" value="light" /> Light
        </p>
        <p>Region:
          <select id="region" name="region">
            <option value="us">US</option>
            <option value="eu">EU</option>
          </select>
        </p>
        <p><img alt="Chart Preview" src="chart.png" /></p>
        <blockquote><p>Note: all changes are local.</p></blockquote>
        <pre><code>let x = 42;</code></pre>
        <ul><li>One</li><li>Two</li></ul>
        <ol><li>First</li><li>Second</li></ol>
        <hr />
    "#;
    let mut session = HtmlViewSession::new(None, html.into());
    assert!(!session.js_active, "static HTML must not allocate QuickJS");

    let theme = HtmlTheme::light();
    let r0 = session.render(80, &theme, true);
    let t0 = rendered_text(&r0.lines);
    assert!(t0.contains("Settings"));
    assert!(t0.contains("[ ]"));
    assert!(t0.contains("(●)"));
    assert!(t0.contains("( )"));
    assert!(t0.contains("[▾ US]"));
    assert!(t0.contains("[🖼 Chart Preview]"));
    assert!(t0.contains("│ Note: all changes are local."));
    assert!(t0.contains("let x = 42;"));
    assert!(t0.contains("• One"));
    assert!(t0.contains("2. Second"));

    let cb = r0
        .controls
        .iter()
        .find(|c| matches!(c.kind, ControlKind::Checkbox { .. }))
        .unwrap()
        .clone();
    let r2 = r0
        .controls
        .iter()
        .filter(|c| matches!(c.kind, ControlKind::Radio { .. }))
        .nth(1)
        .unwrap()
        .clone();
    let sel = r0
        .controls
        .iter()
        .find(|c| matches!(c.kind, ControlKind::Select { .. }))
        .unwrap()
        .clone();

    // Click checkbox -> [x]
    session.click_at(cb.line, cb.col_start);
    // Click second radio -> unchecks first, checks second
    session.click_at(r2.line, r2.col_start);
    // Click select -> cycles to EU
    session.click_at(sel.line, sel.col_start);

    let r1 = session.render(80, &HtmlTheme::no_color(), false);
    let t1 = rendered_text(&r1.lines);
    assert!(t1.contains("[x]"));
    assert!(t1.contains("[▾ EU]"));

    let raw_lines = render_raw_html(html, 60, &HtmlTheme::dark());
    assert!(rendered_text(&raw_lines).contains("<h1>Settings</h1>"));
}

#[cfg(feature = "js")]
#[test]
fn integration_js_sandbox_dom_mutations_and_limits() {
    // Verify external-only scripts stay at 0 MB idle JS RAM.
    let ext_only = HtmlViewSession::new(
        None,
        r#"<script src="https://example.com/app.js"></script><p>Hi</p>"#.into(),
    );
    assert!(!ext_only.js_active);

    // Verify sandbox isolation (no fetch/XMLHttpRequest/require) + dynamic DOM creation + input events + window.open.
    let html = r#"
        <input id="name" type="text" value="" oninput="document.getElementById('greet').textContent = 'Hi ' + this.value" />
        <div id="greet">Hi </div>
        <button id="add">Add Item</button>
        <button id="ext">Open External</button>
        <ul id="list"></ul>
        <div id="sandbox"></div>
        <script>
          document.getElementById('sandbox').textContent =
            [typeof fetch, typeof XMLHttpRequest, typeof require].join(',');
          document.getElementById('add').addEventListener('click', function() {
            var li = document.createElement('li');
            li.innerHTML = '<strong>Dynamic</strong>';
            document.getElementById('list').appendChild(li);
          });
          document.getElementById('ext').addEventListener('click', function() {
            window.open('https://example.org/docs');
          });
        </script>
    "#;
    let mut session = HtmlViewSession::new(None, html.into());
    assert!(session.js_active);
    let theme = HtmlTheme::dark();
    let r0 = session.render(80, &theme, false);
    let t0 = rendered_text(&r0.lines);
    assert!(t0.contains("undefined,undefined,undefined"));

    // Type into text input and verify oninput fires live
    let inp = r0.controls[0].clone();
    session.click_at(inp.line, inp.col_start);
    session.input_char('A');
    session.input_char('I');
    session.finish_editing();
    let r1 = session.render(80, &theme, false);
    assert!(rendered_text(&r1.lines).contains("Hi AI"));

    // Click "Add Item" -> createElement + innerHTML + appendChild
    let add_btn = r1.controls[1].clone();
    session.click_at(add_btn.line, add_btn.col_start);
    let r2 = session.render(80, &theme, false);
    assert!(rendered_text(&r2.lines).contains("Dynamic"));

    // Click "Open External" -> HtmlEffect::OpenUrl
    let ext_btn = r2.controls[2].clone();
    let effects = session.click_at(ext_btn.line, ext_btn.col_start);
    assert!(effects.contains(&HtmlEffect::OpenUrl("https://example.org/docs".into())));

    // Verify 8MB memory cap catches OOM allocations cleanly without panicking
    let oom_html = r#"<script>let a = []; while(true) { a.push(new Array(50000).fill('x')); }</script>"#;
    let oom_session = HtmlViewSession::new(None, oom_html.into());
    assert!(oom_session.last_js_error().is_some());
}

#[cfg(feature = "serve")]
#[tokio::test]
async fn integration_serve_lru_eviction_and_clean_shutdown() {
    use bitefoundry_html::NativeHtmlServer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn status_line(url: &str) -> String {
        let rest = url.strip_prefix("http://").unwrap();
        let (host_port, path) = rest.split_once('/').unwrap();
        let mut s = TcpStream::connect(host_port).await.unwrap();
        let req = format!("GET /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).await.unwrap();
        buf.lines().next().unwrap_or("").to_string()
    }

    let server = NativeHtmlServer::new();
    let first_url = server.serve_html("Doc 0", "<p>doc 0</p>").await.unwrap();
    assert!(status_line(&first_url).await.contains("200 OK"));

    // Insert 16 more docs -> evicts Doc 0 (LRU cap = 16)
    let mut last_url = String::new();
    for i in 1..=16 {
        last_url = server
            .serve_html(&format!("Doc {i}"), &format!("<p>doc {i}</p>"))
            .await
            .unwrap();
    }

    assert!(status_line(&first_url).await.contains("404"));
    assert!(status_line(&last_url).await.contains("200 OK"));

    server.shutdown();
}

#[cfg(feature = "graphics")]
#[test]
fn integration_inline_svg_rasterizes_red_pixels_without_xmlns() {
    use bitefoundry_html::{render_dom, DomDocument, GraphicKind};

    let html = r##"<p>Chart:</p><svg width="100" height="50"><rect width="100" height="50" fill="#e11d48"/><text x="10" y="30" fill="#ffffff">차트</text></svg><button>After</button>"##;
    let doc = DomDocument::parse(html);
    let r = render_dom(&doc, 80, &HtmlTheme::dark(), None, None, false);

    assert_eq!(r.graphics.len(), 1);
    let g = &r.graphics[0];
    assert_eq!(g.kind, GraphicKind::InlineSvg);
    assert!(g.image_bytes.starts_with(b"\x89PNG"));
    assert!(g.line_count >= 3 && g.line_count <= 24);
    assert!(r.lines.len() >= g.line_start + g.line_count);

    // Button after SVG sits strictly below the reserved graphic slot.
    let btn = &r.controls[0];
    assert!(btn.line >= g.line_start + g.line_count);

    // Verify decoded PNG actually contains red pixels (`r > 200`).
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(&g.image_bytes).expect("decode SVG PNG");
    let has_red = pixmap.pixels().iter().any(|p| p.red() > 200 && p.alpha() > 200);
    assert!(has_red, "expected red pixels in rasterized SVG");
}

#[cfg(all(feature = "js", feature = "graphics"))]
#[test]
fn integration_dynamic_js_svg_and_canvas_2d_and_timers() {
    use bitefoundry_html::GraphicKind;

    // Dynamic JS SVG via document.createElementNS + requestAnimationFrame
    let svg_html = r#"
        <svg id="host" width="80" height="80"></svg>
        <script>
          requestAnimationFrame(function() {
            var svg = document.getElementById('host');
            var c = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
            c.setAttribute('cx', '40');
            c.setAttribute('cy', '40');
            c.setAttribute('r', '30');
            c.setAttribute('fill', '#22c55e');
            svg.appendChild(c);
          });
        </script>
    "#;
    let mut svg_session = HtmlViewSession::new(None, svg_html.into());
    let r_svg = svg_session.render(80, &HtmlTheme::dark(), false);
    assert_eq!(r_svg.graphics.len(), 1);
    assert_eq!(r_svg.graphics[0].kind, GraphicKind::InlineSvg);
    let pm_svg = resvg::tiny_skia::Pixmap::decode_png(&r_svg.graphics[0].image_bytes)
        .expect("decode dynamic SVG PNG");
    assert!(
        pm_svg.pixels().iter().any(|p| p.green() > 150 && p.alpha() > 200),
        "dynamic JS SVG circle should render green pixels"
    );

    // Canvas 2D via QuickJS getContext('2d')
    let canvas_html = r#"
        <canvas id="c" width="120" height="60"></canvas>
        <script>
          var canvas = document.getElementById('c');
          var rect = canvas.getBoundingClientRect();
          var ctx = canvas.getContext('2d');
          ctx.fillStyle = '#e11d48';
          ctx.fillRect(0, 0, rect.width / 2, rect.height);
          ctx.strokeStyle = 'rgb(30, 144, 255)';
          ctx.lineWidth = 4;
          ctx.beginPath();
          ctx.arc(90, 30, 18, 0, Math.PI * 2);
          ctx.stroke();
          ctx.fillStyle = '#ffffff';
          ctx.font = 'bold 12px sans-serif';
          ctx.fillText('HI', 10, 30);
        </script>
    "#;
    let mut canvas_session = HtmlViewSession::new(None, canvas_html.into());
    assert!(canvas_session.last_js_error().is_none(), "JS error: {:?}", canvas_session.last_js_error());
    let r_canvas = canvas_session.render(80, &HtmlTheme::dark(), false);
    assert_eq!(r_canvas.graphics.len(), 1);
    assert_eq!(r_canvas.graphics[0].kind, GraphicKind::Canvas2d);
    assert!(r_canvas.graphics[0].image_bytes.starts_with(b"\x89PNG"));
    let pm_canvas = resvg::tiny_skia::Pixmap::decode_png(&r_canvas.graphics[0].image_bytes)
        .expect("decode canvas PNG");
    assert!(
        pm_canvas.pixels().iter().any(|p| p.red() > 200 && p.alpha() > 200),
        "canvas fillRect should render red pixels"
    );
    assert!(
        pm_canvas.pixels().iter().any(|p| p.blue() > 200 && p.alpha() > 200),
        "canvas arc stroke should render blue pixels"
    );
}

#[test]
fn integration_external_script_banner_and_undrawn_canvas() {
    let html = r#"
        <script src="https://cdn.example.com/chart.min.js"></script>
        <canvas id="chart" width="400" height="200"></canvas>
        <p>Summary</p>
    "#;
    let mut session = HtmlViewSession::new(None, html.into());
    assert!(!session.js_active, "external script must not spawn QuickJS");
    let r = session.render(80, &HtmlTheme::dark(), false);
    assert_eq!(r.external_scripts, vec!["https://cdn.example.com/chart.min.js"]);
    assert!(r.graphics.is_empty());
    let text = rendered_text(&r.lines);
    assert!(text.contains("chart.min.js"));
    assert!(text.contains("Press 'o'"));
    assert!(text.contains("Summary"));
}

#[cfg(feature = "graphics")]
#[test]
fn integration_data_image_uris_and_spoof_rejection() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use bitefoundry_html::{render_dom, DomDocument, GraphicKind};

    let mut pm = resvg::tiny_skia::Pixmap::new(4, 4).unwrap();
    pm.fill(resvg::tiny_skia::Color::from_rgba8(10, 120, 240, 255));
    let png_b64 = STANDARD.encode(pm.encode_png().unwrap());

    let html = format!(
        r#"
        <img alt="ValidPng" src="data:image/png;base64,{png_b64}" />
        <img alt="ValidSvg" src="data:image/svg+xml,%3Csvg%20width%3D%2220%22%20height%3D%2220%22%3E%3Crect%20width%3D%2220%22%20height%3D%2220%22%20fill%3D%22green%22/%3E%3C/svg%3E" />
        <img alt="Spoofed" src="data:image/png;base64,PGh0bWw+PGJvZHk+bm9wZTwvYm9keT48L2h0bWw+" />
        "#
    );
    let doc = DomDocument::parse(&html);
    let r = render_dom(&doc, 80, &HtmlTheme::dark(), None, None, false);
    assert_eq!(r.graphics.len(), 2, "valid PNG and SVG data URIs must emit 2 graphics, spoofed rejected");
    assert_eq!(r.graphics[0].kind, GraphicKind::DataImage);
    assert_eq!(r.graphics[1].kind, GraphicKind::DataImage);
    assert!(r.graphics[0].image_bytes.starts_with(b"\x89PNG"));
    assert!(r.graphics[1].image_bytes.starts_with(b"\x89PNG"));
    let text = rendered_text(&r.lines);
    assert!(text.contains("[🖼 Spoofed]"));
}

#[test]
fn markdown_html_session_raw_toggle_and_serve_html() {
    use bitefoundry_html::{HtmlEffect, SourceKind};

    let md = "---\ntitle: Guide\n---\n# Guide\n\nRead [Docs](https://bitefoundry.dev) now.";
    let mut session = HtmlViewSession::from_content(None, ContentKind::Markdown, md.into());
    assert_eq!(session.source_kind(), SourceKind::Markdown);
    assert!(!session.js_active, "Markdown session must never allocate QuickJS");

    let theme = HtmlTheme::dark();
    let r0 = session.render(80, &theme, false);
    let t0 = rendered_text(&r0.lines);
    assert!(t0.contains("title: Guide"));
    assert!(t0.contains("Docs"));
    assert!(!t0.contains("[Docs]("));
    assert_eq!(r0.controls.len(), 1);

    // Click Markdown link control -> emits HtmlEffect::OpenUrl
    let link = r0.controls[0].clone();
    let effs = session.click_at(link.line, link.col_start);
    assert!(effs.contains(&HtmlEffect::OpenUrl("https://bitefoundry.dev".into())));

    // Raw mode toggle shows raw Markdown source
    assert!(session.toggle_raw_mode());
    let r_raw = session.render(80, &theme, false);
    let t_raw = rendered_text(&r_raw.lines);
    assert!(t_raw.contains("# Guide"));
    assert!(t_raw.contains("[Docs](https://bitefoundry.dev)"));
    assert!(r_raw.controls.is_empty());

    // serve_html() returns valid standalone HTML5 document
    let served = session.serve_html();
    assert!(served.starts_with("<!DOCTYPE html>"));
    assert!(served.contains("<title>Guide</title>"));
    assert!(served.contains("https://bitefoundry.dev"));
}
