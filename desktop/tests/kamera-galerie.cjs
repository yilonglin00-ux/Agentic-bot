// Settings › Kamera: gallery (newest first, lazy thumbnails only in the
// visible Settings window, video duration), photo viewer + editor (free
// crop, pen, eraser, undo, "Kopie speichern" default, "Original ersetzen"
// only after confirmation), video player + trim/crop export with progress
// and cancel, delete only after confirmation, refresh after Shortcut 1/2.
// Native side (kamera_* commands, nokimedien:// protocol) is mocked; the
// protocol is served under its Windows form http://nokimedien.localhost/.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const os = require('node:os');
const http = require('node:http');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');

let fehler = 0;
const check = (ok, msg) => { console.log((ok ? 'ok   ' : 'FAIL ') + msg); if (!ok) fehler++; };

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'noki-kamera-'));
const ff = (...a) => execFileSync('ffmpeg', ['-loglevel', 'error', '-y', ...a]);
ff('-f', 'lavfi', '-i', 'testsrc=size=800x500:rate=1', '-frames:v', '1', path.join(tmp, 'foto.png'));
ff('-f', 'lavfi', '-i', 'testsrc=size=160x160:rate=1', '-frames:v', '1', path.join(tmp, 'thumb.jpg'));
ff('-f', 'lavfi', '-i', 'testsrc=size=320x180:rate=25', '-t', '3', '-c:v', 'libvpx', '-b:v', '200k', path.join(tmp, 'clip.webm'));
const FOTO = fs.readFileSync(path.join(tmp, 'foto.png')), THUMB = fs.readFileSync(path.join(tmp, 'thumb.jpg')), CLIP = fs.readFileSync(path.join(tmp, 'clip.webm'));
fs.rmSync(tmp, { recursive: true, force: true });
const pngGroesse = (b) => ({ w: b.readUInt32BE(16), h: b.readUInt32BE(20) });

const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + new URL(req.url, 'http://noki').pathname);
  if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
  fs.readFile(file, (error, data) => {
    if (error) return res.writeHead(404).end();
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
    res.end(data);
  });
});

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const port = server.address().port;
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--autoplay-policy=no-user-gesture-required'] });
  const ctx = await browser.newContext({ viewport: { width: 1100, height: 900 },
    userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36' });
  const main = await ctx.newPage();
  const einst = await ctx.newPage();
  const anfragen = [];
  await ctx.route('http://nokimedien.localhost/**', (route, req) => {
    const u = new URL(req.url()), [, art, roh] = u.pathname.split('/'), name = decodeURIComponent(roh || '');
    anfragen.push({ art, name, seite: req.frame().page() === main ? 'main' : 'einst' });
    const h = { 'Access-Control-Allow-Origin': '*' };
    if (art === 't') return route.fulfill({ status: 200, headers: { ...h, 'Content-Type': 'image/jpeg' }, body: THUMB });
    if (art === 'f' && /\.mov$/.test(name)) {
      // like the real nokimedien:// protocol: HTTP Range (needed to seek)
      const r = /bytes=(\d*)-(\d*)/.exec(req.headers()['range'] || '');
      if (!r) return route.fulfill({ status: 200, headers: { ...h, 'Content-Type': 'video/webm', 'Accept-Ranges': 'bytes' }, body: CLIP });
      const a = r[1] ? +r[1] : CLIP.length - +r[2], e = r[1] && r[2] ? Math.min(+r[2], CLIP.length - 1) : CLIP.length - 1;
      return route.fulfill({ status: 206, headers: { ...h, 'Content-Type': 'video/webm', 'Accept-Ranges': 'bytes', 'Content-Range': `bytes ${a}-${e}/${CLIP.length}` }, body: CLIP.subarray(a, e + 1) });
    }
    if (art === 'f') return route.fulfill({ status: 200, headers: { ...h, 'Content-Type': 'image/png' }, body: FOTO });
    return route.fulfill({ status: 404 });
  });
  await main.exposeBinding('__nachEinst', (_, n, p) => einst.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [n, p]).catch(() => {}));
  await einst.exposeBinding('__nachMain', (_, n, p) => main.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [n, p]).catch(() => {}));
  const mock = (rolle) => {
    window.__calls = []; window.__ev = {};
    window.__emit = (n, p) => (window.__ev[n] || []).forEach(f => f({ payload: p }));
    const jetzt = Date.now();
    window.__kam = { liste: [{ name: 'Noki-Recording-1.mov', art: 'video', groesse: 4.2e6, zeit: jetzt - 1000, w: null, h: null, dauer: null }]
      .concat(Array.from({ length: 69 }, (_, i) => ({ name: i === 2 ? 'Bildschirmaufnahme 2026-10-04 um 10.42.17 mit einem wirklich sehr langen Dateinamen.png' : i === 3 ? 'a.png' : `Noki-Screenshot-${i}.png`,
        art: 'foto', groesse: 300000, zeit: jetzt - 60000 * (i + 1), w: 800, h: 500, dauer: null }))) };
    const invoke = async (name, args) => {
      window.__calls.push({ name, args });
      const K = window.__kam;
      if (name === 'kamera_medien') return JSON.parse(JSON.stringify(K.liste));
      if (name === 'kamera_loeschen') { K.liste = K.liste.filter(m => m.name !== args.name); return null; }
      if (name === 'kamera_bild_speichern') {
        const neu = args.ersetzen ? args.name : args.name.replace(/\.png$/, '-bearbeitet.png');
        if (!args.ersetzen) K.liste.unshift({ name: neu, art: 'foto', groesse: 1000, zeit: Date.now(), w: null, h: null, dauer: null });
        return neu;
      }
      if (name === 'kamera_video_export') {
        const neu = args.name.replace(/\.mov$/, '-geschnitten.mov');
        if (!window.__exportHalten) {
          setTimeout(() => window.__emit('kamera-export', { name: neu, p: 0.3 }), 100);
          setTimeout(() => { K.liste.unshift({ name: neu, art: 'video', groesse: 1, zeit: Date.now(), w: null, h: null, dauer: null }); window.__emit('kamera-export', { name: neu, fertig: true }); }, 400);
        } else setTimeout(() => window.__emit('kamera-export', { name: neu, p: 0.4 }), 100);
        return neu;
      }
      if (name === 'kamera_video_abbrechen') { setTimeout(() => window.__emit('kamera-export', { name: 'x', abgebrochen: true }), 50); return true; }
      if (name === 'fokus_sitzung_status') return { aktiv: false };
      if (name === 'intelligence_settings') return { settings: { level: 'off', ask: true, web: false }, installed: true, loaded: false };
      return null;
    };
    const emitTo = async (ziel, n, p) => {
      if (rolle === 'main' && ziel === 'einstellungen') return window.__nachEinst(n, p);
      if (rolle === 'einst' && ziel === 'main') return window.__nachMain(n, p);
    };
    window.__TAURI__ = { core: { invoke }, event: { listen: async (n, f) => { (window.__ev[n] = window.__ev[n] || []).push(f); return () => {}; }, emit: async () => {}, emitTo },
      window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
    window.__TAURI_INTERNALS__ = { invoke };
  };
  await main.addInitScript(mock, 'main');
  await einst.addInitScript(mock, 'einst');
  main.on('pageerror', e => { console.log('main pageerror:', e.message); fehler++; });
  einst.on('pageerror', e => { console.log('einst pageerror:', e.message); fehler++; });
  await main.goto(`http://127.0.0.1:${port}/index.html`, { timeout: 180000 });
  await main.waitForFunction(() => window.NokiEinstellungen && window.NokiKameraAktion, null, { timeout: 120000 });
  await main.waitForTimeout(1600);
  await einst.goto(`http://127.0.0.1:${port}/settings.html`);
  await einst.waitForTimeout(300);
  const calls = (n) => main.evaluate(n => window.__calls.filter(c => c.name === n), n);
  const bis = (fn, ms, arg) => einst.waitForFunction(fn, arg, { timeout: ms || 8000 }).catch(() => {});
  const klick = async (sel, txt) => {
    const b = txt ? einst.locator(`#einstellungen ${sel}`, { hasText: txt }).first() : einst.locator(`#einstellungen ${sel}`).first();
    await b.click({ timeout: 10000 });
    await main.waitForTimeout(150);
  };
  // drag on the Settings page: from/to as fractions of an element's box
  const ziehen = async (sel, von, nach, schritte = 8) => {
    const loc = einst.locator(`#einstellungen ${sel}`).first();
    await loc.scrollIntoViewIfNeeded();
    const b = await loc.boundingBox();
    const p = (f) => [b.x + f[0] * b.width, b.y + f[1] * b.height];
    await einst.mouse.move(...p(von)); await einst.mouse.down();
    for (let i = 1; i <= schritte; i++) await einst.mouse.move(...p([von[0] + (nach[0] - von[0]) * i / schritte, von[1] + (nach[1] - von[1]) * i / schritte]));
    await einst.mouse.up();
    await main.waitForTimeout(250);
  };

  // ---- 1. Gallery ---------------------------------------------------------
  await main.evaluate(() => window.NokiEinstellungen.auf('kamera'));
  await bis(() => document.querySelectorAll('#einstellungen .k-kachel').length > 0, 20000);
  let kacheln = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-kachel')].map(k => k.getAttribute('data-v')));
  check(kacheln.length === 60 && kacheln[0] === 'Noki-Recording-1.mov' && kacheln[1] === 'Noki-Screenshot-0.png' && kacheln[59] === 'Noki-Screenshot-58.png', `Galerie: 60 Kacheln, neueste zuerst (${kacheln.length})`);
  check(await einst.evaluate(() => /Mehr anzeigen \(10\)/.test(document.querySelector('#einstellungen').textContent)), '"Mehr anzeigen (10)" statt alles auf einmal');
  check(await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-kachel img')].every(i => i.loading === 'lazy')), 'Vorschaubilder lazy');
  check(await einst.evaluate(() => !!document.querySelector('#einstellungen .k-kachel .k-play svg')), 'Video-Kachel mit Play-Symbol');
  await main.evaluate(() => window.__emit('kamera-medien', { name: 'Noki-Recording-1.mov', w: 1920, h: 1080, dauer: 12.4 }));
  await bis(() => (document.querySelector('#einstellungen .k-kachel .k-dauer') || {}).textContent === '0:12');
  check(await einst.evaluate(() => (document.querySelector('#einstellungen .k-kachel .k-dauer') || {}).textContent) === '0:12', 'Videodauer erscheint, sobald bekannt (0:12)');
  await einst.waitForTimeout(800);
  const thumbsEinst = anfragen.filter(a => a.art === 't' && a.seite === 'einst').length, thumbsMain = anfragen.filter(a => a.art === 't' && a.seite === 'main').length;
  check(thumbsEinst > 0 && thumbsMain === 0, `Vorschauen laden nur im sichtbaren Fenster (einst ${thumbsEinst}, main ${thumbsMain})`);
  // Cards: uniform, ONE line per card, long names with an ellipsis
  const karten = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-kachel')].slice(0, 8).map(k => {
    const n = k.querySelector('.k-name'), f = k.querySelector('.k-fuss'), cs = getComputedStyle(n);
    return { name: n.textContent, h: Math.round(k.getBoundingClientRect().height), fussH: Math.round(f.getBoundingClientRect().height),
      zeilen: Math.round(n.getBoundingClientRect().height / parseFloat(cs.lineHeight || 14) ) || 1, nowrap: cs.whiteSpace === 'nowrap', ellipsis: cs.textOverflow === 'ellipsis',
      gekuerzt: n.scrollWidth > n.clientWidth, zeit: (k.querySelector('.k-zeit') || {}).textContent || '', typ: !!k.querySelector('.k-typ svg') };
  }));
  const lang = karten.find(k => /^Bildschirmaufnahme/.test(k.name)), kurz = karten.find(k => k.name === 'a.png');
  check(new Set(karten.map(k => k.h)).size === 1 && new Set(karten.map(k => k.fussH)).size === 1, `alle Karten gleich hoch (${[...new Set(karten.map(k => k.h))]} px), keine Layoutspruenge`);
  check(lang && lang.nowrap && lang.ellipsis && lang.gekuerzt && kurz && !kurz.gekuerzt, 'langer Name einzeilig mit …, kurzer Name ganz');
  check(karten.every(k => k.typ && /^(Heute|\d\d\.\d\d\.) \d\d:\d\d$/.test(k.zeit)), `Typ-Symbol + Datum/Zeit je Karte (${karten[1].zeit})`);
  if (process.env.SHOTS) await einst.screenshot({ path: process.env.SHOTS + '/galerie.png' });
  await klick('[data-e="k_mehr"]');
  await bis(() => document.querySelectorAll('#einstellungen .k-kachel').length === 70);
  check(await einst.evaluate(() => document.querySelectorAll('#einstellungen .k-kachel').length) === 70, 'Mehr anzeigen laedt den Rest');

  // ---- 2. Photo viewer + editor ------------------------------------------
  await klick('.k-kachel[data-v="Noki-Screenshot-0.png"]');
  await bis(() => document.querySelector('#einstellungen img.k-gross'));
  const gross = await einst.evaluate(() => (document.querySelector('#einstellungen img.k-gross') || {}).src || '');
  check(/\/f\/Noki-Screenshot-0\.png/.test(gross), 'Foto oeffnet die grosse Vorschau intern');
  await bis(() => (document.querySelector('#einstellungen img.k-gross') || {}).naturalWidth > 0);
  check(await einst.evaluate(() => document.querySelector('#einstellungen img.k-gross').naturalWidth) === 800, 'grosse Vorschau geladen (800 px)');
  await klick('[data-e="k_bearbeiten"]');
  await bis(() => document.querySelector('#einstellungen .k-buehne .k-crop.an'), 15000);
  check(await einst.evaluate(() => !!document.querySelector('#einstellungen .k-buehne .k-crop.an')), 'Bearbeiten: Zuschnitt-Rahmen bereit');
  const verh = await einst.evaluate(() => { const r = document.querySelector('#einstellungen .k-buehne').getBoundingClientRect(); return r.width / r.height; });
  check(Math.abs(verh - 1.6) < 0.02, `Buehne im Seitenverhaeltnis des Fotos (${verh.toFixed(3)})`);
  // free crop: top-left handle to (25 %, 20 %)
  await ziehen('.k-buehne', [0.002, 0.003], [0.25, 0.2]);
  await bis(() => /left:\s*2[45]\./.test(document.querySelector('#einstellungen .k-crop').getAttribute('style')));
  let crop = await einst.evaluate(() => document.querySelector('#einstellungen .k-crop').getAttribute('style'));
  check(/left:\s*2[45]\.\d+%/.test(crop) && /top:\s*(19|20)\.\d+%/.test(crop), `freier Zuschnitt per Griff (${crop})`);
  // pen: two strokes, undo one; eraser removes one, undo restores
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Stift');
  await bis(() => document.querySelector('#einstellungen .k-farben'));
  await klick('[data-e="k_farbe"][data-v="#0a84ff"]');
  await ziehen('.k-buehne', [0.4, 0.5], [0.8, 0.5]);
  await ziehen('.k-buehne', [0.4, 0.7], [0.8, 0.9]);
  await bis(() => document.querySelectorAll('#einstellungen .k-striche path').length === 2);
  let striche = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-striche path')].map(p => p.getAttribute('stroke')));
  check(striche.length === 2 && striche.every(f => f === '#0a84ff'), `Stift zeichnet (${striche.length} Striche, ${striche[0]})`);
  await klick('[data-e="k_undo"]');
  await bis(() => document.querySelectorAll('#einstellungen .k-striche path').length === 1);
  check(await einst.evaluate(() => document.querySelectorAll('#einstellungen .k-striche path').length) === 1, 'Rueckgaengig entfernt den letzten Strich');
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Radierer');
  await ziehen('.k-buehne', [0.6, 0.4], [0.6, 0.6]);
  await bis(() => document.querySelectorAll('#einstellungen .k-striche path').length === 0);
  check(await einst.evaluate(() => document.querySelectorAll('#einstellungen .k-striche path').length) === 0, 'Radierer entfernt den Strich');
  await klick('[data-e="k_undo"]');
  await bis(() => document.querySelectorAll('#einstellungen .k-striche path').length === 1);
  check(await einst.evaluate(() => document.querySelectorAll('#einstellungen .k-striche path').length) === 1, 'Rueckgaengig holt ihn zurueck');
  // toolbar: four tools as one group; undo is an icon; reset is a real button
  const leiste = await einst.evaluate(() => ({ tools: [...document.querySelectorAll('#einstellungen .k-werkzeuge .k-seg .e-seg-btn')].map(b => b.textContent),
    undoIcon: !!document.querySelector('#einstellungen [data-e="k_undo"] svg') && document.querySelector('#einstellungen [data-e="k_undo"]').textContent.trim() === '',
    reset: !!document.querySelector('#einstellungen .k-werkzeuge [data-e="k_reset"]'),
    reihe: (() => { const e = document.querySelector('#einstellungen'); const y = s => e.querySelector(s).getBoundingClientRect().top;
      return y('.k-zurueck') < y('.k-buehne') && y('.k-buehne') < y('.k-werkzeuge') && y('.k-werkzeuge') < y('[data-e="k_kopie"]'); })() }));
  check(leiste.tools.join('|') === 'Zuschneiden|Stift|Radierer|Text' && leiste.undoIcon && leiste.reset && leiste.reihe,
    `Werkzeugleiste ${leiste.tools.join(' | ')}, Undo als Symbol, Reihenfolge Galerie > Vorschau > Werkzeuge > Speichern`);
  // free colour: native colour picker
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Stift');
  await bis(() => document.querySelector('#einstellungen input[data-kamfarbe]'));
  await einst.evaluate(() => { const i = document.querySelector('#einstellungen input[data-kamfarbe]'); i.value = '#12ab34'; i.dispatchEvent(new Event('input', { bubbles: true })); });
  await bis(() => document.querySelector('#einstellungen .k-eigen.aktiv'));
  await ziehen('.k-buehne', [0.4, 0.3], [0.7, 0.35]);
  await bis(() => document.querySelectorAll('#einstellungen .k-striche path').length === 2);
  striche = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-striche path')].map(p => p.getAttribute('stroke')));
  check(striche[1] === '#12ab34', `freie Farbwahl (Farbwaehler) fuer den Stift (${striche[1]})`);
  // text: click into the picture, type, size, move
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Text');
  const bb = await einst.locator('#einstellungen .k-buehne').boundingBox();
  await einst.mouse.click(bb.x + bb.width * 0.3, bb.y + bb.height * 0.6);
  await bis(() => document.activeElement && document.activeElement.matches('.k-textfeld'));
  check(await einst.evaluate(() => document.activeElement && document.activeElement.matches('.k-textfeld')), 'Klick ins Bild setzt Text, Eingabefeld hat den Fokus');
  await einst.keyboard.type('Hallo Noki');
  await bis(() => (document.querySelector('#einstellungen .k-striche text') || {}).textContent === 'Hallo Noki');
  await klick('.e-seg-btn[data-e="k_textgroesse"]', 'L');
  await bis(() => +(document.querySelector('#einstellungen .k-striche text') || { getAttribute: () => 0 }).getAttribute('font-size') > 70);
  let tx = await einst.evaluate(() => { const t = document.querySelector('#einstellungen .k-striche text'); return { t: t.textContent, f: t.getAttribute('fill'), g: +t.getAttribute('font-size'), x: +t.getAttribute('x'), y: +t.getAttribute('y'), w: t.getAttribute('font-weight') }; });
  check(tx.t === 'Hallo Noki' && tx.f === '#12ab34' && Math.abs(tx.g - 80) < 1 && tx.w === '700', `Text mit Inhalt, Farbe, Groesse, Staerke (${tx.t}, ${tx.f}, ${tx.g}px, ${tx.w})`);
  await ziehen('.k-buehne', [0.32, 0.57], [0.52, 0.77]);
  await bis(() => Math.abs(+document.querySelector('#einstellungen .k-striche text').getAttribute('x') - 0.5 * 800) < 8);
  tx = await einst.evaluate(() => { const t = document.querySelector('#einstellungen .k-striche text'); return { x: +t.getAttribute('x'), y: +t.getAttribute('y') }; });
  if (process.env.SHOTS) await einst.screenshot({ path: process.env.SHOTS + '/foto-editor.png' });
  check(Math.abs(tx.x - 400) < 8 && Math.abs(tx.y - 400) < 8, `Text verschiebbar (${tx.x}, ${tx.y})`);
  // undo step by step: move, size, typing, the text itself
  const anzahl = () => einst.evaluate(() => ({ p: document.querySelectorAll('#einstellungen .k-striche path').length, t: [...document.querySelectorAll('#einstellungen .k-striche text')].map(t => t.textContent + '@' + t.getAttribute('x') + '/' + t.getAttribute('font-size')) }));
  const schritte = [];
  let undoVorher = JSON.stringify(await anzahl());
  for (let i = 0; i < 4; i++) {
    await klick('[data-e="k_undo"]');
    await einst.waitForFunction(v => JSON.stringify({ p: document.querySelectorAll('#einstellungen .k-striche path').length, t: [...document.querySelectorAll('#einstellungen .k-striche text')].map(t => t.textContent + '@' + t.getAttribute('x') + '/' + t.getAttribute('font-size')) }) !== v, undoVorher, { timeout: 5000 }).catch(() => {});
    undoVorher = JSON.stringify(await anzahl()); schritte.push(undoVorher);
  }
  console.log('  Undo-Schritte:', schritte.join('  >  '));
  check(schritte.join('|') === ['{"p":2,"t":["Hallo Noki@240.0/80.0"]}', '{"p":2,"t":["Hallo Noki@240.0/48.0"]}', '{"p":2,"t":[]}', '{"p":1,"t":[]}'].join('|'),
    'Undo nimmt je Klick genau einen Schritt zurueck (Verschieben > Groesse > Text > gruener Strich)');
  // redo the text for the export check
  await einst.mouse.click(bb.x + bb.width * 0.5, bb.y + bb.height * 0.9);
  await bis(() => document.activeElement && document.activeElement.matches('.k-textfeld'));
  await einst.keyboard.type('Export');
  await bis(() => [...document.querySelectorAll('#einstellungen .k-striche text')].some(t => t.textContent === 'Export'));
  await main.evaluate(() => { const o = CanvasRenderingContext2D.prototype.fillText; window.__ft = [];
    CanvasRenderingContext2D.prototype.fillText = function (t, x, y) { window.__ft.push([t, x, y, this.fillStyle]); return o.apply(this, arguments); }; });
  // "Original ersetzen": the SAME button asks (like "Alle Dokumente entfernen"); no second click = nothing
  const knopfText = (e) => einst.evaluate(e => { const b = document.querySelector(`#einstellungen [data-e="${e}"]`); return b ? [...b.querySelectorAll('.e-stapel > span')].find(x => !x.classList.contains('weg')).textContent : null; }, e);
  await klick('[data-e="k_ersetzen"]');
  await bis(() => /Wirklich ersetzen/.test([...document.querySelectorAll('#einstellungen [data-e="k_ersetzen"] .e-stapel > span:not(.weg)')].map(x => x.textContent).join('')));
  check((await calls('kamera_bild_speichern')).length === 0 && await knopfText('k_ersetzen') === 'Wirklich ersetzen?' && await einst.evaluate(() => !document.querySelector('#einstellungen .k-frage')),
    '"Original ersetzen": derselbe Knopf wird zu "Wirklich ersetzen?", keine zweite Box');
  await bis(() => /Original ersetzen/.test([...document.querySelectorAll('#einstellungen [data-e="k_ersetzen"] .e-stapel > span:not(.weg)')].map(x => x.textContent).join('')), 6000);
  check((await calls('kamera_bild_speichern')).length === 0 && await knopfText('k_ersetzen') === 'Original ersetzen', 'ohne zweiten Klick nach 3 s zurueck, nichts ersetzt');
  // Save as copy (default)
  await klick('[data-e="k_kopie"]');
  await main.waitForFunction(() => window.__calls.some(c => c.name === 'kamera_bild_speichern'), null, { timeout: 15000 }).catch(() => {});
  const sp = (await calls('kamera_bild_speichern')).pop();
  const png = sp ? Buffer.from(sp.args.daten.split(',')[1], 'base64') : Buffer.alloc(0);
  const g = png.length > 24 ? pngGroesse(png) : {};
  check(sp && sp.args.ersetzen === false && sp.args.name === 'Noki-Screenshot-0.png', 'Kopie speichern = Standard (Original bleibt)');
  check(Math.abs(g.w - 600) <= 4 && Math.abs(g.h - 400) <= 4, `gespeichertes Bild = Zuschnitt in voller Aufloesung (${g.w}x${g.h})`);
  const ft = await main.evaluate(() => window.__ft);
  check(ft.length === 1 && ft[0][0] === 'Export' && Math.abs(ft[0][1] - (400 - 200)) < 3 && Math.abs(ft[0][2] - (450 - 100)) < 3, `Kopie enthaelt den Text, im Zuschnitt richtig versetzt (${JSON.stringify(ft[0])})`);
  await bis(() => /Als Kopie gespeichert/.test(document.querySelector('#einstellungen').textContent));
  check(await einst.evaluate(() => /Als Kopie gespeichert: Noki-Screenshot-0-bearbeitet\.png/.test(document.querySelector('#einstellungen').textContent)), 'Bestaetigung + die Kopie ist geoeffnet');
  // replace original deliberately
  await klick('[data-e="k_zu"]');
  await klick('.k-kachel[data-v="Noki-Screenshot-1.png"]');
  await klick('[data-e="k_bearbeiten"]');
  await bis(() => document.querySelector('#einstellungen .k-buehne'), 15000);
  await ziehen('.k-buehne', [0.002, 0.003], [0.3, 0.3]);
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Stift');
  await ziehen('.k-buehne', [0.4, 0.5], [0.8, 0.5]);
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Text');
  const bb2 = await einst.locator('#einstellungen .k-buehne').boundingBox();
  await einst.mouse.click(bb2.x + bb2.width * 0.5, bb2.y + bb2.height * 0.5);
  await bis(() => document.activeElement && document.activeElement.matches('.k-textfeld'));
  await einst.keyboard.type('weg');
  await bis(() => !!document.querySelector('#einstellungen .k-striche text'));
  await klick('[data-e="k_reset"]');
  await bis(() => !document.querySelector('#einstellungen .k-striche'));
  const nachReset = await einst.evaluate(() => ({ striche: !!document.querySelector('#einstellungen .k-striche'), crop: document.querySelector('#einstellungen .k-crop') && document.querySelector('#einstellungen .k-crop').getAttribute('style'), undo: document.querySelector('#einstellungen [data-e="k_undo"]').disabled }));
  check(!nachReset.striche && (!nachReset.crop || /left:0\.000%;top:0\.000%;width:100\.000%;height:100\.000%/.test(nachReset.crop)) && nachReset.undo, 'Zuruecksetzen verwirft Zuschnitt, Striche und Text auf einmal');
  await klick('[data-e="k_ersetzen"]');
  await klick('[data-e="k_ersetzen"]');
  await main.waitForFunction(() => window.__calls.filter(c => c.name === 'kamera_bild_speichern').length === 2, null, { timeout: 15000 }).catch(() => {});
  const sp2 = (await calls('kamera_bild_speichern')).pop();
  check(sp2 && sp2.args.ersetzen === true && sp2.args.name === 'Noki-Screenshot-1.png', 'Original ersetzen nur nach Bestaetigung');

  // ---- 3. Delete only after confirmation ---------------------------------
  await bis(() => document.querySelector('#einstellungen [data-e="k_loeschen"]'));
  const breiteL = await einst.evaluate(() => Math.round(document.querySelector('#einstellungen [data-e="k_loeschen"]').getBoundingClientRect().width));
  const knoepfeVorher = await einst.evaluate(() => document.querySelectorAll('#einstellungen button').length);
  await klick('[data-e="k_loeschen"]');
  await bis(() => /Wirklich löschen/.test([...document.querySelectorAll('#einstellungen [data-e="k_loeschen"] .e-stapel > span:not(.weg)')].map(x => x.textContent).join('')));
  const frage = await einst.evaluate(() => ({ breite: Math.round(document.querySelector('#einstellungen [data-e="k_loeschen"]').getBoundingClientRect().width), knoepfe: document.querySelectorAll('#einstellungen button').length, box: !!document.querySelector('#einstellungen .k-frage') }));
  check((await calls('kamera_loeschen')).length === 0 && await knopfText('k_loeschen') === 'Wirklich löschen?' && frage.breite === breiteL && frage.knoepfe === knoepfeVorher && !frage.box,
    `erster Klick: derselbe Knopf fragt "Wirklich löschen?" (kein zweiter Knopf, keine Box, Breite ${breiteL}->${frage.breite})`);
  await bis(() => /^Löschen$/.test([...document.querySelectorAll('#einstellungen [data-e="k_loeschen"] .e-stapel > span:not(.weg)')].map(x => x.textContent).join('')), 6000);
  check(await knopfText('k_loeschen') === 'Löschen' && (await calls('kamera_loeschen')).length === 0, 'nach 3 s ohne zweiten Klick wieder "Löschen"');
  await klick('[data-e="k_loeschen"]');
  await klick('[data-e="k_loeschen"]');
  await bis(() => !document.querySelector('#einstellungen .k-kachel[data-v="Noki-Screenshot-1.png"]') && document.querySelector('#einstellungen .k-kachel'));
  const lo = (await calls('kamera_loeschen')).pop();
  check(lo && lo.args.name === 'Noki-Screenshot-1.png' && await einst.evaluate(() => !document.querySelector('#einstellungen .k-kachel[data-v="Noki-Screenshot-1.png"]')), 'nach Bestaetigung in den Papierkorb, Kachel weg');

  // ---- 4. Video player + trim/crop export --------------------------------
  await klick('.k-kachel[data-v="Noki-Recording-1.mov"]');
  await bis(() => document.querySelector('#einstellungen video.k-gross'));
  const vid = await einst.evaluate(() => { const v = document.querySelector('#einstellungen video.k-gross'); return { controls: v.controls, src: v.getAttribute('src'), poster: v.getAttribute('poster') }; });
  check(vid.controls && /\/f\/Noki-Recording-1\.mov/.test(vid.src) && /\/t\//.test(vid.poster), 'Video oeffnet einen Player (Play/Pause, Zeitleiste, Dauer)');
  await einst.evaluate(() => document.querySelector('#einstellungen video.k-gross').play().catch(() => {}));
  await bis(() => document.querySelector('#einstellungen video.k-gross').currentTime > 0.3, 10000);
  check(await einst.evaluate(() => document.querySelector('#einstellungen video.k-gross').currentTime > 0.3), 'Video spielt im Einstellungsfenster');
  await einst.evaluate(() => document.querySelector('#einstellungen video.k-gross').pause());
  await klick('[data-e="k_bearbeiten"]');
  await bis(() => document.querySelector('#einstellungen .k-trim-griff'), 15000);
  const vEd = () => einst.evaluate(() => { const v = document.querySelector('#einstellungen video[data-medium]'); const k = document.querySelector('#einstellungen .k-kopf-linie');
    return { t: v.currentTime, paused: v.paused, kopf: parseFloat(k.style.left), pos: document.querySelector('#einstellungen .k-pos').textContent,
      knopf: document.querySelector('#einstellungen [data-e="k_play"]').getAttribute('aria-label'), zweite: document.querySelectorAll('#einstellungen video').length,
      reset: (() => { const r = document.querySelector('#einstellungen [data-e="k_reset"]'); return r && r.textContent.trim() === 'Zurücksetzen' && r.offsetHeight >= 26; })() }; });
  let ve = await vEd();
  check(ve.zweite === 1 && ve.knopf === 'Abspielen' && ve.reset, 'Editor: ein Video, Play-Knopf, sichtbarer "Zurücksetzen"-Knopf');
  const vTools = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-werkzeuge .k-seg .e-seg-btn')].map(b => b.textContent + (b.classList.contains('aktiv') ? '*' : '')).join(' | '));
  check(vTools === 'Zuschneiden | Schneiden* | Stift | Radierer | Text', `Video-Werkzeugleiste ${vTools}`);
  await klick('[data-e="k_play"]');
  await bis(() => !document.querySelector('#einstellungen video[data-medium]').paused);
  await bis(() => parseFloat(document.querySelector('#einstellungen .k-kopf-linie').style.left) > 10 && document.querySelector('#einstellungen video[data-medium]').currentTime > 0.4, 8000);
  ve = await vEd();
  check(!ve.paused && ve.t > 0.4 && ve.kopf > 10 && ve.knopf === 'Pause', `Play waehrend der Bearbeitung: Abspielkopf laeuft (${ve.t.toFixed(2)} s, ${ve.kopf.toFixed(1)} %, ${ve.pos})`);
  await klick('[data-e="k_play"]');
  await bis(() => document.querySelector('#einstellungen video[data-medium]').paused);
  ve = await vEd();
  const angehalten = ve.t;
  await einst.waitForTimeout(500);
  ve = await vEd();
  check(ve.paused && Math.abs(ve.t - angehalten) < 0.01 && ve.knopf === 'Abspielen', `Pause haelt an (${ve.t.toFixed(2)} s)`);
  // trim start: the picture jumps to the cut
  await ziehen('.k-trim-spur', [0, 0.5], [0.2, 0.5]);
  await bis(() => Math.abs(document.querySelector('#einstellungen video[data-medium]').currentTime - 0.6) < 0.1);
  ve = await vEd();
  check(Math.abs(ve.t - 0.6) < 0.1, `Trim-Start setzt die Vorschau auf den Schnitt (${ve.t.toFixed(2)} s)`);
  await klick('[data-e="k_play"]');
  await bis(() => document.querySelector('#einstellungen video[data-medium]').currentTime > 0.8, 8000);
  ve = await vEd();
  check(!ve.paused && ve.t > 0.8, `weiter abspielen ab dem Start (${ve.t.toFixed(2)} s)`);
  await klick('[data-e="k_play"]');
  await bis(() => document.querySelector('#einstellungen video[data-medium]').paused);
  await ziehen('.k-trim-spur', [1, 0.5], [0.8, 0.5]);
  // seek: click into the timeline
  const spur = await einst.locator('#einstellungen .k-trim-spur').boundingBox();
  await einst.mouse.click(spur.x + spur.width * 0.5, spur.y + spur.height * 0.5);
  await bis(() => Math.abs(document.querySelector('#einstellungen video[data-medium]').currentTime - 1.5) < 0.1);
  ve = await vEd();
  check(Math.abs(ve.t - 1.5) < 0.1 && Math.abs(ve.kopf - 50) < 3, `Seek per Zeitleiste (${ve.t.toFixed(2)} s, Kopf ${ve.kopf.toFixed(1)} %)`);
  // playing stops at the trim end
  await klick('[data-e="k_play"]');
  await bis(() => document.querySelector('#einstellungen video[data-medium]').paused && document.querySelector('#einstellungen video[data-medium]').currentTime > 2, 8000);
  ve = await vEd();
  if (process.env.SHOTS) await einst.screenshot({ path: process.env.SHOTS + '/video-editor.png' });
  check(ve.paused && Math.abs(ve.t - 2.4) < 0.15, `Wiedergabe endet am Trim-Ende (${ve.t.toFixed(2)} s)`);
  await bis(() => /0:01 – 0:02 · 1,8 s/.test((document.querySelector('#einstellungen .k-trim-zeit') || {}).textContent || ''));
  const tz = await einst.evaluate(() => document.querySelector('#einstellungen .k-trim-zeit').textContent);
  await bis(() => !document.querySelector('#einstellungen [data-e="k_undo"]').disabled);
  check(await einst.evaluate(() => !document.querySelector('#einstellungen [data-e="k_undo"]').disabled), 'Undo nach Trim verfuegbar');
  check(/0:01 – 0:02 · 1,8 s/.test(tz), `Schneiden mit Start-/End-Griff (${tz})`);
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Zuschneiden');
  await bis(() => document.querySelector('#einstellungen .k-crop.an'));
  await ziehen('.k-buehne', [0.998, 0.997], [0.5, 0.5]);
  // pen on the video at the playhead (1.5 s), same palette as the photo editor
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Stift');
  await bis(() => document.querySelector('#einstellungen .k-kontext input[data-kamfarbe]'));
  const palette = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .k-kontext .k-farbe')].map(b => b.getAttribute('data-v') || 'eigen').join(','));
  check(/^#ff453a,#ff9f0a,#ffd60a,#30d158,#0a84ff,#bf5af2,#ffffff,#1c1c1e,eigen$/.test(palette) && await einst.evaluate(() => document.querySelectorAll('#einstellungen .k-dicke').length === 3),
    'gleiche Farbpalette, Farbwaehler und Staerken wie im Fotoeditor');
  check(await einst.evaluate(() => document.querySelector('#einstellungen .k-eigen.aktiv')) !== null, `Farbe aus dem Fotoeditor uebernommen (#12ab34)`);
  await klick('[data-e="k_farbe"][data-v="#ff9f0a"]');
  const posStift = await einst.evaluate(() => document.querySelector('#einstellungen video[data-medium]').currentTime);
  await ziehen('.k-buehne', [0.1, 0.2], [0.4, 0.25]);
  await bis(() => document.querySelectorAll('#einstellungen .k-striche path').length === 1);
  check(await einst.evaluate(() => (document.querySelector('#einstellungen .k-striche path') || {}).getAttribute && document.querySelector('#einstellungen .k-striche path').getAttribute('stroke')) === '#ff9f0a', `Stift auf dem Video (ab ${posStift.toFixed(2)} s)`);
  // text
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Text');
  const vb = await einst.locator('#einstellungen .k-buehne').boundingBox();
  await einst.mouse.click(vb.x + vb.width * 0.1, vb.y + vb.height * 0.4);
  await bis(() => document.activeElement && document.activeElement.matches('.k-textfeld'));
  await einst.keyboard.type('Noki Video');
  await bis(() => (document.querySelector('#einstellungen .k-striche text') || {}).textContent === 'Noki Video');
  check(await einst.evaluate(() => (document.querySelector('#einstellungen .k-striche text') || {}).textContent) === 'Noki Video', 'Text auf dem Video');
  // time rule: before the insert position the overlays are hidden
  const spurV = await einst.locator('#einstellungen .k-trim-spur').boundingBox();
  await einst.mouse.click(spurV.x + 2, spurV.y + spurV.height / 2);
  await bis(() => !document.querySelector('#einstellungen .k-striche path') && !document.querySelector('#einstellungen .k-striche text'));
  check(await einst.evaluate(() => !document.querySelector('#einstellungen .k-striche path') && !document.querySelector('#einstellungen .k-striche text')), 'vor der Einfuegeposition unsichtbar (Overlay gilt ab seinem Zeitpunkt)');
  await einst.mouse.click(spurV.x + spurV.width * 0.97, spurV.y + spurV.height / 2);
  await bis(() => !!document.querySelector('#einstellungen .k-striche path') && !!document.querySelector('#einstellungen .k-striche text'));
  check(await einst.evaluate(() => !!document.querySelector('#einstellungen .k-striche path') && !!document.querySelector('#einstellungen .k-striche text')), 'ab der Einfuegeposition sichtbar');
  // eraser + undo
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Radierer');
  await ziehen('.k-buehne', [0.25, 0.12], [0.25, 0.26]);
  await bis(() => !document.querySelector('#einstellungen .k-striche path'));
  check(await einst.evaluate(() => !document.querySelector('#einstellungen .k-striche path') && !!document.querySelector('#einstellungen .k-striche text')), 'Radierer entfernt den Strich (Text bleibt)');
  await klick('[data-e="k_undo"]');
  await bis(() => !!document.querySelector('#einstellungen .k-striche path'));
  check(await einst.evaluate(() => !!document.querySelector('#einstellungen .k-striche path')), 'Undo holt den Strich zurueck (gleicher Verlauf)');
  // Zuruecksetzen: trim + crop back to the original state
  await klick('[data-e="k_reset"]');
  await bis(() => /0:00 – 0:03 · 3,0 s/.test(document.querySelector('#einstellungen .k-trim-zeit').textContent));
  const rz = await einst.evaluate(() => ({ t: document.querySelector('#einstellungen .k-trim-zeit').textContent, crop: document.querySelector('#einstellungen .k-crop'),
    striche: !!document.querySelector('#einstellungen .k-striche'), undo: document.querySelector('#einstellungen [data-e="k_undo"]').disabled }));
  check(/0:00 – 0:03 · 3,0 s/.test(rz.t) && !rz.striche && rz.undo, `Zuruecksetzen: volle Dauer, kein Zuschnitt, keine Zeichnung/Text (${rz.t})`);
  // again for the export
  await ziehen('.k-trim-spur', [0, 0.5], [0.2, 0.5]);
  await ziehen('.k-trim-spur', [1, 0.5], [0.8, 0.5]);
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Zuschneiden');
  await bis(() => document.querySelector('#einstellungen .k-crop.an'));
  await ziehen('.k-buehne', [0.998, 0.997], [0.5, 0.5]);
  // overlays for the export: a stroke from 1.5 s, a text from 0 s
  await einst.mouse.click(spurV.x + spurV.width * 0.5, spurV.y + spurV.height / 2);
  await bis(() => Math.abs(document.querySelector('#einstellungen video[data-medium]').currentTime - 1.5) < 0.1);
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Stift');
  await ziehen('.k-buehne', [0.1, 0.1], [0.4, 0.4]);
  await einst.mouse.click(spurV.x + 1, spurV.y + spurV.height / 2);
  await bis(() => document.querySelector('#einstellungen video[data-medium]').currentTime < 0.1);
  await klick('.e-seg-btn[data-e="k_werkzeug"]', 'Text');
  await einst.mouse.click(vb.x + vb.width * 0.05, vb.y + vb.height * 0.3);
  await bis(() => document.activeElement && document.activeElement.matches('.k-textfeld'));
  await einst.keyboard.type('Ab Start');
  await bis(() => (document.querySelector('#einstellungen .k-striche text') || {}).textContent === 'Ab Start');
  await main.evaluate(() => { window.__ft = []; });
  await klick('[data-e="k_export"]');
  await bis(() => /Gespeichert als Noki-Recording-1-geschnitten\.mov/.test(document.querySelector('#einstellungen').textContent), 10000);
  const ex = (await calls('kamera_video_export')).pop();
  check(ex && Math.abs(ex.args.start - 0.6) < 0.08 && Math.abs(ex.args.ende - 2.4) < 0.08, `Export mit Zeitbereich (${ex && ex.args.start.toFixed(2)}–${ex && ex.args.ende.toFixed(2)} s)`);
  check(ex && ex.args.x === 0 && ex.args.y === 0 && Math.abs(ex.args.w - 0.5) < 0.03 && Math.abs(ex.args.h - 0.5) < 0.03, `Export mit raeumlichem Zuschnitt (${ex && [ex.args.x, ex.args.y, ex.args.w.toFixed(2), ex.args.h.toFixed(2)]})`);
  check(await einst.evaluate(() => /das Original bleibt unverändert/.test(document.querySelector('#einstellungen').textContent)), 'Export fertig: neue Datei, Original unveraendert');
  // the overlays really go into the export: PNG in crop size, with pixels, per insert time
  const ov = await main.evaluate(async (o) => Promise.all((o || []).map(e => new Promise(r => { const i = new Image(); i.onload = () => {
    const c = document.createElement('canvas'); c.width = i.width; c.height = i.height; const g = c.getContext('2d'); g.drawImage(i, 0, 0);
    const d = g.getImageData(0, 0, i.width, i.height).data; let n = 0; for (let k = 3; k < d.length; k += 4) if (d[k] > 0) n++;
    r({ t0: e.t0, w: i.width, h: i.height, pixel: n }); }; i.src = e.daten; }))), ex && ex.args.overlays);
  console.log('  Overlays im Export:', JSON.stringify(ov));
  check(ov.length === 2 && ov[0].t0 < 0.05 && Math.abs(ov[1].t0 - 1.5) < 0.1 && ov.every(o => o.w === 160 && o.h === 90 && o.pixel > 50),
    'Export enthaelt Zeichnung (ab 1,5 s) und Text (ab 0 s) als Ebenen in Zuschnittgroesse');
  check((await main.evaluate(() => window.__ft)).some(f => f[0] === 'Ab Start'), 'Text wird fuer den Export gezeichnet');
  check((await calls('kamera_medien')).length >= 4, 'Galerie danach aktualisiert');
  // progress + cancel
  await main.evaluate(() => { window.__exportHalten = true; });
  await klick('[data-e="k_export"]');
  await bis(() => /Exportiert … 40 %/.test(document.querySelector('#einstellungen').textContent));
  const balken = await einst.evaluate(() => (document.querySelector('#einstellungen .k-fortschritt i') || {}).style.width);
  check(balken === '40%', `Fortschritt sichtbar (${balken})`);
  await klick('[data-e="k_export_stopp"]');
  await bis(() => /Export abgebrochen/.test(document.querySelector('#einstellungen').textContent));
  check((await calls('kamera_video_abbrechen')).length === 1 && await einst.evaluate(() => /Export abgebrochen/.test(document.querySelector('#einstellungen').textContent)), 'Export abbrechbar');
  await klick('[data-e="k_abbrechen"]');
  await klick('[data-e="k_zu"]');

  // ---- 5. New capture (Shortcut 1/2) refreshes an open gallery -------------
  const vorher = (await calls('kamera_medien')).length;
  await main.evaluate(() => { window.__kam.liste.unshift({ name: 'Noki-Screenshot-neu.png', art: 'foto', groesse: 1, zeit: Date.now(), w: 10, h: 10, dauer: null });
    window.NokiKameraAktion.nach('foto', { pfad: '/Users/x/Desktop/Noki Kamera/Noki-Screenshot-neu.png' }, true); });
  await bis(() => (document.querySelector('#einstellungen .k-kachel') || {}).getAttribute && document.querySelector('#einstellungen .k-kachel').getAttribute('data-v') === 'Noki-Screenshot-neu.png');
  check((await calls('kamera_medien')).length === vorher + 1 && await einst.evaluate(() => document.querySelector('#einstellungen .k-kachel').getAttribute('data-v')) === 'Noki-Screenshot-neu.png', 'neue Aufnahme erscheint sofort vorne (ein Abruf, kein Polling)');
  await main.waitForTimeout(3000);
  check((await calls('kamera_medien')).length === vorher + 1, 'kein weiteres Nachladen im Leerlauf');

  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
