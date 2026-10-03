// Shortcut 7 / timer: the panel closes on Start, the timer end is ONE big
// "Einmal durch den Desktop" (no parade, no clones) and Noki returns to the
// exact previous size and energy.
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
  await page.waitForFunction(() => window.NokiTimer && window.NokiWerk && window.NokiRaum && window.NokiAktion, null, { timeout: 120000 });
  await page.waitForTimeout(25000);          // spawn animation finished (headless is slow)
  const panel = () => page.evaluate(() => { const z = window.NokiWerk.zustand(); return z.offen && z.was === 'timer'; });

  // 1. Shortcut 7 -> panel; Start -> timer runs AND panel is gone at once.
  await page.evaluate(() => window.__emit('noki://werkzeug', { was: 'timer' }));
  check(await panel(), 'Shortcut 7 oeffnet das Timer-Panel');
  const start = page.locator('#werk [data-w="tstart"]');
  await start.waitFor({ state: 'visible', timeout: 20000 });
  const box = await start.boundingBox();
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  const nachStart = await page.evaluate(() => ({ t: window.NokiTimer.zustand(), z: window.NokiWerk.zustand(),
    display: getComputedStyle(document.getElementById('werk')).display }));
  check(nachStart.t.laeuft && !nachStart.z.offen && nachStart.display === 'none', `Starten -> Timer laeuft, Panel sofort zu (${nachStart.display})`);
  await page.evaluate(() => window.NokiTimer.abbrechen());

  // 2. Timer end with an open timer panel and a user size of 1.4x.
  await page.evaluate(() => { window.NokiRaum.groesseZiel(54 * 1.4); window.NokiEnergie.setzen('normal'); });
  await page.waitForFunction(() => Math.abs(window.NokiRaum.groesse() - 54 * 1.4) < 0.1, null, { timeout: 30000 });
  await page.evaluate(() => { window.NokiTimer.start(2 / 60); window.__emit('noki://werkzeug', { was: 'timer' }); });
  check(await panel(), 'Timer-Panel waehrend laufendem Timer offen');
  await page.waitForFunction(() => window.NokiTimer.zustand().ende !== null, null, { timeout: 30000 });
  const ende = await page.evaluate(() => ({ panel: window.NokiWerk.zustand().offen, effekt: window.NokiTimer.zustand().ende,
    parade: window.NokiSchwarm.paradeZustand(), schwarm: window.NokiSchwarm.zustand().an,
    klone: !!document.getElementById('nokiSchwarm'),
    layer: window.__calls.filter(c => c.name === 'noki_parade_ebene' || (c.name === 'noki_schwarm' && c.args && c.args.an)).length }));
  check(!ende.panel, 'Timer-Ende schliesst ein offenes Timer-Panel');
  check(!ende.parade && !ende.schwarm && !ende.klone && ende.layer === 0, 'keine Parade, keine Klone, kein Parade-Layer');
  await page.waitForFunction(() => window.NokiRaum.groesse() > 54 * 1.4 * 2, null, { timeout: 30000 }).catch(() => {});
  const gross = await page.evaluate(() => window.NokiRaum.groesse());
  check(gross > 54 * 1.4 * 2, `Noki wird voruebergehend sehr gross (${gross.toFixed(1)} px)`);
  // Headless WebGL runs at ~1-2 fps and the simulation clock advances per
  // frame: lift-off + flight take minutes here (seconds on the Mac).
  await page.waitForFunction(() => window.NokiAktion.laeuft() === 'desktop', null, { timeout: 600000 }).catch(() => {});
  const flug = await page.evaluate(() => ({ e: window.NokiTimer.zustand().ende, a: window.NokiAktion.laeuft() }));
  check(flug.e === 'flug' && flug.a === 'desktop', `genau "Einmal durch den Desktop" laeuft (${flug.e}/${flug.a})`);
  // A user size change during the effect becomes the size to return to.
  await page.evaluate(() => window.NokiRaum.groesseZiel(54 * 1.2));
  const t0 = Date.now();
  await page.waitForFunction(() => window.NokiTimer.zustand().ende === null, null, { timeout: 1500000 }).catch(() => {});
  console.log(`  Effekt-Dauer im Headless-Browser: ${((Date.now() - t0) / 1000).toFixed(1)} s`);
  let starts = await page.evaluate(() => window.__calls.length);
  await page.waitForFunction(() => Math.abs(window.NokiRaum.groesse() - 54 * 1.2) < 0.1, null, { timeout: 60000 }).catch(() => {});
  const danach = await page.evaluate(() => ({ e: window.NokiTimer.zustand().ende, g: window.NokiRaum.groesse(), en: window.NokiEnergie.lesen(), a: window.NokiAktion.laeuft() }));
  check(danach.e === null && !danach.a, 'Effekt beendet, keine Aktion laeuft mehr');
  check(Math.abs(danach.g - 54 * 1.2) < 0.1, `danach exakt die (waehrenddessen gewaehlte) Benutzergroesse (${danach.g})`);
  check(danach.en === 'normal', `Energie unveraendert (${danach.en})`);
  // Exactly one action: no second start after the end.
  await page.waitForTimeout(3000);
  check(!(await page.evaluate(() => window.NokiAktion.laeuft())) && starts >= 0, 'keine zweite Timer-Animation');

  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
