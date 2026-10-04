// Freeze styles (only Klassisch + Anomalie; persisted Tore/Zeitrisse ->
// Klassisch, saved once) and the shortcut overview (Noki Talk instead of
// the old "Noki hoert zu" card). Native side mocked.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const root = path.resolve(__dirname, '..');

let fehler = 0;
const check = (ok, msg) => { console.log((ok ? 'ok   ' : 'FAIL ') + msg); if (!ok) fehler++; };

const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + new URL(req.url, 'http://noki').pathname);
  if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
  fs.readFile(file, (error, data) => {
    if (error) return res.writeHead(404).end();
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
    res.end(data);
  });
});

async function lauf(browser, port, gespeichert, mitEinst) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await ctx.newPage();
  const einst = mitEinst ? await ctx.newPage() : null;
  if (einst) {
    await page.exposeBinding('__nachEinst', (_, n, p) => einst.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [n, p]).catch(() => {}));
    await einst.exposeBinding('__nachMain', (_, n, p) => page.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [n, p]).catch(() => {}));
  }
  page.on('pageerror', e => { console.log('pageerror:', e.message); fehler++; });
  const mock = ([alt, rolle]) => {
    window.__calls = []; window.__ev = {};
    const invoke = async (name, args) => {
      window.__calls.push({ name, args });
      if (name === 'werk_daten_lesen' && args && args.schluessel === 'einstellungen') return { freeze: alt };
      if (name === 'fokus_sitzung_status') return { aktiv: false };
      if (name === 'intelligence_settings') return { settings: { level: 'off', ask: true, web: false }, installed: true, loaded: false };
      return null;
    };
    window.__emit = (n, p) => (window.__ev[n] || []).forEach(f => f({ payload: p }));
    const emitTo = async (ziel, n, p) => {
      if (rolle === 'main' && ziel === 'einstellungen' && window.__nachEinst) return window.__nachEinst(n, p);
      if (rolle === 'einst' && ziel === 'main') return window.__nachMain(n, p);
    };
    window.__TAURI__ = { core: { invoke }, event: { listen: async (n, f) => { (window.__ev[n] = window.__ev[n] || []).push(f); return () => {}; }, emit: async () => {}, emitTo },
      window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
    window.__TAURI_INTERNALS__ = { invoke };
  };
  await page.addInitScript(mock, [gespeichert, 'main']);
  if (einst) await einst.addInitScript(mock, [gespeichert, 'einst']);
  await page.goto(`http://127.0.0.1:${port}/index.html`, { timeout: 180000 });
  await page.waitForFunction(() => window.NokiEinstellungen && window.NokiFreeze, null, { timeout: 120000 });
  await page.waitForFunction(() => window.__calls.some(c => c.name === 'werk_daten_lesen' && c.args && c.args.schluessel === 'einstellungen'), null, { timeout: 30000 });
  await page.waitForTimeout(500);
  if (einst) { await einst.goto(`http://127.0.0.1:${port}/settings.html`); await einst.waitForTimeout(300); }
  return { page, einst, ctx };
}

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const port = server.address().port;
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });

  for (const alt of ['tore', 'zeitrisse']) {
    const { page, ctx } = await lauf(browser, port, alt);
    const z = await page.evaluate(() => ({ wert: window.NokiEinstellungen.zustand().werte.freeze, stil: window.NokiFreeze.stil(),
      gesichert: window.__calls.filter(c => c.name === 'werk_daten_schreiben' && c.args.schluessel === 'einstellungen').map(c => c.args.wert.freeze) }));
    check(z.wert === 'klassisch' && z.stil === 'klassisch', `gespeichert "${alt}" -> Klassisch (${z.wert}/${z.stil})`);
    check(z.gesichert.length === 1 && z.gesichert[0] === 'klassisch', `Migration wird einmal gesichert (${z.gesichert.join(',')})`);
    await ctx.close();
  }
  const { page, einst } = await lauf(browser, port, 'anomalie', true);
  let z = await page.evaluate(() => ({ stil: window.NokiFreeze.stil(), n: window.__calls.filter(c => c.name === 'werk_daten_schreiben').length }));
  check(z.stil === 'anomalie' && z.n === 0, 'Anomalie bleibt, nichts wird neu geschrieben');
  check(await page.evaluate(() => window.NokiFreeze.stil('tore')) === 'klassisch', 'stil("tore") -> Klassisch');
  check(await page.evaluate(() => window.NokiFreeze.stil('anomalie')) === 'anomalie', 'stil("anomalie") bleibt waehlbar');

  await page.evaluate(() => window.NokiEinstellungen.auf('freeze'));
  await einst.waitForSelector('#einstellungen .e-frz-karte', { timeout: 20000 });
  const karten = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .e-frz-karte')].map(k => k.getAttribute('data-v')));
  check(karten.join(',') === 'klassisch,anomalie', `Freeze-Einstellung: nur Klassisch + Anomalie (${karten.join(',')})`);
  const txt = await einst.evaluate(() => document.querySelector('#einstellungen').textContent);
  check(!/Tore|Zeitrisse/.test(txt), 'keine Tore/Zeitrisse in den Einstellungen');
  await einst.click('#einstellungen .e-frz-karte[data-v="klassisch"]');
  await page.waitForFunction(() => window.NokiFreeze.stil() === 'klassisch', null, { timeout: 8000 }).catch(() => {});
  z = await page.evaluate(() => ({ stil: window.NokiFreeze.stil(), w: window.__calls.filter(c => c.name === 'werk_daten_schreiben').pop() }));
  check(z.stil === 'klassisch' && z.w && z.w.args.wert.freeze === 'klassisch', 'Klassisch waehlen speichert');

  // Shortcut overview (Settings > Shortcuts uses the same cards).
  await page.evaluate(() => window.NokiEinstellungen.auf('shortcuts'));
  await einst.waitForFunction(() => document.querySelectorAll('#einstellungen .n-sc-gruppe').length > 3, null, { timeout: 8000 }).catch(() => {});
  const sc = await einst.evaluate(() => [...document.querySelectorAll('.n-sc-gruppe')].map(g => ({ h: g.querySelector('h3').textContent, k: [...g.querySelectorAll('.n-shortcut')].map(k => k.textContent.replace(/\s+/g, ' ').trim()) })));
  const talk = sc.find(g => g.h === 'Noki Talk');
  check(!!talk && talk.k.length === 3, `Uebersicht: Gruppe "Noki Talk" mit 3 Karten (${talk && talk.k.join(' | ')})`);
  if (talk) {
    check(/^⌥ ?⌥ ?Aufnahme starten/.test(talk.k[0]), 'Option ×2 = Start');
    check(/^⌥ ?Aufnahme beenden/.test(talk.k[1]), 'Option ×1 = Stopp');
    check(/^Leertaste ?Aufnahme beenden/.test(talk.k[2]), 'Leertaste = Stopp');
  }
  const alles = JSON.stringify(sc);
  check(!/Noki hört zu|Noki H2/.test(alles), 'kein "Noki hört zu"/"Noki H2"-Eintrag mehr');
  check(sc.length > 3, `uebrige Gruppen unveraendert vorhanden (${sc.map(g => g.h).join(', ')})`);

  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
