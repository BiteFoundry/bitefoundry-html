//! On-demand sandboxed QuickJS-NG bridge with strict memory and execution-time limits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::{CatchResultExt, Context, Function, Runtime, Value};

use crate::dom::DomNodeId;
pub use crate::session::DomSharedState;

/// Hard heap memory limit for each QuickJS runtime (8 MiB).
pub const JS_MEMORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
/// Hard call-stack limit for each QuickJS runtime (512 KiB).
pub const JS_MAX_STACK_BYTES: usize = 512 * 1024;
/// Per-script / per-event execution deadline (200 ms).
pub const JS_EXECUTION_TIMEOUT: Duration = Duration::from_millis(200);

const BF_DOM_SHIM: &str = r#"
(function(g) {
  const _cache = new Map();
  const _listeners = new Map();
  const _docListeners = new Map();
  const _ctxCache = new Map();
  let _timerId = 0;
  const _timers = [];
  const _cancelledTimers = new Set();

  function _toKebab(prop) {
    return String(prop).replace(/[A-Z]/g, function(m) { return '-' + m.toLowerCase(); });
  }

  function _parsePx(val, fallback) {
    if (val === null || val === undefined || val === "") return fallback;
    const n = parseFloat(String(val));
    return (isFinite(n) && n > 0) ? n : fallback;
  }

  function _makeCanvas2d(id, canvasEl) {
    let ctx2d = _ctxCache.get(id);
    if (ctx2d) return ctx2d;
    if (!__bf_canvas_ensure(id)) return null;
    let _dash = [];
    ctx2d = {
      canvas: canvasEl,
      get fillStyle() { return __bf_canvas_get_str(id, "fillStyle"); },
      set fillStyle(v) { __bf_canvas_set_str(id, "fillStyle", String(v)); },
      get strokeStyle() { return __bf_canvas_get_str(id, "strokeStyle"); },
      set strokeStyle(v) { __bf_canvas_set_str(id, "strokeStyle", String(v)); },
      get lineWidth() { return __bf_canvas_get_num(id, "lineWidth"); },
      set lineWidth(v) { __bf_canvas_set_num(id, "lineWidth", Number(v)); },
      get lineCap() { return __bf_canvas_get_str(id, "lineCap"); },
      set lineCap(v) { __bf_canvas_set_str(id, "lineCap", String(v)); },
      get lineJoin() { return __bf_canvas_get_str(id, "lineJoin"); },
      set lineJoin(v) { __bf_canvas_set_str(id, "lineJoin", String(v)); },
      get globalAlpha() { return __bf_canvas_get_num(id, "globalAlpha"); },
      set globalAlpha(v) { __bf_canvas_set_num(id, "globalAlpha", Number(v)); },
      get font() { return __bf_canvas_get_str(id, "font"); },
      set font(v) { __bf_canvas_set_str(id, "font", String(v)); },
      get textAlign() { return __bf_canvas_get_str(id, "textAlign"); },
      set textAlign(v) { __bf_canvas_set_str(id, "textAlign", String(v)); },
      get textBaseline() { return __bf_canvas_get_str(id, "textBaseline"); },
      set textBaseline(v) { __bf_canvas_set_str(id, "textBaseline", String(v)); },
      setLineDash: function(segs) {
        if (!Array.isArray(segs)) return;
        _dash = segs.map(Number).filter(function(n) { return isFinite(n) && n >= 0; });
        __bf_canvas_set_dash(id, _dash);
      },
      getLineDash: function() { return _dash.slice(); },
      save: function() { __bf_canvas_op0(id, "save"); },
      restore: function() { __bf_canvas_op0(id, "restore"); },
      translate: function(x, y) { __bf_canvas_op2(id, "translate", Number(x), Number(y)); },
      scale: function(x, y) { __bf_canvas_op2(id, "scale", Number(x), Number(y)); },
      rotate: function(a) { __bf_canvas_op1(id, "rotate", Number(a)); },
      setTransform: function(a, b, c, d, e, f) {
        __bf_canvas_op6(id, "setTransform", [Number(a), Number(b), Number(c), Number(d), Number(e), Number(f)]);
      },
      resetTransform: function() { __bf_canvas_op0(id, "resetTransform"); },
      fillRect: function(x, y, w, h) { __bf_canvas_op4(id, "fillRect", Number(x), Number(y), Number(w), Number(h)); },
      strokeRect: function(x, y, w, h) { __bf_canvas_op4(id, "strokeRect", Number(x), Number(y), Number(w), Number(h)); },
      clearRect: function(x, y, w, h) { __bf_canvas_op4(id, "clearRect", Number(x), Number(y), Number(w), Number(h)); },
      beginPath: function() { __bf_canvas_op0(id, "beginPath"); },
      closePath: function() { __bf_canvas_op0(id, "closePath"); },
      moveTo: function(x, y) { __bf_canvas_op2(id, "moveTo", Number(x), Number(y)); },
      lineTo: function(x, y) { __bf_canvas_op2(id, "lineTo", Number(x), Number(y)); },
      rect: function(x, y, w, h) { __bf_canvas_op4(id, "rect", Number(x), Number(y), Number(w), Number(h)); },
      arc: function(x, y, r, s, e, ccw) {
        __bf_canvas_op6(id, "arc", [Number(x), Number(y), Number(r), Number(s), Number(e), ccw ? 1 : 0]);
      },
      ellipse: function(x, y, rx, ry, rot, s, e, ccw) {
        __bf_canvas_ellipse(id, [Number(x), Number(y), Number(rx), Number(ry), Number(rot), Number(s), Number(e), ccw ? 1 : 0]);
      },
      quadraticCurveTo: function(cpx, cpy, x, y) {
        __bf_canvas_op4(id, "quadraticCurveTo", Number(cpx), Number(cpy), Number(x), Number(y));
      },
      bezierCurveTo: function(cp1x, cp1y, cp2x, cp2y, x, y) {
        __bf_canvas_op6(id, "bezierCurveTo", [Number(cp1x), Number(cp1y), Number(cp2x), Number(cp2y), Number(x), Number(y)]);
      },
      fill: function(rule) {
        __bf_canvas_op0(id, rule === "evenodd" ? "fillEvenOdd" : "fill");
      },
      stroke: function() { __bf_canvas_op0(id, "stroke"); },
      fillText: function(text, x, y) { __bf_canvas_text(id, "fill", String(text), Number(x), Number(y)); },
      strokeText: function(text, x, y) { __bf_canvas_text(id, "stroke", String(text), Number(x), Number(y)); },
      measureText: function(text) {
        return { width: __bf_canvas_measure_text(id, String(text)) };
      }
    };
    _ctxCache.set(id, ctx2d);
    return ctx2d;
  }

  function _wrap(id) {
    if (id === null || id === undefined || id < 0) return null;
    let el = _cache.get(id);
    if (el) return el;

    const classList = {
      add: function() {
        for (let i = 0; i < arguments.length; i++) {
          __bf_class_op(id, "add", String(arguments[i]));
        }
      },
      remove: function() {
        for (let i = 0; i < arguments.length; i++) {
          __bf_class_op(id, "remove", String(arguments[i]));
        }
      },
      toggle: function(cls, force) {
        const name = String(cls);
        if (force === true) {
          __bf_class_op(id, "add", name);
          return true;
        }
        if (force === false) {
          __bf_class_op(id, "remove", name);
          return false;
        }
        return __bf_class_op(id, "toggle", name);
      },
      contains: function(cls) {
        return __bf_class_op(id, "contains", String(cls));
      },
      replace: function(oldCls, newCls) {
        if (__bf_class_op(id, "contains", String(oldCls))) {
          __bf_class_op(id, "remove", String(oldCls));
          __bf_class_op(id, "add", String(newCls));
          return true;
        }
        return false;
      }
    };

    const style = new Proxy({
      setProperty: function(prop, val) {
        __bf_style_set(id, String(prop), val === null || val === undefined ? "" : String(val));
      },
      getPropertyValue: function(prop) {
        return __bf_style_get(id, String(prop));
      },
      removeProperty: function(prop) {
        const old = __bf_style_get(id, String(prop));
        __bf_style_set(id, String(prop), "");
        return old;
      }
    }, {
      get: function(target, prop) {
        if (prop in target) return target[prop];
        if (typeof prop !== 'string') return undefined;
        return __bf_style_get(id, _toKebab(prop));
      },
      set: function(target, prop, value) {
        if (typeof prop !== 'string') return true;
        __bf_style_set(id, _toKebab(prop), value === null || value === undefined ? "" : String(value));
        return true;
      }
    });

    const dataset = new Proxy({}, {
      get: function(_t, prop) {
        if (typeof prop !== 'string') return undefined;
        const v = __bf_get_attr(id, "data-" + _toKebab(prop));
        return v === null ? undefined : v;
      },
      set: function(_t, prop, value) {
        if (typeof prop !== 'string') return true;
        __bf_set_attr(id, "data-" + _toKebab(prop), String(value));
        return true;
      },
      deleteProperty: function(_t, prop) {
        if (typeof prop !== 'string') return true;
        __bf_remove_attr(id, "data-" + _toKebab(prop));
        return true;
      }
    });

    el = {
      __bf_id: id,
      classList: classList,
      style: style,
      dataset: dataset,
      get nodeType() { return 1; },
      get tagName() { return __bf_get_prop(id, "tagName"); },
      get nodeName() { return __bf_get_prop(id, "tagName"); },
      get id() { return __bf_get_prop(id, "id"); },
      set id(v) { __bf_set_prop(id, "id", String(v)); },
      get className() { return __bf_get_prop(id, "className"); },
      set className(v) { __bf_set_prop(id, "className", String(v)); },
      get textContent() { return __bf_get_text(id); },
      set textContent(v) { __bf_set_text(id, v === null || v === undefined ? "" : String(v)); },
      get innerText() { return __bf_get_text(id); },
      set innerText(v) { __bf_set_text(id, v === null || v === undefined ? "" : String(v)); },
      get innerHTML() { return __bf_get_inner_html(id); },
      set innerHTML(v) { __bf_set_inner_html(id, v === null || v === undefined ? "" : String(v)); },
      get value() { return __bf_get_prop(id, "value"); },
      set value(v) { __bf_set_prop(id, "value", v === null || v === undefined ? "" : String(v)); },
      get checked() { return __bf_get_prop(id, "checked") === "true"; },
      set checked(v) { __bf_set_prop(id, "checked", v ? "true" : "false"); },
      get open() { return __bf_get_prop(id, "open") === "true"; },
      set open(v) { __bf_set_prop(id, "open", v ? "true" : "false"); },
      get hidden() { return __bf_get_prop(id, "hidden") === "true"; },
      set hidden(v) { __bf_set_prop(id, "hidden", v ? "true" : "false"); },
      get selectedIndex() { return parseInt(__bf_get_prop(id, "selectedIndex"), 10) || 0; },
      set selectedIndex(v) { __bf_set_prop(id, "selectedIndex", String(v)); },
      get href() { return __bf_get_attr(id, "href") || ""; },
      set href(v) { __bf_set_attr(id, "href", String(v)); },
      get type() { return __bf_get_attr(id, "type") || ""; },
      set type(v) { __bf_set_attr(id, "type", String(v)); },
      get width() {
        const attr = __bf_get_attr(id, "width");
        const isCanvas = __bf_get_prop(id, "tagName") === "CANVAS";
        return Math.round(_parsePx(attr, isCanvas ? 300 : 0));
      },
      set width(v) {
        const w = Math.max(1, Math.round(Number(v) || 300));
        __bf_set_attr(id, "width", String(w));
        if (__bf_get_prop(id, "tagName") === "CANVAS") {
          const h = Math.round(_parsePx(__bf_get_attr(id, "height"), 150));
          __bf_canvas_resize(id, w, h);
        }
      },
      get height() {
        const attr = __bf_get_attr(id, "height");
        const isCanvas = __bf_get_prop(id, "tagName") === "CANVAS";
        return Math.round(_parsePx(attr, isCanvas ? 150 : 0));
      },
      set height(v) {
        const h = Math.max(1, Math.round(Number(v) || 150));
        __bf_set_attr(id, "height", String(h));
        if (__bf_get_prop(id, "tagName") === "CANVAS") {
          const w = Math.round(_parsePx(__bf_get_attr(id, "width"), 300));
          __bf_canvas_resize(id, w, h);
        }
      },
      get parentNode() { return _wrap(__bf_get_parent(id)); },
      get parentElement() { return _wrap(__bf_get_parent(id)); },
      get children() { return __bf_get_children(id).map(_wrap).filter(Boolean); },
      getContext: function(kind) {
        if (String(kind).toLowerCase() === "2d" && __bf_get_prop(id, "tagName") === "CANVAS") {
          return _makeCanvas2d(id, el);
        }
        return null;
      },
      toDataURL: function() {
        if (__bf_get_prop(id, "tagName") === "CANVAS") {
          return __bf_canvas_to_data_url(id);
        }
        return "";
      },
      getBoundingClientRect: function() {
        const isCanvas = __bf_get_prop(id, "tagName") === "CANVAS";
        const defW = isCanvas ? 300 : 600;
        const defH = isCanvas ? 150 : 300;
        const w = _parsePx(__bf_get_attr(id, "width") || __bf_style_get(id, "width"), defW);
        const h = _parsePx(__bf_get_attr(id, "height") || __bf_style_get(id, "height"), defH);
        return { x: 0, y: 0, width: w, height: h, top: 0, left: 0, right: w, bottom: h };
      },
      getAttribute: function(name) {
        return __bf_get_attr(id, String(name));
      },
      setAttribute: function(name, val) {
        const k = String(name);
        const v = String(val);
        __bf_set_attr(id, k, v);
        if (__bf_get_prop(id, "tagName") === "CANVAS" && (k.toLowerCase() === "width" || k.toLowerCase() === "height")) {
          const w = Math.round(_parsePx(__bf_get_attr(id, "width"), 300));
          const h = Math.round(_parsePx(__bf_get_attr(id, "height"), 150));
          __bf_canvas_resize(id, w, h);
        }
      },
      removeAttribute: function(name) {
        __bf_remove_attr(id, String(name));
      },
      hasAttribute: function(name) {
        return __bf_get_attr(id, String(name)) !== null;
      },
      querySelector: function(sel) {
        const arr = __bf_qsa(id, String(sel));
        return arr.length > 0 ? _wrap(arr[0]) : null;
      },
      querySelectorAll: function(sel) {
        return __bf_qsa(id, String(sel)).map(_wrap).filter(Boolean);
      },
      appendChild: function(child) {
        if (child && typeof child.__bf_id === 'number') {
          __bf_append_child(id, child.__bf_id);
        }
        return child;
      },
      append: function() {
        for (let i = 0; i < arguments.length; i++) {
          const item = arguments[i];
          if (typeof item === 'string') {
            const tid = __bf_create_text(item);
            __bf_append_child(id, tid);
          } else if (item && typeof item.__bf_id === 'number') {
            __bf_append_child(id, item.__bf_id);
          }
        }
      },
      remove: function() {
        __bf_remove_node(id);
      },
      removeChild: function(child) {
        if (child && typeof child.__bf_id === 'number') {
          __bf_remove_node(child.__bf_id);
        }
        return child;
      },
      addEventListener: function(type, fn) {
        if (typeof fn !== 'function') return;
        const key = id + ':' + String(type).toLowerCase();
        let list = _listeners.get(key);
        if (!list) { list = []; _listeners.set(key, list); }
        if (list.indexOf(fn) === -1) list.push(fn);
      },
      removeEventListener: function(type, fn) {
        const key = id + ':' + String(type).toLowerCase();
        const list = _listeners.get(key);
        if (!list) return;
        const idx = list.indexOf(fn);
        if (idx !== -1) list.splice(idx, 1);
      },
      dispatchEvent: function(ev) {
        const t = ev && ev.type ? String(ev.type).toLowerCase() : 'event';
        return !__bf_dispatch_event(id, t, "");
      },
      click: function() {
        const inline = __bf_get_attr(id, "onclick") || "";
        __bf_dispatch_event(id, "click", inline);
      },
      focus: function() {},
      blur: function() {}
    };

    _cache.set(id, el);
    return el;
  }

  function _createEvent(type, target) {
    return {
      type: type,
      target: target,
      currentTarget: target,
      defaultPrevented: false,
      _stopped: false,
      preventDefault: function() { this.defaultPrevented = true; },
      stopPropagation: function() { this._stopped = true; },
      stopImmediatePropagation: function() { this._stopped = true; }
    };
  }

  g.__bf_flush_timers = function() {
    let executed = 0;
    while (_timers.length > 0 && executed < 16) {
      const item = _timers.shift();
      if (_cancelledTimers.has(item.id)) {
        _cancelledTimers.delete(item.id);
        continue;
      }
      executed++;
      item.fn();
    }
  };

  g.__bf_dispatch_event = function(nodeId, eventType, inlineCode) {
    const type = String(eventType).toLowerCase();
    const target = _wrap(nodeId);
    const ev = _createEvent(type, target);

    if (inlineCode && inlineCode.length > 0 && target) {
      const fn = new Function("event", inlineCode);
      const res = fn.call(target, ev);
      if (res === false) ev.preventDefault();
    }

    let curId = nodeId;
    while (curId >= 0) {
      const curEl = _wrap(curId);
      ev.currentTarget = curEl;

      const propHandler = curEl && curEl["on" + type];
      if (typeof propHandler === 'function') {
        const res = propHandler.call(curEl, ev);
        if (res === false) ev.preventDefault();
      }

      const key = curId + ':' + type;
      const list = _listeners.get(key);
      if (list) {
        const copy = list.slice();
        for (let i = 0; i < copy.length; i++) {
          copy[i].call(curEl, ev);
        }
      }

      if (ev._stopped) break;
      const parentId = __bf_get_parent(curId);
      if (parentId < 0 || parentId === curId) break;
      curId = parentId;
    }

    if (!ev._stopped) {
      const dlist = _docListeners.get(type);
      if (dlist) {
        ev.currentTarget = g.document;
        const copy = dlist.slice();
        for (let i = 0; i < copy.length; i++) {
          copy[i].call(g.document, ev);
        }
      }
    }

    g.__bf_flush_timers();
    return !!ev.defaultPrevented;
  };

  g.__bf_fire_dom_content_loaded = function() {
    const ev = _createEvent("domcontentloaded", g.document);
    const dlist = _docListeners.get("domcontentloaded");
    if (dlist) {
      const copy = dlist.slice();
      for (let i = 0; i < copy.length; i++) {
        copy[i].call(g.document, ev);
      }
    }
    g.__bf_flush_timers();
  };

  g.document = {
    get title() { return __bf_get_title(); },
    set title(v) { __bf_set_title(String(v)); },
    get body() {
      const arr = __bf_qsa(-1, "body");
      return arr.length > 0 ? _wrap(arr[0]) : null;
    },
    get documentElement() {
      const arr = __bf_qsa(-1, "html");
      return arr.length > 0 ? _wrap(arr[0]) : null;
    },
    getElementById: function(id) {
      return _wrap(__bf_get_by_id(String(id)));
    },
    querySelector: function(sel) {
      const arr = __bf_qsa(-1, String(sel));
      return arr.length > 0 ? _wrap(arr[0]) : null;
    },
    querySelectorAll: function(sel) {
      return __bf_qsa(-1, String(sel)).map(_wrap).filter(Boolean);
    },
    getElementsByTagName: function(tag) {
      return __bf_qsa(-1, String(tag)).map(_wrap).filter(Boolean);
    },
    getElementsByClassName: function(cls) {
      const sel = String(cls).trim().split(/\s+/).filter(Boolean).map(function(c) { return '.' + c; }).join('');
      if (!sel) return [];
      return __bf_qsa(-1, sel).map(_wrap).filter(Boolean);
    },
    createElement: function(tag) {
      return _wrap(__bf_create_el(String(tag)));
    },
    createElementNS: function(_ns, tag) {
      return _wrap(__bf_create_el(String(tag)));
    },
    createTextNode: function(text) {
      return { __bf_id: __bf_create_text(String(text)), nodeType: 3 };
    },
    addEventListener: function(type, fn) {
      if (typeof fn !== 'function') return;
      const key = String(type).toLowerCase();
      let list = _docListeners.get(key);
      if (!list) { list = []; _docListeners.set(key, list); }
      if (list.indexOf(fn) === -1) list.push(fn);
    },
    removeEventListener: function(type, fn) {
      const key = String(type).toLowerCase();
      const list = _docListeners.get(key);
      if (!list) return;
      const idx = list.indexOf(fn);
      if (idx !== -1) list.splice(idx, 1);
    }
  };

  g.setTimeout = function(fn, _delay) {
    const tid = ++_timerId;
    if (typeof fn === 'function') {
      _timers.push({ id: tid, fn: fn });
    }
    return tid;
  };
  g.clearTimeout = function(tid) {
    _cancelledTimers.add(tid);
  };
  g.requestAnimationFrame = function(fn) {
    const tid = ++_timerId;
    if (typeof fn === 'function') {
      _timers.push({ id: tid, fn: function() { fn(16); } });
    }
    return tid;
  };
  g.cancelAnimationFrame = function(tid) {
    _cancelledTimers.add(tid);
  };
  g.devicePixelRatio = 1;
  g.innerWidth = 1024;
  g.innerHeight = 768;

  g.alert = function(msg) {
    __bf_alert(msg === undefined ? "" : String(msg));
  };
  g.confirm = function(msg) {
    __bf_alert(msg === undefined ? "" : String(msg));
    return true;
  };
  g.open = function(url) {
    if (url !== undefined && url !== null) {
      __bf_open_url(String(url));
    }
    return null;
  };
  g.console = {
    log: function() { __bf_log("log", Array.prototype.slice.call(arguments).map(String).join(" ")); },
    info: function() { __bf_log("info", Array.prototype.slice.call(arguments).map(String).join(" ")); },
    warn: function() { __bf_log("warn", Array.prototype.slice.call(arguments).map(String).join(" ")); },
    error: function() { __bf_log("error", Array.prototype.slice.call(arguments).map(String).join(" ")); }
  };
  g.Event = function(type) { return _createEvent(String(type).toLowerCase(), null); };
  g.window = g;
  g.self = g;
})(globalThis);
"#;

/// Sandboxed QuickJS runtime bound to a shared mutable [`DomDocument`].
pub struct JsSession {
    _rt: Runtime,
    ctx: Context,
    state: Arc<Mutex<DomSharedState>>,
    epoch: Instant,
    deadline_ms: Arc<AtomicU64>,
    last_js_error: Option<String>,
}

impl JsSession {
    /// Creates an on-demand QuickJS session, configures strict sandbox limits, and runs inline `<script>` blocks.
    pub fn new(state: Arc<Mutex<DomSharedState>>) -> Self {
        let rt = Runtime::new().expect("QuickJS runtime initialization failed");
        rt.set_memory_limit(JS_MEMORY_LIMIT_BYTES);
        rt.set_max_stack_size(JS_MAX_STACK_BYTES);

        let epoch = Instant::now();
        let deadline_ms = Arc::new(AtomicU64::new(0));
        {
            let deadline_ref = Arc::clone(&deadline_ms);
            rt.set_interrupt_handler(Some(Box::new(move || {
                let limit = deadline_ref.load(Ordering::Relaxed);
                if limit == 0 {
                    return false;
                }
                (epoch.elapsed().as_millis() as u64) >= limit
            })));
        }

        let ctx = Context::full(&rt).expect("QuickJS context initialization failed");

        let mut session = Self {
            _rt: rt,
            ctx,
            state,
            epoch,
            deadline_ms,
            last_js_error: None,
        };

        if let Err(err) = session.install_bridge() {
            session.last_js_error = Some(err);
            return session;
        }

        let scripts: Vec<String> = session
            .state
            .lock()
            .map(|s| s.doc.inline_scripts().to_vec())
            .unwrap_or_default();

        for script in scripts {
            if let Err(err) = session.eval_script(&script) {
                session.last_js_error = Some(err);
            }
        }

        let _ = session.eval_script("__bf_fire_dom_content_loaded();");
        session
    }

    pub fn last_js_error(&self) -> Option<&str> {
        self.last_js_error.as_deref()
    }

    /// Evaluates a JavaScript snippet with a 200ms execution deadline.
    pub fn eval_script(&mut self, code: &str) -> Result<(), String> {
        let res: Result<(), String> = {
            let _guard = arm_deadline(self.epoch, &self.deadline_ms);
            self.ctx.with(|ctx| {
                ctx.eval::<Value, _>(code)
                    .catch(&ctx)
                    .map(|_| ())
                    .map_err(|e| format!("{e}"))
            })
        };
        if let Err(ref err) = res {
            self.last_js_error = Some(err.clone());
        }
        res
    }

    /// Dispatches a `click` event (both inline `onclick` and `addEventListener('click', ...)`).
    /// Returns `true` if `event.preventDefault()` was called.
    pub fn dispatch_click(&mut self, node_id: DomNodeId) -> bool {
        let inline = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.doc.get_attr(node_id, "onclick").map(ToString::to_string))
            .unwrap_or_default();
        self.dispatch_event(node_id, "click", &inline)
    }

    /// Updates the control's DOM value/checked state (if `new_value_or_checked` is `Some`)
    /// and dispatches `input` (`oninput`) and `change` (`onchange`) events.
    pub fn dispatch_input_change(
        &mut self,
        node_id: DomNodeId,
        new_value_or_checked: Option<&str>,
    ) -> bool {
        if let Some(new_val) = new_value_or_checked {
            if let Ok(mut st) = self.state.lock() {
                let is_checkable = st.doc.element(node_id).is_some_and(|el| {
                    el.tag == "input"
                        && matches!(
                            el.input_type().to_ascii_lowercase().as_str(),
                            "checkbox" | "radio"
                        )
                });
                if is_checkable {
                    if new_val.eq_ignore_ascii_case("true") || new_val == "1" {
                        st.doc.set_attr(node_id, "checked", "");
                    } else {
                        st.doc.remove_attr(node_id, "checked");
                    }
                } else {
                    st.doc.set_input_value(node_id, new_val);
                }
                st.dirty = true;
            }
        }
        let (oninput, onchange) = self
            .state
            .lock()
            .ok()
            .map(|s| {
                (
                    s.doc
                        .get_attr(node_id, "oninput")
                        .unwrap_or("")
                        .to_string(),
                    s.doc
                        .get_attr(node_id, "onchange")
                        .unwrap_or("")
                        .to_string(),
                )
            })
            .unwrap_or_default();
        let p1 = self.dispatch_event(node_id, "input", &oninput);
        let p2 = self.dispatch_event(node_id, "change", &onchange);
        p1 || p2
    }

    /// Dispatches `input` (`oninput`) while editing a text field.
    pub fn dispatch_input(&mut self, node_id: DomNodeId) -> bool {
        let oninput = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.doc.get_attr(node_id, "oninput").map(ToString::to_string))
            .unwrap_or_default();
        self.dispatch_event(node_id, "input", &oninput)
    }

    /// Dispatches `change` (`onchange`) when finishing input editing.
    pub fn dispatch_change(&mut self, node_id: DomNodeId) -> bool {
        let onchange = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.doc.get_attr(node_id, "onchange").map(ToString::to_string))
            .unwrap_or_default();
        self.dispatch_event(node_id, "change", &onchange)
    }

    /// Dispatches `submit` (`onsubmit`) on a `<form>`. Returns `true` if `preventDefault()` was called.
    pub fn dispatch_submit(&mut self, form_id: DomNodeId) -> bool {
        let onsubmit = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.doc.get_attr(form_id, "onsubmit").map(ToString::to_string))
            .unwrap_or_default();
        self.dispatch_event(form_id, "submit", &onsubmit)
    }

    fn dispatch_event(&mut self, node_id: DomNodeId, event_type: &str, inline_code: &str) -> bool {
        let res: Result<bool, String> = {
            let _guard = arm_deadline(self.epoch, &self.deadline_ms);
            self.ctx.with(|ctx| {
                let globals = ctx.globals();
                let func: Function = globals
                    .get("__bf_dispatch_event")
                    .catch(&ctx)
                    .map_err(|e| format!("{e}"))?;
                func.call::<_, bool>((
                    node_id.0 as i64,
                    event_type.to_string(),
                    inline_code.to_string(),
                ))
                .catch(&ctx)
                .map_err(|e| format!("{e}"))
            })
        };
        match res {
            Ok(prevented) => prevented,
            Err(err) => {
                self.last_js_error = Some(err);
                false
            }
        }
    }

    fn install_bridge(&mut self) -> Result<(), String> {
        let state = Arc::clone(&self.state);
        self.ctx.with(|ctx| -> Result<(), String> {
            let g = ctx.globals();

            macro_rules! bind_fn {
                ($name:expr, $closure:expr) => {{
                    let f = Function::new(ctx.clone(), $closure)
                        .catch(&ctx)
                        .map_err(|e| format!("{e}"))?;
                    g.set($name, f).catch(&ctx).map_err(|e| format!("{e}"))?;
                }};
            }

            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_by_id", move |id: String| -> i64 {
                    st.lock()
                        .ok()
                        .and_then(|s| s.doc.get_element_by_id(&id))
                        .map(|n| n.0 as i64)
                        .unwrap_or(-1)
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_qsa", move |root_id: i64, sel: String| -> Vec<i64> {
                    st.lock()
                        .ok()
                        .map(|s| {
                            let nodes = if root_id < 0 {
                                s.doc.query_selector_all(&sel)
                            } else {
                                s.doc.query_selector_all_from(DomNodeId(root_id as usize), &sel)
                            };
                            nodes.into_iter().map(|n| n.0 as i64).collect()
                        })
                        .unwrap_or_default()
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_attr", move |id: i64, name: String| -> Option<String> {
                    if id < 0 {
                        return None;
                    }
                    st.lock()
                        .ok()
                        .and_then(|s| s.doc.get_attr(DomNodeId(id as usize), &name).map(ToString::to_string))
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_set_attr", move |id: i64, name: String, val: String| {
                    if id >= 0 {
                        if let Ok(mut s) = st.lock() {
                            s.doc.set_attr(DomNodeId(id as usize), &name, &val);
                            s.dirty = true;
                        }
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_remove_attr", move |id: i64, name: String| {
                    if id >= 0 {
                        if let Ok(mut s) = st.lock() {
                            s.doc.remove_attr(DomNodeId(id as usize), &name);
                            s.dirty = true;
                        }
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_text", move |id: i64| -> String {
                    if id < 0 {
                        return String::new();
                    }
                    st.lock()
                        .ok()
                        .map(|s| s.doc.text_content(DomNodeId(id as usize)))
                        .unwrap_or_default()
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_set_text", move |id: i64, text: String| {
                    if id >= 0 {
                        if let Ok(mut s) = st.lock() {
                            s.doc.set_text_content(DomNodeId(id as usize), &text);
                            s.dirty = true;
                        }
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_inner_html", move |id: i64| -> String {
                    if id < 0 {
                        return String::new();
                    }
                    st.lock()
                        .ok()
                        .map(|s| s.doc.inner_html(DomNodeId(id as usize)))
                        .unwrap_or_default()
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_set_inner_html", move |id: i64, html: String| {
                    if id >= 0 {
                        if let Ok(mut s) = st.lock() {
                            s.doc.set_inner_html(DomNodeId(id as usize), &html);
                            s.dirty = true;
                        }
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_prop", move |id: i64, prop: String| -> String {
                    if id < 0 {
                        return String::new();
                    }
                    let Ok(s) = st.lock() else {
                        return String::new();
                    };
                    let Some(el) = s.doc.element(DomNodeId(id as usize)) else {
                        return String::new();
                    };
                    match prop.as_str() {
                        "tagName" => el.tag.to_ascii_uppercase(),
                        "id" => el.get_attr("id").unwrap_or("").to_string(),
                        "className" => el.get_attr("class").unwrap_or("").to_string(),
                        "value" => el.value.clone(),
                        "checked" => el.checked.to_string(),
                        "open" => el.open.to_string(),
                        "hidden" => el.hidden.to_string(),
                        "selectedIndex" => el.selected_index.to_string(),
                        _ => el.get_attr(&prop).unwrap_or("").to_string(),
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_set_prop", move |id: i64, prop: String, val: String| {
                    if id < 0 {
                        return;
                    }
                    let nid = DomNodeId(id as usize);
                    let Ok(mut s) = st.lock() else {
                        return;
                    };
                    match prop.as_str() {
                        "id" => s.doc.set_attr(nid, "id", &val),
                        "className" => s.doc.set_attr(nid, "class", &val),
                        "value" => s.doc.set_input_value(nid, &val),
                        "checked" => {
                            if val == "true" {
                                s.doc.set_attr(nid, "checked", "");
                            } else {
                                s.doc.remove_attr(nid, "checked");
                            }
                        }
                        "open" => {
                            if val == "true" {
                                s.doc.set_attr(nid, "open", "");
                            } else {
                                s.doc.remove_attr(nid, "open");
                            }
                        }
                        "hidden" => {
                            if val == "true" {
                                s.doc.set_attr(nid, "hidden", "");
                            } else {
                                s.doc.remove_attr(nid, "hidden");
                            }
                        }
                        "selectedIndex" => {
                            if let Ok(idx) = val.parse::<usize>() {
                                s.doc.set_select_index(nid, idx);
                            }
                        }
                        _ => s.doc.set_attr(nid, &prop, &val),
                    }
                    s.dirty = true;
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_class_op",
                    move |id: i64, op: String, cls: String| -> bool {
                        if id < 0 {
                            return false;
                        }
                        let nid = DomNodeId(id as usize);
                        let Ok(mut s) = st.lock() else {
                            return false;
                        };
                        match op.as_str() {
                            "add" => {
                                s.doc.add_class(nid, &cls);
                                s.dirty = true;
                                true
                            }
                            "remove" => {
                                s.doc.remove_class(nid, &cls);
                                s.dirty = true;
                                false
                            }
                            "toggle" => {
                                let res = s.doc.toggle_class(nid, &cls);
                                s.dirty = true;
                                res
                            }
                            "contains" => s.doc.has_class(nid, &cls),
                            _ => false,
                        }
                    }
                );
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_style_get", move |id: i64, prop: String| -> String {
                    if id < 0 {
                        return String::new();
                    }
                    st.lock()
                        .ok()
                        .map(|s| s.doc.get_style_property(DomNodeId(id as usize), &prop))
                        .unwrap_or_default()
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_style_set",
                    move |id: i64, prop: String, val: String| {
                        if id >= 0 {
                            if let Ok(mut s) = st.lock() {
                                s.doc
                                    .set_style_property(DomNodeId(id as usize), &prop, &val);
                                s.dirty = true;
                            }
                        }
                    }
                );
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_create_el", move |tag: String| -> i64 {
                    st.lock()
                        .ok()
                        .map(|mut s| {
                            s.dirty = true;
                            s.doc.create_element(&tag).0 as i64
                        })
                        .unwrap_or(-1)
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_create_text", move |text: String| -> i64 {
                    st.lock()
                        .ok()
                        .map(|mut s| {
                            s.dirty = true;
                            s.doc.create_text_node(&text).0 as i64
                        })
                        .unwrap_or(-1)
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_append_child", move |parent: i64, child: i64| -> bool {
                    if parent < 0 || child < 0 {
                        return false;
                    }
                    st.lock()
                        .ok()
                        .map(|mut s| {
                            let ok = s.doc.append_child(
                                DomNodeId(parent as usize),
                                DomNodeId(child as usize),
                            );
                            if ok {
                                s.dirty = true;
                            }
                            ok
                        })
                        .unwrap_or(false)
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_remove_node", move |id: i64| -> bool {
                    if id < 0 {
                        return false;
                    }
                    st.lock()
                        .ok()
                        .map(|mut s| {
                            let ok = s.doc.remove_node(DomNodeId(id as usize));
                            if ok {
                                s.dirty = true;
                            }
                            ok
                        })
                        .unwrap_or(false)
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_children", move |id: i64| -> Vec<i64> {
                    if id < 0 {
                        return Vec::new();
                    }
                    st.lock()
                        .ok()
                        .and_then(|s| {
                            s.doc.node(DomNodeId(id as usize)).map(|n| {
                                n.children
                                    .iter()
                                    .copied()
                                    .filter(|&cid| s.doc.element(cid).is_some())
                                    .map(|cid| cid.0 as i64)
                                    .collect()
                            })
                        })
                        .unwrap_or_default()
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_parent", move |id: i64| -> i64 {
                    if id < 0 {
                        return -1;
                    }
                    st.lock()
                        .ok()
                        .and_then(|s| s.doc.node(DomNodeId(id as usize)).and_then(|n| n.parent))
                        .and_then(|p| {
                            st.lock()
                                .ok()
                                .and_then(|s| s.doc.element(p).map(|_| p.0 as i64))
                        })
                        .unwrap_or(-1)
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_get_title", move || -> String {
                    st.lock()
                        .ok()
                        .and_then(|s| s.doc.title())
                        .unwrap_or_default()
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_set_title", move |title: String| {
                    if let Ok(mut s) = st.lock() {
                        s.doc.set_title(&title);
                        s.dirty = true;
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_alert", move |msg: String| {
                    if let Ok(mut s) = st.lock() {
                        s.alerts.push(msg);
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_open_url", move |url: String| {
                    if let Ok(mut s) = st.lock() {
                        s.open_urls.push(url);
                    }
                });
            }
            {
                let st = Arc::clone(&state);
                bind_fn!("__bf_log", move |level: String, msg: String| {
                    if let Ok(mut s) = st.lock() {
                        s.console_logs.push(format!("[{level}] {msg}"));
                    }
                });
            }

            // Canvas 2D bindings (lazily allocate `CanvasSurface` under `graphics` feature).
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_ensure", move |id: i64| -> bool {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return false;
                        }
                        let nid = DomNodeId(id as usize);
                        let Ok(mut s) = st.lock() else {
                            return false;
                        };
                        let (w, h) = canvas_attrs_dims(&s.doc, nid);
                        s.canvases
                            .entry(nid)
                            .or_insert_with(|| crate::canvas::CanvasSurface::new(w, h));
                        true
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = id;
                        false
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_resize", move |id: i64, w: i64, h: i64| {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return;
                        }
                        let nid = DomNodeId(id as usize);
                        if let Ok(mut s) = st.lock() {
                            if let Some(surface) = s.canvases.get_mut(&nid) {
                                surface.resize(w.max(1) as u32, h.max(1) as u32);
                                s.dirty = true;
                            }
                        }
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = (id, w, h);
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_get_str",
                    move |id: i64, prop: String| -> String {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return String::new();
                            }
                            let Ok(s) = st.lock() else {
                                return String::new();
                            };
                            let Some(surface) = s.canvases.get(&DomNodeId(id as usize)) else {
                                return String::new();
                            };
                            match prop.as_str() {
                                "fillStyle" => surface.fill_style().to_string(),
                                "strokeStyle" => surface.stroke_style().to_string(),
                                "font" => surface.font().to_string(),
                                _ => String::new(),
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, prop);
                            String::new()
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_set_str",
                    move |id: i64, prop: String, val: String| {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return;
                            }
                            if let Ok(mut s) = st.lock() {
                                if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                    match prop.as_str() {
                                        "fillStyle" => surface.set_fill_style(&val),
                                        "strokeStyle" => surface.set_stroke_style(&val),
                                        "lineCap" => surface.set_line_cap(&val),
                                        "lineJoin" => surface.set_line_join(&val),
                                        "font" => surface.set_font(&val),
                                        "textAlign" => surface.set_text_align(&val),
                                        "textBaseline" => surface.set_text_baseline(&val),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, prop, val);
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_get_num", move |id: i64, prop: String| -> f64 {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return 0.0;
                        }
                        let Ok(s) = st.lock() else {
                            return 0.0;
                        };
                        let Some(surface) = s.canvases.get(&DomNodeId(id as usize)) else {
                            return 0.0;
                        };
                        match prop.as_str() {
                            "lineWidth" => surface.line_width() as f64,
                            "globalAlpha" => surface.global_alpha() as f64,
                            _ => 0.0,
                        }
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = (id, prop);
                        0.0
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_set_num",
                    move |id: i64, prop: String, val: f64| {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return;
                            }
                            if let Ok(mut s) = st.lock() {
                                if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                    match prop.as_str() {
                                        "lineWidth" => surface.set_line_width(val as f32),
                                        "globalAlpha" => surface.set_global_alpha(val as f32),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, prop, val);
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_set_dash", move |id: i64, segs: Vec<f64>| {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return;
                        }
                        if let Ok(mut s) = st.lock() {
                            if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                surface.set_line_dash(segs.into_iter().map(|v| v as f32).collect());
                            }
                        }
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = (id, segs);
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_op0", move |id: i64, op: String| {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return;
                        }
                        if let Ok(mut s) = st.lock() {
                            let mut drew = false;
                            if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                match op.as_str() {
                                    "save" => surface.save(),
                                    "restore" => surface.restore(),
                                    "resetTransform" => surface.reset_transform(),
                                    "beginPath" => surface.begin_path(),
                                    "closePath" => surface.close_path(),
                                    "fill" => {
                                        surface.fill(false);
                                        drew = true;
                                    }
                                    "fillEvenOdd" => {
                                        surface.fill(true);
                                        drew = true;
                                    }
                                    "stroke" => {
                                        surface.stroke();
                                        drew = true;
                                    }
                                    _ => {}
                                }
                            }
                            if drew {
                                s.dirty = true;
                            }
                        }
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = (id, op);
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_op1", move |id: i64, op: String, a: f64| {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return;
                        }
                        if let Ok(mut s) = st.lock() {
                            if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                if op == "rotate" {
                                    surface.rotate(a as f32);
                                }
                            }
                        }
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = (id, op, a);
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_op2",
                    move |id: i64, op: String, a: f64, b: f64| {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return;
                            }
                            if let Ok(mut s) = st.lock() {
                                if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                    match op.as_str() {
                                        "translate" => surface.translate(a as f32, b as f32),
                                        "scale" => surface.scale(a as f32, b as f32),
                                        "moveTo" => surface.move_to(a as f32, b as f32),
                                        "lineTo" => surface.line_to(a as f32, b as f32),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, op, a, b);
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_op4",
                    move |id: i64, op: String, a: f64, b: f64, c: f64, d: f64| {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return;
                            }
                            if let Ok(mut s) = st.lock() {
                                let mut drew = false;
                                if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                    match op.as_str() {
                                        "fillRect" => {
                                            surface.fill_rect(a as f32, b as f32, c as f32, d as f32);
                                            drew = true;
                                        }
                                        "strokeRect" => {
                                            surface.stroke_rect(a as f32, b as f32, c as f32, d as f32);
                                            drew = true;
                                        }
                                        "clearRect" => {
                                            surface.clear_rect(a as f32, b as f32, c as f32, d as f32);
                                            drew = true;
                                        }
                                        "rect" => surface.rect(a as f32, b as f32, c as f32, d as f32),
                                        "quadraticCurveTo" => surface
                                            .quadratic_curve_to(a as f32, b as f32, c as f32, d as f32),
                                        _ => {}
                                    }
                                }
                                if drew {
                                    s.dirty = true;
                                }
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, op, a, b, c, d);
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_op6",
                    move |id: i64, op: String, args: Vec<f64>| {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 || args.len() < 6 {
                                return;
                            }
                            if let Ok(mut s) = st.lock() {
                                if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                    match op.as_str() {
                                        "setTransform" => surface.set_transform(
                                            args[0] as f32,
                                            args[1] as f32,
                                            args[2] as f32,
                                            args[3] as f32,
                                            args[4] as f32,
                                            args[5] as f32,
                                        ),
                                        "bezierCurveTo" => surface.bezier_curve_to(
                                            args[0] as f32,
                                            args[1] as f32,
                                            args[2] as f32,
                                            args[3] as f32,
                                            args[4] as f32,
                                            args[5] as f32,
                                        ),
                                        "arc" => surface.arc(
                                            args[0] as f32,
                                            args[1] as f32,
                                            args[2] as f32,
                                            args[3] as f32,
                                            args[4] as f32,
                                            args[5] != 0.0,
                                        ),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, op, args);
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_ellipse", move |id: i64, args: Vec<f64>| {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 || args.len() < 8 {
                            return;
                        }
                        if let Ok(mut s) = st.lock() {
                            if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                surface.ellipse(
                                    args[0] as f32,
                                    args[1] as f32,
                                    args[2] as f32,
                                    args[3] as f32,
                                    args[4] as f32,
                                    args[5] as f32,
                                    args[6] as f32,
                                    args[7] != 0.0,
                                );
                            }
                        }
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = (id, args);
                    }
                });
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_text",
                    move |id: i64, mode: String, text: String, x: f64, y: f64| {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return;
                            }
                            if let Ok(mut s) = st.lock() {
                                let mut drew = false;
                                if let Some(surface) = s.canvases.get_mut(&DomNodeId(id as usize)) {
                                    if mode == "stroke" {
                                        surface.stroke_text(&text, x as f32, y as f32);
                                    } else {
                                        surface.fill_text(&text, x as f32, y as f32);
                                    }
                                    drew = true;
                                }
                                if drew {
                                    s.dirty = true;
                                }
                            }
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, mode, text, x, y);
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!(
                    "__bf_canvas_measure_text",
                    move |id: i64, text: String| -> f64 {
                        #[cfg(feature = "graphics")]
                        {
                            if id < 0 {
                                return 0.0;
                            }
                            let Ok(s) = st.lock() else {
                                return 0.0;
                            };
                            s.canvases
                                .get(&DomNodeId(id as usize))
                                .map(|c| c.measure_text(&text) as f64)
                                .unwrap_or(0.0)
                        }
                        #[cfg(not(feature = "graphics"))]
                        {
                            let _ = (id, text);
                            0.0
                        }
                    }
                );
            }
            {
                #[cfg(feature = "graphics")]
                let st = Arc::clone(&state);
                bind_fn!("__bf_canvas_to_data_url", move |id: i64| -> String {
                    #[cfg(feature = "graphics")]
                    {
                        if id < 0 {
                            return String::new();
                        }
                        let nid = DomNodeId(id as usize);
                        let Ok(mut s) = st.lock() else {
                            return String::new();
                        };
                        let (w, h) = canvas_attrs_dims(&s.doc, nid);
                        let surface = s
                            .canvases
                            .entry(nid)
                            .or_insert_with(|| crate::canvas::CanvasSurface::new(w, h));
                        surface.to_data_url().unwrap_or_default()
                    }
                    #[cfg(not(feature = "graphics"))]
                    {
                        let _ = id;
                        String::new()
                    }
                });
            }

            ctx.eval::<Value, _>(BF_DOM_SHIM)
                .catch(&ctx)
                .map(|_| ())
                .map_err(|e| format!("{e}"))
        })
    }
}

#[cfg(feature = "graphics")]
fn canvas_attrs_dims(doc: &crate::dom::DomDocument, id: DomNodeId) -> (u32, u32) {
    let w = doc
        .get_attr(id, "width")
        .and_then(|v| v.trim().trim_end_matches("px").parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(crate::canvas::DEFAULT_CANVAS_WIDTH);
    let h = doc
        .get_attr(id, "height")
        .and_then(|v| v.trim().trim_end_matches("px").parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(crate::canvas::DEFAULT_CANVAS_HEIGHT);
    (w, h)
}

struct DeadlineGuard<'a> {
    deadline_ms: &'a AtomicU64,
}

fn arm_deadline(epoch: Instant, deadline_ms: &AtomicU64) -> DeadlineGuard<'_> {
    let now_ms = epoch.elapsed().as_millis() as u64;
    let limit = now_ms
        .saturating_add(JS_EXECUTION_TIMEOUT.as_millis() as u64)
        .max(1);
    deadline_ms.store(limit, Ordering::Relaxed);
    DeadlineGuard { deadline_ms }
}

impl Drop for DeadlineGuard<'_> {
    fn drop(&mut self) {
        self.deadline_ms.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use crate::render::{HtmlTheme, RenderedHtml};
    use crate::session::{HtmlEffect, HtmlViewSession};

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
    fn static_html_does_not_allocate_js_runtime() {
        let session = HtmlViewSession::new(
            Some("Static".into()),
            "<h1>Hello</h1><p>No scripts here.</p><a href=\"https://example.com\">Link</a>".into(),
        );
        assert!(!session.js_active);
    }

    #[test]
    fn onclick_button_mutates_dom_and_render_reflects_change() {
        let html = r#"<button onclick="document.getElementById('out').textContent = 'Clicked!'">Go</button><div id="out">Init</div>"#;
        let mut session = HtmlViewSession::new(None, html.into());
        assert!(session.js_active);

        let theme = HtmlTheme::dark();
        let before = session.render(80, &theme, false);
        assert!(rendered_text(before).contains("Init"));
        let btn = before.controls[0].clone();

        let effects = session.click_at(btn.line, btn.col_start + 1);
        assert!(effects.contains(&HtmlEffect::DomChanged));

        let after = session.render(80, &theme, false);
        let text = rendered_text(after);
        assert!(text.contains("Clicked!"));
        assert!(!text.contains("Init"));
    }

    #[test]
    fn add_event_listener_toggles_class_and_display_none() {
        let html = r#"
            <style>.hidden { display: none; }</style>
            <button id="toggle">Toggle</button>
            <div id="panel" class="hidden">Secret Panel</div>
            <div id="banner">Visible Banner</div>
            <script>
              document.getElementById('toggle').addEventListener('click', function() {
                document.getElementById('panel').classList.toggle('hidden');
                var b = document.getElementById('banner');
                b.style.display = (b.style.display === 'none') ? 'block' : 'none';
              });
            </script>
        "#;
        let mut session = HtmlViewSession::new(None, html.into());
        assert!(session.js_active);

        let theme = HtmlTheme::dark();
        let r0 = session.render(80, &theme, false);
        let t0 = rendered_text(r0);
        assert!(!t0.contains("Secret Panel"));
        assert!(t0.contains("Visible Banner"));
        let btn = r0.controls[0].clone();

        session.click_at(btn.line, btn.col_start);
        let r1 = session.render(80, &theme, false);
        let t1 = rendered_text(r1);
        assert!(t1.contains("Secret Panel"));
        assert!(!t1.contains("Visible Banner"));

        session.click_at(btn.line, btn.col_start);
        let r2 = session.render(80, &theme, false);
        let t2 = rendered_text(r2);
        assert!(!t2.contains("Secret Panel"));
        assert!(t2.contains("Visible Banner"));
    }

    #[test]
    fn alert_produces_toast_effect() {
        let html = r#"<button onclick="alert('Hello')">Say Hi</button>"#;
        let mut session = HtmlViewSession::new(None, html.into());
        let theme = HtmlTheme::dark();
        let r = session.render(80, &theme, false);
        let btn = r.controls[0].clone();

        let effects = session.click_at(btn.line, btn.col_start);
        assert!(effects.contains(&HtmlEffect::Toast("Hello".to_string())));
    }

    #[test]
    fn infinite_loop_aborts_safely_within_timeout() {
        let start = std::time::Instant::now();
        let html = r#"<p>Safe page</p><script>while(true){}</script>"#;
        let session = HtmlViewSession::new(None, html.into());
        let elapsed = start.elapsed();

        assert!(session.js_active);
        assert!(
            session.last_js_error().is_some(),
            "infinite loop should capture a non-fatal JS timeout error"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "infinite loop took too long: {elapsed:?}"
        );
    }
}
