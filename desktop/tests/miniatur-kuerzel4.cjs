// Shortcut 4 / Miniatur: the explicit toggle always wins over hover. While
// the native helper reports the Miniatur as LARGE (pointer inside), 4 still
// hides it (empty frame to the helper) and the next 4 shows it again -
// repeatable. The helper side (schirm/main.swift "rahmen") is native and not
// testable here; this checks the page sends the hide every time.
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

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--enable-webgl', '--ignore-gpu-blocklist'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  page.on('pageerror', e => console.log('pageerror:', e.message));
  await page.addInitScript(() => {
    window.__calls = []; window.__ev = {};
    const invoke = async (name, args) => {
      window.__calls.push({ name, args });
      if (name === 'fokus_sitzung_status') return { aktiv: false };
      if (name === 'intelligence_settings') return { settings: { level: 'off', ask: true, web: false }, installed: true, loaded: false };
      return null;
    };
    window.__emit = (n, p) => (window.__ev[n] || []).forEach(f => f({ payload: p }));
    window.__TAURI__ = { core: { invoke }, event: { listen: async (n, f) => { (window.__ev[n] = window.__ev[n] || []).push(f); return () => {}; }, emit: async () => {}, emitTo: async () => {} },
      window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
    window.__TAURI_INTERNALS__ = { invoke };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/index.html`, { timeout: 180000 });
  await page.waitForFunction(() => window.NokiVorschau, null, { timeout: 120000 });
  await page.waitForTimeout(3000);
  const zonen = () => page.evaluate(() => window.__calls.filter(c => c.name === 'noki_vorschau_zone').map(c => c.args.w));
  const an = () => page.evaluate(() => window.NokiVorschau.diag().an);
  const kuerzel4 = () => page.evaluate(() => window.__emit('noki://vorschau_toggle', {}));
  // make sure it is visible first
  if (!(await an())) { await kuerzel4(); await page.waitForTimeout(300); }
  check(await an(), 'Shortcut 4 oeffnet die Miniatur');
  let fehlZyklen = 0;
  for (let i = 0; i < 6; i++) {
    // pointer inside -> helper reports LARGE (Grossansicht)
    await page.evaluate(() => window.__emit('noki://vorschau_hover', { modus: 'gross' }));
    await page.waitForTimeout(150);
    const vorher = (await zonen()).length;
    await kuerzel4();
    await page.waitForTimeout(250);
    const z = await zonen(), zu = !(await an()) && z.length > vorher && z[z.length - 1] === 0;
    await kuerzel4();
    await page.waitForTimeout(250);
    const wieder = await an();
    if (!zu || !wieder) { fehlZyklen++; console.log('  Zyklus', i, { zu, wieder, z: z.slice(vorher) }); }
  }
  check(fehlZyklen === 0, `6x bei Hover/Grossansicht: 4 schliesst sofort (leerer Rahmen an den Helfer), 4 oeffnet wieder (${fehlZyklen} Fehler)`);
  // twice hidden in a row (e.g. state drift) - every hide is sent again
  await kuerzel4(); await page.waitForTimeout(200);
  await page.evaluate(() => window.NokiVorschau.verbergen(true)); await page.waitForTimeout(200);
  check(!(await an()), 'bleibt geschlossen, kein haengender Zustand');
  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
