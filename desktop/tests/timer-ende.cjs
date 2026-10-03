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
    schwarm: window.NokiSchwarm.zustand().an, klone: !!document.getElementById('nokiSchwarm'),
    schwarmAn: window.__calls.filter(c => c.name === 'noki_schwarm' && c.args && c.args.an).length }));
  check(!ende.panel, 'Timer-Ende schliesst ein offenes Timer-Panel');
  check(!ende.schwarm && !ende.klone && ende.schwarmAn === 0, 'keine Logo-Parade, keine Klone');
  await page.waitForFunction(() => window.NokiRaum.groesse() > 54 * 1.4 * 2, null, { timeout: 30000 }).catch(() => {});
  const gross = await page.evaluate(() => window.NokiRaum.groesse());
  check(gross > 54 * 1.4 * 2, `Noki wird voruebergehend sehr gross (${gross.toFixed(1)} px)`);
  // Follow the ONE round (headless: ~1-2 fps, simulation time per frame).
  const W = await page.evaluate(() => window.NokiRaum.buehne().w);
  const phasen = [], spur = [];
  const t0 = Date.now();
  let warGross = true;
  while (Date.now() - t0 < 1500000) {
    const z = await page.evaluate(() => ({ p: window.NokiSchwarm.paradeZustand(), e: window.NokiTimer.zustand().ende,
      g: window.NokiRaum.groesse(), a: window.NokiAktion.laeuft(), x: window.NokiRaum.ort().x }));
    if (z.p && phasen[phasen.length - 1] !== z.p.phase) { phasen.push(z.p.phase); spur.push(Math.round(z.p.x)); }
    if (z.p && z.p.phase !== 'landen' && z.g < 54 * 1.4 * 2) warGross = false;
    if (z.a === 'desktop') { check(false, 'keine "Einmal durch den Desktop"-Choreografie'); break; }
    if (z.e === null) break;
    await page.waitForTimeout(300);
  }
  console.log(`  Phasen: ${phasen.join(' > ')}  x beim Phasenwechsel: ${spur.join(', ')}  (Breite ${W})`);
  console.log(`  Effekt-Dauer im Headless-Browser: ${((Date.now() - t0) / 1000).toFixed(1)} s`);
  check(phasen[0] === 'winken', 'beginnt mit dem Winken');
  const iAus = phasen.indexOf('aus'), iHeim = phasen.indexOf('heim');
  check(iAus > 0 && iHeim > iAus, 'fliegt hinaus und kommt zurueck (eine Runde)');
  if (iAus > 0 && iHeim > iAus) {
    const rausRechts = spur[iAus] > W / 2, reinLinks = spur[iHeim] < W / 2;
    check(rausRechts === reinLinks, `hinaus auf der einen, herein von der gegenueberliegenden Seite (aus x=${spur[iAus]}, heim x=${spur[iHeim]})`);
  }
  check(warGross, 'bleibt waehrend der Runde gross');
  await page.waitForFunction(() => Math.abs(window.NokiRaum.groesse() - 54 * 1.4) < 0.1, null, { timeout: 120000 }).catch(() => {});
  const zurueckG = await page.evaluate(() => window.NokiRaum.groesse());
  check(Math.abs(zurueckG - 54 * 1.4) < 0.15, `danach exakt die vorherige Benutzergroesse (${zurueckG})`);
  // Normal size control works again afterwards.
  await page.evaluate(() => window.NokiRaum.groesseZiel(54 * 1.2));
  await page.waitForFunction(() => Math.abs(window.NokiRaum.groesse() - 54 * 1.2) < 0.1, null, { timeout: 60000 }).catch(() => {});
  const danach = await page.evaluate(() => ({ e: window.NokiTimer.zustand().ende, p: window.NokiSchwarm.paradeZustand(), g: window.NokiRaum.groesse(), en: window.NokiEnergie.lesen() }));
  check(danach.e === null && !danach.p, 'Runde beendet');
  check(Math.abs(danach.g - 54 * 1.2) < 0.15, `normale Groessensteuerung danach (${danach.g})`);
  check(danach.en === 'normal', `Energie unveraendert (${danach.en})`);
  // 3. Waving still works afterwards, repeatedly (petting greeting path).
  await page.waitForTimeout(3000);
  const w1 = await page.evaluate(() => window.NokiTestGruss());
  await page.waitForTimeout(6000);
  const w2 = await page.evaluate(() => window.NokiTestGruss());
  check(w1 && w2, `Winken danach sofort und wiederholt moeglich (${w1}, ${w2})`);
  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
