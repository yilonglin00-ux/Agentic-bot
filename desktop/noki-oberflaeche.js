/* Noki · Oberfläche (Darstellung) — THE theme source for every Noki page
 * (Ask/Work/Code, Noki Terminal, Projekte, Settings, character page).
 * One canonical default (STANDARD), one font list (SCHRIFTEN), one place that
 * turns the stored choice into CSS tokens on <html>. Runs only on load and
 * when the setting changes (event) - no timer, no rAF. */
(function () {
  'use strict';
  // Canonical product default. Reset = exactly this.
  // Product decision: the UI default font is the SYSTEM font.
  var STANDARD = { flaeche: null, text: null, schrift: 'system', material: 'normal', groesse: 'standard' };
  // Default warm beige (flaeche = null): the tuned Noki palette.
  var BEIGE = { bg: [30, 28, 24], tief: [25, 23, 20], raised: [38, 35, 30], tint: '255, 244, 222', accent: [226, 214, 181], text: [239, 235, 226] };
  // Old preset keys (earlier version) keep working.
  var ALT = { sand: { h: 36, s: 0.30, v: 0.125 }, salbei: { h: 95, s: 0.17, v: 0.115 }, graphit: { h: 240, s: 0.03, v: 0.112 } };
  // Local macOS fonts only (never downloaded or bundled). Availability is
  // checked on the page that shows the picker.
  var SCHRIFTEN = [
    ['system', 'System', '-apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif', 'Neutral'],
    ['helvetica-neue', 'Helvetica Neue', '"Helvetica Neue", Helvetica, Arial, sans-serif', 'Neutral'],
    ['helvetica', 'Helvetica', 'Helvetica, Arial, sans-serif', 'Neutral'],
    ['arial', 'Arial', 'Arial, Helvetica, sans-serif', 'Neutral'],
    ['avenir', 'Avenir Next', '"Avenir Next", Avenir, -apple-system, sans-serif', 'Neutral'],
    ['avenir-classic', 'Avenir', 'Avenir, "Avenir Next", sans-serif', 'Neutral'],
    ['futura', 'Futura', 'Futura, "Avenir Next", sans-serif', 'Neutral'],
    ['gill', 'Gill Sans', '"Gill Sans", "Gill Sans MT", sans-serif', 'Neutral'],
    ['optima', 'Optima', 'Optima, sans-serif', 'Neutral'],
    ['lucida', 'Lucida Grande', '"Lucida Grande", sans-serif', 'Neutral'],
    ['geneva', 'Geneva', 'Geneva, sans-serif', 'Neutral'],
    ['seravek', 'Seravek', 'Seravek, sans-serif', 'Neutral'],
    ['pt-sans', 'PT Sans', '"PT Sans", sans-serif', 'Neutral'],
    ['skia', 'Skia', 'Skia, sans-serif', 'Neutral'],
    ['din-alt', 'DIN Alternate', '"DIN Alternate", sans-serif', 'Neutral'],
    ['din-cond', 'DIN Condensed', '"DIN Condensed", sans-serif', 'Markant'],
    ['impact', 'Impact', 'Impact, "Arial Narrow Bold", "Arial Narrow", sans-serif', 'Markant'],
    ['arialblack', 'Arial Black', '"Arial Black", Arial, sans-serif', 'Markant'],
    ['arial-narrow', 'Arial Narrow', '"Arial Narrow", Arial, sans-serif', 'Markant'],
    ['copperplate', 'Copperplate', 'Copperplate, "Copperplate Gothic Light", serif', 'Markant'],
    ['rockwell', 'Rockwell', 'Rockwell, serif', 'Markant'],
    ['phosphate', 'Phosphate', 'Phosphate, sans-serif', 'Markant'],
    ['superclarendon', 'Superclarendon', 'Superclarendon, serif', 'Markant'],
    ['georgia', 'Georgia', 'Georgia, serif', 'Serif'],
    ['times', 'Times New Roman', '"Times New Roman", Times, serif', 'Serif'],
    ['palatino', 'Palatino', 'Palatino, "Palatino Linotype", serif', 'Serif'],
    ['baskerville', 'Baskerville', 'Baskerville, "Baskerville Old Face", serif', 'Serif'],
    ['didot', 'Didot', 'Didot, "Bodoni 72", serif', 'Serif'],
    ['bodoni', 'Bodoni 72', '"Bodoni 72", Didot, serif', 'Serif'],
    ['hoefler', 'Hoefler Text', '"Hoefler Text", Georgia, serif', 'Serif'],
    ['big-caslon', 'Big Caslon', '"Big Caslon", serif', 'Serif'],
    ['cochin', 'Cochin', 'Cochin, serif', 'Serif'],
    ['charter', 'Charter', 'Charter, serif', 'Serif'],
    ['iowan', 'Iowan Old Style', '"Iowan Old Style", serif', 'Serif'],
    ['pt-serif', 'PT Serif', '"PT Serif", serif', 'Serif'],
    ['trebuchet', 'Trebuchet MS', '"Trebuchet MS", sans-serif', 'Freundlich'],
    ['verdana', 'Verdana', 'Verdana, sans-serif', 'Freundlich'],
    ['chalkboard', 'Chalkboard', 'Chalkboard, "Chalkboard SE", sans-serif', 'Freundlich'],
    ['chalkboard-se', 'Chalkboard SE', '"Chalkboard SE", sans-serif', 'Freundlich'],
    ['marker-felt', 'Marker Felt', '"Marker Felt", sans-serif', 'Freundlich'],
    ['noteworthy', 'Noteworthy', 'Noteworthy, sans-serif', 'Freundlich'],
    ['bradley', 'Bradley Hand', '"Bradley Hand", cursive', 'Freundlich'],
    ['comic', 'Comic Sans MS', '"Comic Sans MS", sans-serif', 'Freundlich'],
    ['papyrus', 'Papyrus', 'Papyrus, fantasy', 'Freundlich'],
    ['luminari', 'Luminari', 'Luminari, fantasy', 'Freundlich'],
    ['american-typewriter', 'American Typewriter', '"American Typewriter", serif', 'Schreibmaschine'],
    ['menlo', 'Menlo', 'Menlo, monospace', 'Schreibmaschine'],
    ['monaco', 'Monaco', 'Monaco, monospace', 'Schreibmaschine'],
    ['courier-new', 'Courier New', '"Courier New", Courier, monospace', 'Schreibmaschine'],
    ['sf-mono', 'SF Mono', '"SF Mono", ui-monospace, monospace', 'Schreibmaschine']
  ];
  var GLAS = { mehr: 0.6, normal: 1, weniger: 1.6 };
  var ZOOM = { kompakt: 1, standard: 1, gross: 1 };   // UI size = native webview zoom (see oberflaeche_zoom)
  var globalstil = document.createElement('style');
  globalstil.textContent = [
    'html, body, body * { font-family: var(--noki-ui-font) !important; font-synthesis: none; }',
    'body :not(pre):not(pre *):not(code):not(code *):not(.nc-term):not(.nc-term *):not(.nc-shell-pane):not(.nc-shell-pane *):not(.nc-tab-shell):not(.nc-tab-shell *):not(.nc-ts-slot):not(.nc-ts-slot *):not(.nc-diff):not(.nc-diff *):not(.nc-d):not(.nc-p-code):not(.nc-vl-pre):not(.nc-vl-datei):not(.nc-vl-datei *):not(.nc-ergebnis):not(.nc-ergebnis *):not(.nc-audit):not(.nc-audit *):not(.nc-log):not(.nc-log *):not(.nc-ev):not(.nc-ev *):not(.nc-b):not(.nc-b1):not(.nc-note):not(.nc-info):not(.nc-live):not(.nc-prompt textarea):not(.nc-verlauf-lauf):not(.nc-verlauf-lauf *):not(.nc-v-pre):not(.nc-v-pre *):not(.ni-preview-text):not(.ni-code-liste):not(.ni-code-fehler):not(.ni-diff-add):not(.ni-diff-del):not(.ni-diff-ctx):not(.ni-diff-hunk):not(.ni-diff-plus):not(.ni-diff-minus):not(.nc-p-file):not(.nc-p-path):not(.n-pfad):not(.e-kv-mono):not(.e-font-chip):not(.e-font-chip *) { color: var(--noki-text) !important; font-family: var(--noki-ui-font) !important; font-synthesis: none; }',
    '#askNoki :not(pre):not(pre *):not(code):not(code *):not(.nc-term):not(.nc-term *):not(.nc-shell-pane):not(.nc-shell-pane *):not(.nc-tab-shell):not(.nc-tab-shell *):not(.nc-ts-slot):not(.nc-ts-slot *):not(.nc-diff):not(.nc-diff *):not(.nc-d):not(.nc-p-code):not(.nc-vl-pre):not(.nc-vl-datei):not(.nc-vl-datei *):not(.nc-ergebnis):not(.nc-ergebnis *):not(.nc-audit):not(.nc-audit *):not(.nc-log):not(.nc-log *):not(.nc-ev):not(.nc-ev *):not(.nc-b):not(.nc-b1):not(.nc-note):not(.nc-info):not(.nc-live):not(.nc-prompt textarea):not(.nc-verlauf-lauf):not(.nc-verlauf-lauf *):not(.nc-v-pre):not(.nc-v-pre *):not(.ni-preview-text):not(.ni-code-liste):not(.ni-code-fehler):not(.ni-diff-add):not(.ni-diff-del):not(.ni-diff-ctx):not(.ni-diff-hunk):not(.ni-diff-plus):not(.ni-diff-minus):not(.nc-p-file):not(.nc-p-path):not(.e-font-chip):not(.e-font-chip *), #einstellungen :not(pre):not(pre *):not(code):not(code *):not(.n-pfad):not(.e-kv-mono):not(.e-font-chip):not(.e-font-chip *), #werk *, #ablage * { color: var(--noki-text) !important; }',
    '#askNoki .nc-term, #askNoki .nc-term *, #askNoki .nc-shell-pane, #askNoki .nc-shell-pane *, #askNoki .nc-tab-shell, #askNoki .nc-tab-shell *, #askNoki .nc-ts-slot, #askNoki .nc-ts-slot *, #askNoki .nc-diff, #askNoki .nc-diff *, #askNoki .nc-d, #askNoki .nc-p-code, #askNoki .nc-vl-pre, #askNoki .nc-vl-datei, #askNoki .nc-vl-datei *, #askNoki .nc-ergebnis, #askNoki .nc-ergebnis *, #askNoki .nc-audit, #askNoki .nc-audit *, #askNoki .nc-log, #askNoki .nc-log *, #askNoki .nc-ev, #askNoki .nc-ev *, #askNoki .nc-b, #askNoki .nc-b1, #askNoki .nc-note, #askNoki .nc-info, #askNoki .nc-live, #askNoki .nc-prompt textarea, #askNoki .nc-verlauf-lauf, #askNoki .nc-verlauf-lauf *, #askNoki .nc-v-pre, #askNoki .nc-v-pre *, #askNoki .ni-preview-text, #askNoki .ni-code-liste, #askNoki .ni-code-fehler, #askNoki .ni-diff-add, #askNoki .ni-diff-del, #askNoki .ni-diff-ctx, #askNoki .ni-diff-hunk, #askNoki .ni-diff-plus, #askNoki .ni-diff-minus, #askNoki .nc-p-file, #askNoki .nc-p-path, pre, pre *, code, code *, .n-pfad, .e-kv-mono { font-family: var(--noki-mono, ui-monospace, SFMono-Regular, Menlo, monospace) !important; }',
    '#einstellungen .e-font-chip, #einstellungen .e-font-chip * { font-family: var(--chip-font) !important; }'
  ].join('\n');
  var globalstilEinsetzen = function () { (document.head || document.documentElement).appendChild(globalstil); };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', globalstilEinsetzen, { once: true });
  else globalstilEinsetzen();

  function hsv(c) {
    var h = c.h, s = c.s, v = c.v, f = function (n) { var k = (n + h / 60) % 6; return v - v * s * Math.max(0, Math.min(k, 4 - k, 1)); };
    return [Math.round(f(5) * 255), Math.round(f(3) * 255), Math.round(f(1) * 255)];
  }
  function lum(r) { var a = r.map(function (x) { x /= 255; return x <= 0.03928 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4); }); return 0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2]; }
  function kontrast(a, b) { var x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); }
  function mix(a, b, t) { return a.map(function (x, i) { return Math.round(x + (b[i] - x) * t); }); }
  function css(r) { return 'rgb(' + r.join(', ') + ')'; }
  function farbeOk(c) { return c && isFinite(c.h) && isFinite(c.s) && isFinite(c.v); }
  function normal(o) {
    o = o || {};
    var n = {
      flaeche: farbeOk(o.flaeche) ? { h: +o.flaeche.h, s: +o.flaeche.s, v: +o.flaeche.v } : (ALT[o.farbe] || null),
      text: farbeOk(o.text) ? { h: +o.text.h, s: +o.text.s, v: +o.text.v } : null,
      schrift: SCHRIFTEN.some(function (f) { return f[0] === o.schrift; }) ? o.schrift : STANDARD.schrift,
      material: GLAS[o.material] ? o.material : STANDARD.material,
      groesse: ZOOM[o.groesse] ? o.groesse : STANDARD.groesse
    };
    return n;
  }
  // Tokens from the choice. Text keeps a readable default against any surface
  // unless the user picks one; secondary text is derived (alpha), not equal.
  function ableiten(o) {
    var bg, tief, raised, tint, accent;
    if (o.flaeche) {
      bg = hsv(o.flaeche);
      var hell = lum(bg) > 0.35;
      tief = mix(bg, hell ? [255, 255, 255] : [0, 0, 0], 0.14);
      raised = mix(bg, hell ? [0, 0, 0] : [255, 255, 255], 0.07);
      accent = hsv({ h: o.flaeche.h, s: Math.min(0.5, o.flaeche.s * 0.6 + 0.15), v: hell ? 0.42 : 0.88 });
    } else { bg = BEIGE.bg; tief = BEIGE.tief; raised = BEIGE.raised; accent = BEIGE.accent; }
    var hellBg = lum(bg) > 0.35;
    var text = o.text ? hsv(o.text) : (o.flaeche ? (hellBg ? [36, 32, 26] : [239, 235, 226]) : BEIGE.text);
    tint = o.text || o.flaeche ? text.join(', ') : BEIGE.tint;
    return { bg: bg, tief: tief, raised: raised, accent: accent, text: text, tint: tint, hellBg: hellBg, kontrast: kontrast(text, bg) };
  }
  function anwenden(roh) {
    var o = normal(roh), t = ableiten(o), r = document.documentElement.style, f = SCHRIFTEN.filter(function (x) { return x[0] === o.schrift; })[0];
    r.setProperty('--noki-bg', css(t.bg)); r.setProperty('--noki-bg-tief', css(t.tief)); r.setProperty('--noki-raised', css(t.raised));
    r.setProperty('--noki-tint', t.tint); r.setProperty('--noki-accent', css(t.accent)); r.setProperty('--noki-accent-rgb', t.accent.join(', '));
    r.setProperty('--noki-text', css(t.text)); r.setProperty('--noki-text-rgb', t.text.join(', '));
    r.setProperty('--noki-text-secondary', 'rgba(' + t.text.join(', ') + ', 0.74)');
    r.setProperty('--noki-text-muted', 'rgba(' + t.text.join(', ') + ', 0.56)');
    r.setProperty('--noki-glas', String(GLAS[o.material]));
    r.setProperty('--noki-ui-font', f[2]);
    r.setProperty('--noki-mono', 'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Monaco, monospace');
    var d = document.documentElement;
    d.setAttribute('data-noki-schrift', o.schrift);
    // Own text colour or a light surface: the shared text rules take over.
    if (o.text || t.hellBg) d.setAttribute('data-noki-textfarbe', t.hellBg ? 'hell' : 'eigen'); else d.removeAttribute('data-noki-textfarbe');
    window.NokiOberflaeche.wert = o; window.NokiOberflaeche.tokens = t;
  }
  window.NokiOberflaeche = { anwenden: anwenden, normal: normal, ableiten: ableiten, hsv: hsv, kontrast: kontrast,
    STANDARD: STANDARD, SCHRIFTEN: SCHRIFTEN, BEIGE: BEIGE, wert: null, tokens: null };
  anwenden(null);
  var T = window.__TAURI__;
  if (!T || !T.core) return;
  T.core.invoke('oberflaeche_lesen').then(anwenden).catch(function () {});
  if (T.event && T.event.listen) T.event.listen('noki://oberflaeche', function (e) { anwenden(e && e.payload); });
})();
