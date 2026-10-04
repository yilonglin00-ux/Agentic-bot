// Timer: the "Noki winkt" switch is gone (waving at the end is fixed); an
// old stored timer.gruss=false is ignored and dropped on the next save; the
// other timer settings stay. Native side mocked. (The wave itself at timer
// end: tests/timer-ende.cjs.)
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
      if (name === 'werk_daten_lesen' && args && args.schluessel === 'einstellungen') return alt;
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
  const { page, einst } = await lauf(browser, port, { freeze: 'anomalie', timer: { dauer: 40, blase: false, gruss: false } }, true);
  const w = await page.evaluate(() => window.NokiEinstellungen.zustand().werte.timer);
  check(!('gruss' in w) && w.dauer === 40 && w.blase === false, `alter Wert gruss=false ignoriert, Rest uebernommen (${JSON.stringify(w)})`);
  await page.evaluate(() => window.NokiEinstellungen.auf('timer'));
  await einst.waitForFunction(() => /Timer in Nokis Händen zeigen/.test(document.querySelector('#einstellungen').textContent), null, { timeout: 15000 }).catch(() => {});
  const txt = await einst.evaluate(() => document.querySelector('#einstellungen').textContent);
  check(/Timer in Nokis Händen zeigen/.test(txt), 'Timer-Einstellungen sonst unveraendert ("Timer in Nokis Händen zeigen")');
  check(!/winkt/i.test(txt), 'kein Winken-Schalter mehr');
  // a save writes no gruss key any more
  await einst.click('#einstellungen .e-check');
  await page.waitForFunction(() => window.__calls.some(c => c.name === 'werk_daten_schreiben'), null, { timeout: 8000 }).catch(() => {});
  const s = await page.evaluate(() => window.__calls.filter(c => c.name === 'werk_daten_schreiben' && c.args.schluessel === 'einstellungen').pop());
  check(s && s.args.wert.timer && !('gruss' in s.args.wert.timer) && s.args.wert.timer.blase === true, `gesichert ohne gruss (${s && JSON.stringify(s.args.wert.timer)})`);
  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
