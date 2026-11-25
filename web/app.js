/* skimonitor web client: paints the cell frames the server sends and forwards input.
   There is no layout logic here on purpose — every glyph position, colour and
   border is decided by the server's ratatui buffer, which is the same code the
   TUI draws with. This file only turns cells into DOM and keys into actions. */

(function () {
  'use strict';

  var screenEl = document.getElementById('screen');
  var root = document.documentElement;

  var FS = 16;          // font px
  var LH = 23;          // cell height px (1.4375 * FS, close to the docs shot)
  var cw = 9.6;         // measured cell width px
  var cols = 0, rows = 0;
  var rowEls = [], rowHtml = [];
  var seq = -1;

  /* glyphs painted as geometry (see style.css) — bars must tile the cell and
     braille is missing from every common monospace font */
  var GLYPH = {
    '\u2588': 'ge g8',   // █
    '\u2591': 'ge sh',   // ░
    '\u2581': 'ge g1', '\u2582': 'ge g2', '\u2583': 'ge g3', '\u2584': 'ge g4',
    '\u2585': 'ge g5', '\u2586': 'ge g6', '\u2587': 'ge g7',
    '\u258F': 'ge l1', '\u258E': 'ge l2', '\u258D': 'ge l3', '\u258C': 'ge l4',
    '\u258B': 'ge l5', '\u258A': 'ge l6', '\u2589': 'ge l7'
  };

  function charClass(ch) {
    var code = ch.codePointAt(0);
    if (code >= 0x2800 && code <= 0x28FF) {
      return 'br b' + (code - 0x2800).toString(16);
    }
    return GLYPH[ch] || '';
  }

  /* 256 braille rules: dots 1-8 of the 8-dot layout, positioned like the
     project's screenshot renderer (2 columns x 4 rows inside the cell). */
  function brailleCss() {
    var byBit = {
      0: [25, 15], 1: [25, 35], 2: [25, 55], 3: [75, 15],
      4: [75, 35], 5: [75, 55], 6: [25, 75], 7: [75, 75]
    };
    var css = '';
    for (var b = 0; b < 256; b++) {
      var layers = [];
      for (var i = 0; i < 8; i++) {
        if (b & (1 << i)) {
          var p = byBit[i];
          layers.push('radial-gradient(circle at ' + p[0] + '% ' + p[1] +
            '%, currentColor 0 calc(var(--fs) * 0.088), transparent calc(var(--fs) * 0.125))');
        }
      }
      if (layers.length) {
        css += '.br.b' + b.toString(16) + '{background-image:' + layers.join(',') + '}\n';
      }
    }
    return css;
  }

  var styleTag = document.createElement('style');
  styleTag.textContent = brailleCss();
  document.head.appendChild(styleTag);

  function escapeHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function styleCss(pal) {
    var css = '';
    if (pal[0]) css += 'color:' + pal[0] + ';';
    if (pal[1]) css += 'background-color:' + pal[1] + ';';
    if (pal[2]) css += 'font-weight:700;';
    return css;
  }

  function runHtml(text, pal) {
    var css = styleCss(pal);
    var out = '', buf = '';
    function flush() {
      if (buf) {
        out += '<span style="' + css + '">' + escapeHtml(buf) + '</span>';
        buf = '';
      }
    }
    for (var i = 0; i < text.length; ) {
      var cp = text.codePointAt(i);
      var ch = String.fromCodePoint(cp);
      i += ch.length;
      var cls = charClass(ch);
      if (cls) {
        flush();
        out += '<span class="' + cls + '" style="' + css + '"></span>';
      } else {
        buf += ch;
      }
    }
    flush();
    return out;
  }

  function buildRows() {
    var html = '';
    for (var y = 0; y < rows; y++) html += '<div class="row"></div>';
    screenEl.innerHTML = html;
    rowEls = Array.prototype.slice.call(screenEl.children);
    rowHtml = new Array(rows).fill(null);
    screenEl.style.width = (cols * cw).toFixed(2) + 'px';
    screenEl.style.height = (rows * LH) + 'px';
  }

  function render(frame) {
    if (frame.seq <= seq) return;         // stale (e.g. pulled while resizing)
    seq = frame.seq;
    if (frame.cols !== cols || frame.rows !== rows) {
      cols = frame.cols;
      rows = frame.rows;
      buildRows();
    }
    var pal = frame.pal, lines = frame.lines;
    for (var y = 0; y < rows; y++) {
      var runs = lines[y] || [];
      var html = '';
      for (var r = 0; r < runs.length; r++) {
        html += runHtml(runs[r][0], pal[runs[r][1]] || ['', '', 0]);
      }
      if (html !== rowHtml[y]) {
        rowEls[y].innerHTML = html;
        rowHtml[y] = html;
      }
    }
  }

  /* ---- sizing: the grid is whatever fits the window, one cell per cell ---- */
  function measureCell() {
    var probe = document.createElement('span');
    probe.textContent = 'M'.repeat(64);
    probe.style.cssText = 'position:absolute;left:-9999px;top:0;visibility:hidden;' +
      'white-space:pre;font-family:var(--font);font-size:' + FS + 'px;font-weight:400';
    document.body.appendChild(probe);
    var w = probe.getBoundingClientRect().width / 64;
    probe.remove();
    return w > 0 ? w : 9.6;
  }

  var sizeTimer = null;
  function fit(force) {
    cw = measureCell();
    root.style.setProperty('--fs', FS + 'px');
    root.style.setProperty('--lh', LH + 'px');
    root.style.setProperty('--cw', cw + 'px');
    var newCols = Math.max(20, Math.floor((window.innerWidth - 1) / cw));
    var newRows = Math.max(8, Math.floor(window.innerHeight / LH));
    if (newCols !== cols || newRows !== rows || force) {
      cols = newCols;
      rows = newRows;
      buildRows();
      // visible to nobody, readable by the verification scripts
      root.dataset.grid = cols + 'x' + rows;
      root.dataset.cell = cw.toFixed(3) + 'x' + LH;
      root.dataset.font = (document.fonts && document.fonts.check('16px JBM')) ? 'JBM' : 'fallback';
      send('/api/size', { cols: cols, rows: rows });
    }
  }

  /* ---- input ---------------------------------------------------------- */
  var chain = Promise.resolve();
  function send(path, body) {
    chain = chain.then(function () {
      return fetch(path, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
    }).catch(function () { /* the stream reconnects; frames resync anyway */ });
    return chain;
  }

  var KEYS = {
    Enter: 'enter', Escape: 'esc', ArrowUp: 'up', ArrowDown: 'down',
    ArrowLeft: 'left', ArrowRight: 'right', Tab: 'tab',
    PageUp: 'pageup', PageDown: 'pagedown', Home: 'home', End: 'end',
    Backspace: 'backspace', Delete: 'delete'
  };

  window.addEventListener('keydown', function (e) {
    if (e.metaKey || e.altKey) return;                 // browser shortcuts
    if (e.ctrlKey && e.key !== 'c') return;            // Ctrl-C: never forward
    var msg = null;
    if (e.key === 'Tab' && e.shiftKey) {
      msg = { k: 'backtab' };
    } else if (KEYS[e.key]) {
      msg = { k: KEYS[e.key] };
    } else if (e.key.length === 1 && !e.ctrlKey) {
      msg = { k: 'c', v: e.key };
    }
    if (!msg) return;
    e.preventDefault();
    send('/api/key', msg);
  });

  var wheelLines = 0, wheelCell = null, wheelTimer = null;
  function flushWheel() {
    if (wheelLines !== 0 && wheelCell) {
      send('/api/wheel', { x: wheelCell[0], y: wheelCell[1], d: wheelLines });
    }
    wheelLines = 0;
    wheelTimer = null;
  }
  screenEl.addEventListener('wheel', function (e) {
    e.preventDefault();
    var r = screenEl.getBoundingClientRect();
    wheelCell = [
      Math.max(0, Math.floor((e.clientX - r.left) / cw)),
      Math.max(0, Math.floor((e.clientY - r.top) / LH))
    ];
    wheelLines += (e.deltaY > 0 ? 3 : -3);
    wheelLines = Math.max(-12, Math.min(12, wheelLines));
    if (wheelTimer === null) wheelTimer = setTimeout(flushWheel, 60);
  }, { passive: false });

  screenEl.addEventListener('mousedown', function (e) {
    if (e.button !== 0) return;
    var r = screenEl.getBoundingClientRect();
    var x = Math.max(0, Math.floor((e.clientX - r.left) / cw));
    var y = Math.max(0, Math.floor((e.clientY - r.top) / LH));
    screenEl.focus();
    send('/api/click', { x: x, y: y });
  });

  window.addEventListener('resize', function () {
    if (sizeTimer) clearTimeout(sizeTimer);
    sizeTimer = setTimeout(function () { fit(false); }, 180);
  });

  /* ---- frame stream --------------------------------------------------- */
  var STATIC = /(^|[?&])static=1(&|$)/.test(location.search);

  var es = null;
  function connect() {
    if (STATIC) {
      // one-shot snapshot: pages held open by SSE never finish a headless
      // --screenshot, and this is also what docs captures use
      fetch('/api/screen?cols=' + cols + '&rows=' + rows)
        .then(function (r) { return r.json(); })
        .then(render)
        .catch(function () {});
      return;
    }
    es = new EventSource('/events');
    es.addEventListener('open', function () { seq = -1; });
    es.onmessage = function (ev) {
      try {
        render(JSON.parse(ev.data));
      } catch (err) { /* a torn frame is skipped; the next one is whole */ }
    };
  }

  function boot() {
    var ready = (document.fonts && document.fonts.ready) ? document.fonts.ready : Promise.resolve();
    ready.then(function () {
      fit(true);
      connect();
      screenEl.focus();
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
