// Injected by qur_browser at document end, main frame only.
// Talks back through window.webkit.messageHandlers.qur with small JSON strings.
(function () {
  "use strict";
  if (window.__qur) return;
  var post = function (m) {
    try { window.webkit.messageHandlers.qur.postMessage(JSON.stringify(m)); } catch (_) {}
  };
  var Z = "2147483647";

  // ---- SPA navigation hooks ----
  var lastHref = location.href;
  var navChanged = function () {
    if (location.href === lastHref) return;
    lastHref = location.href;
    hideBox();
    post({ t: "nav", url: location.href, title: document.title || "" });
  };
  ["pushState", "replaceState"].forEach(function (k) {
    var orig = history[k];
    history[k] = function () {
      var r = orig.apply(this, arguments);
      try { navChanged(); } catch (_) {}
      return r;
    };
  });
  addEventListener("popstate", navChanged, true);
  addEventListener("hashchange", navChanged, true);

  // ---- inspector ----
  var on = false, accent = "#ff7a45", box = null, label = null, hovered = null;

  function isOverlay(el) { return el === box || el === label; }
  function mk() {
    var d = document.createElement("div");
    d.setAttribute("data-qur-overlay", "");
    d.style.cssText = "position:fixed;pointer-events:none;z-index:" + Z +
      ";margin:0;padding:0;box-sizing:border-box;display:none;";
    (document.documentElement || document.body).appendChild(d);
    return d;
  }
  function ensure() {
    if (!box || !box.isConnected) box = mk();
    if (!label || !label.isConnected) label = mk();
    box.style.outline = "2px solid " + accent;
    box.style.outlineOffset = "-1px";
    box.style.background = "transparent";
    label.style.cssText += ";background:" + accent +
      ";color:#fff;font:600 11px/16px -apple-system,system-ui,sans-serif;padding:0 5px;" +
      "border-radius:3px;white-space:nowrap;";
  }
  function hideBox() {
    hovered = null;
    if (box) box.style.display = "none";
    if (label) label.style.display = "none";
  }
  function removeOverlays() {
    hovered = null;
    if (box) box.remove();
    if (label) label.remove();
    box = label = null;
  }
  function targetOf(e) {
    var p = e.composedPath ? e.composedPath() : [];
    for (var i = 0; i < p.length; i++) {
      if (p[i] && p[i].nodeType === 1 && !isOverlay(p[i])) return p[i];
    }
    var t = e.target;
    return t && t.nodeType === 1 && !isOverlay(t) ? t : null;
  }
  function show(el) {
    ensure();
    var r = el.getBoundingClientRect();
    box.style.left = r.left + "px"; box.style.top = r.top + "px";
    box.style.width = r.width + "px"; box.style.height = r.height + "px";
    box.style.display = "block";
    var name = el.tagName.toLowerCase();
    if (el.id) name += "#" + el.id;
    else if (el.classList.length) name += "." + el.classList[0];
    label.textContent = name + "  " + Math.round(r.width) + "×" + Math.round(r.height);
    label.style.display = "block";
    var ty = r.top - 18;
    if (ty < 0) ty = Math.min(r.bottom + 2, innerHeight - 18);
    label.style.top = ty + "px";
    label.style.left = Math.max(0, Math.min(r.left, innerWidth - label.offsetWidth)) + "px";
  }

  function uniqueId(el) {
    if (!el.id) return false;
    try { return document.querySelectorAll("#" + CSS.escape(el.id)).length === 1; } catch (_) { return false; }
  }
  function selectorOf(el) {
    if (uniqueId(el)) return "#" + CSS.escape(el.id);
    var parts = [], e = el;
    while (e && e.nodeType === 1 && parts.length < 6) {
      if (parts.length && uniqueId(e)) { parts.unshift("#" + CSS.escape(e.id)); break; }
      var s = e.tagName.toLowerCase();
      if (e === document.body || e === document.documentElement) { parts.unshift(s); break; }
      var par = e.parentElement;
      if (par) {
        var same = [];
        for (var i = 0; i < par.children.length; i++)
          if (par.children[i].tagName === e.tagName) same.push(par.children[i]);
        if (same.length > 1) s += ":nth-of-type(" + (same.indexOf(e) + 1) + ")";
      }
      parts.unshift(s);
      e = par;
    }
    return parts.join(" > ");
  }
  function attrSource(el) {
    return el.getAttribute("data-qur-source") || el.getAttribute("data-source");
  }
  // Own attribute, then React dev _debugSource, then the nearest tagged ancestor.
  function sourceOf(el) {
    var a = attrSource(el);
    if (a) return a;
    try {
      var key = Object.keys(el).find(function (k) { return k.indexOf("__reactFiber$") === 0; });
      var f = key ? el[key] : null;
      for (var n = 0; f && n < 30; n++, f = f.return) {
        var s = f._debugSource;
        if (s && s.fileName) return s.fileName + ":" + s.lineNumber;
      }
    } catch (_) {}
    var up = el.parentElement && el.parentElement.closest("[data-qur-source],[data-source]");
    return up ? attrSource(up) : null;
  }
  var STYLE_KEYS = ["display", "position", "width", "height", "margin", "padding", "color",
    "background-color", "font-family", "font-size", "font-weight", "line-height", "border",
    "border-radius", "gap", "flex-direction", "justify-content", "align-items",
    "grid-template-columns"];
  function pick(el) {
    var cs = getComputedStyle(el), styles = [];
    STYLE_KEYS.forEach(function (k) { var v = cs.getPropertyValue(k); if (v) styles.push([k, v]); });
    var r = el.getBoundingClientRect();
    var text = (el.innerText || el.textContent || "").trim();
    if (text.length > 300) text = text.slice(0, 300);
    var html = el.outerHTML || "";
    if (html.length > 3000) html = html.slice(0, 3000) + "…[truncated]";
    post({
      t: "pick", selector: selectorOf(el), tag: el.tagName.toLowerCase(),
      id: el.id || null, classes: Array.prototype.slice.call(el.classList),
      text: text, html: html, styles: styles,
      rect: [r.left, r.top, r.width, r.height], url: location.href, source: sourceOf(el)
    });
  }
  function hasSelection() {
    var s = getSelection();
    return s && !s.isCollapsed && s.toString().trim() !== "";
  }

  function onMove(e) {
    var el = targetOf(e);
    if (!el || el === hovered) return;
    hovered = el; show(el);
  }
  function swallow(e) { e.stopImmediatePropagation(); }
  function onClick(e) {
    e.preventDefault(); e.stopImmediatePropagation();
    if (e.type !== "click" || hasSelection()) return;
    var el = targetOf(e);
    if (el) { show(el); pick(el); }
  }
  function onUp(e) {
    e.stopImmediatePropagation();
    if (hasSelection()) {
      var t = getSelection().toString().trim();
      if (t.length > 5000) t = t.slice(0, 5000);
      post({ t: "text", text: t, url: location.href });
    }
  }
  function onKey(e) {
    if (e.key !== "Escape") return;
    e.preventDefault(); e.stopImmediatePropagation();
    inspect(false);
    post({ t: "esc" });
  }
  function onScroll() { if (hovered && hovered.isConnected) show(hovered); else hideBox(); }

  var H = [["mousemove", onMove], ["pointermove", onMove], ["mouseover", onMove],
    ["pointerdown", swallow], ["mousedown", swallow], ["pointerup", swallow],
    ["mouseup", onUp], ["click", onClick], ["dblclick", onClick], ["auxclick", onClick],
    ["contextmenu", onClick], ["keydown", onKey]];
  function inspect(v, color) {
    if (color) accent = String(color);
    v = !!v;
    if (v === on) { if (v) ensure(); return; }
    on = v;
    H.forEach(function (h) { (v ? addEventListener : removeEventListener)(h[0], h[1], true); });
    (v ? addEventListener : removeEventListener)("scroll", onScroll, true);
    if (!v) removeOverlays();
  }

  addEventListener("pagehide", removeOverlays, true);
  // The web view's title can lag didFinish; report the real one once the DOM is in.
  post({ t: "nav", url: location.href, title: document.title || "" });
  window.__qur = { inspect: inspect };
})();
