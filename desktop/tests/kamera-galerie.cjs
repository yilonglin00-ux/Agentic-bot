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
    if (art === 'f' && /\.mov$/.test(name)) return route.fulfill({ status: 200, headers: { ...h, 'Content-Type': 'video/webm' }, body: CLIP });
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
      .concat(Array.from({ length: 69 }, (_, i) => ({ name: `Noki-Screenshot-${i}.png`, art: 'foto', groesse: 300000, zeit: jetzt - 60000 * (i + 1), w: 800, h: 500, dauer: null }))) };
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
  // "Original ersetzen" needs a confirmation; cancel = nothing happens
  await klick('[data-e="k_ersetzen"]');
  await bis(() => document.querySelector('#einstellungen [data-e="k_ersetzen_ja"]'));
  check((await calls('kamera_bild_speichern')).length === 0 && await einst.evaluate(() => !!document.querySelector('#einstellungen [data-e="k_ersetzen_ja"]')), '"Original ersetzen" fragt erst nach');
  await klick('[data-e="k_ersetzen_nein"]');
  check((await calls('kamera_bild_speichern')).length === 0, 'Abbrechen ersetzt nichts');
  // Save as copy (default)
  await klick('[data-e="k_kopie"]');
  await main.waitForFunction(() => window.__calls.some(c => c.name === 'kamera_bild_speichern'), null, { timeout: 15000 }).catch(() => {});
  const sp = (await calls('kamera_bild_speichern')).pop();
  const png = sp ? Buffer.from(sp.args.daten.split(',')[1], 'base64') : Buffer.alloc(0);
  const g = png.length > 24 ? pngGroesse(png) : {};
  check(sp && sp.args.ersetzen === false && sp.args.name === 'Noki-Screenshot-0.png', 'Kopie speichern = Standard (Original bleibt)');
  check(Math.abs(g.w - 600) <= 4 && Math.abs(g.h - 400) <= 4, `gespeichertes Bild = Zuschnitt in voller Aufloesung (${g.w}x${g.h})`);
  await bis(() => /Als Kopie gespeichert/.test(document.querySelector('#einstellungen').textContent));
  check(await einst.evaluate(() => /Als Kopie gespeichert: Noki-Screenshot-0-bearbeitet\.png/.test(document.querySelector('#einstellungen').textContent)), 'Bestaetigung + die Kopie ist geoeffnet');
  // replace original deliberately
  await klick('[data-e="k_zu"]');
  await klick('.k-kachel[data-v="Noki-Screenshot-1.png"]');
  await klick('[data-e="k_bearbeiten"]');
  await bis(() => document.querySelector('#einstellungen .k-buehne'), 15000);
  await klick('[data-e="k_ersetzen"]');
  await klick('[data-e="k_ersetzen_ja"]');
  await main.waitForFunction(() => window.__calls.filter(c => c.name === 'kamera_bild_speichern').length === 2, null, { timeout: 15000 }).catch(() => {});
  const sp2 = (await calls('kamera_bild_speichern')).pop();
  check(sp2 && sp2.args.ersetzen === true && sp2.args.name === 'Noki-Screenshot-1.png', 'Original ersetzen nur nach Bestaetigung');

  // ---- 3. Delete only after confirmation ---------------------------------
  await bis(() => document.querySelector('#einstellungen [data-e="k_loeschen"]'));
  await klick('[data-e="k_loeschen"]');
  check((await calls('kamera_loeschen')).length === 0, 'Loeschen fragt erst nach');
  await klick('[data-e="k_loeschen_nein"]');
  await klick('[data-e="k_loeschen"]');
  await klick('[data-e="k_loeschen_ja"]');
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
  await klick('[data-e="k_bearbeiten"]');
  await bis(() => document.querySelector('#einstellungen .k-trim-griff'), 15000);
  await ziehen('.k-trim-spur', [0, 0.5], [0.2, 0.5]);
  await ziehen('.k-trim-spur', [1, 0.5], [0.8, 0.5]);
  await bis(() => /0:01 – 0:02 · 1,8 s/.test((document.querySelector('#einstellungen .k-trim-zeit') || {}).textContent || ''));
  const tz = await einst.evaluate(() => document.querySelector('#einstellungen .k-trim-zeit').textContent);
  check(/0:01 – 0:02 · 1,8 s/.test(tz), `Schneiden mit Start-/End-Griff (${tz})`);
  await klick('.e-seg-btn[data-e="k_zuschnitt"]', 'Bildausschnitt');
  await bis(() => document.querySelector('#einstellungen .k-crop.an'));
  await ziehen('.k-buehne', [0.998, 0.997], [0.5, 0.5]);
  await klick('[data-e="k_export"]');
  await bis(() => /Gespeichert als Noki-Recording-1-geschnitten\.mov/.test(document.querySelector('#einstellungen').textContent), 10000);
  const ex = (await calls('kamera_video_export')).pop();
  check(ex && Math.abs(ex.args.start - 0.6) < 0.08 && Math.abs(ex.args.ende - 2.4) < 0.08, `Export mit Zeitbereich (${ex && ex.args.start.toFixed(2)}–${ex && ex.args.ende.toFixed(2)} s)`);
  check(ex && ex.args.x === 0 && ex.args.y === 0 && Math.abs(ex.args.w - 0.5) < 0.03 && Math.abs(ex.args.h - 0.5) < 0.03, `Export mit raeumlichem Zuschnitt (${ex && [ex.args.x, ex.args.y, ex.args.w.toFixed(2), ex.args.h.toFixed(2)]})`);
  check(await einst.evaluate(() => /das Original bleibt unverändert/.test(document.querySelector('#einstellungen').textContent)), 'Export fertig: neue Datei, Original unveraendert');
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
