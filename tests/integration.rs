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
