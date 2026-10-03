// Arbeitsplatz-Fokus input path, end to end in the browser:
//   settings.html (the real Settings window) <-> index.html (state + logic)
// Clicks in the Settings window are forwarded by DOM path and replayed in the
// main window. Measures every Uni/Coding/Normal/Start/Beenden click while the
// native Focus session is starting/ending (slow backend), plus the Shortcut-8
// quick panel and its toggle.
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
  const port = server.address().port;
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const main = await ctx.newPage();
  const einst = await ctx.newPage();
  // Event bridge between the two "windows".
  await main.exposeBinding('__nachEinst', (_, name, payload) => einst.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [name, payload]).catch(() => {}));
  await einst.exposeBinding('__nachMain', (_, name, payload) => main.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [name, payload]).catch(() => {}));
  const tauriMock = (rolle) => {
    window.__calls = [];
    window.__nokiEvents = {};
    window.__startMs = 1500; window.__endeMs = 1200;
    window.__emit = (name, payload) => (window.__nokiEvents[name] || []).forEach(fn => fn({ payload }));
    const listen = async (name, fn) => { (window.__nokiEvents[name] = window.__nokiEvents[name] || []).push(fn); return () => {}; };
    const warte = ms => new Promise(r => setTimeout(r, ms));
    const invoke = async (name, args) => {
      window.__calls.push({ name, args, t: performance.now() });
      if (name === 'platz_liste') return { ax: true, profile: [{ name: 'Uni', apps: [] }, { name: 'Coding', apps: [] }, { name: 'Normal', apps: [] }] };
      if (name === 'fokus_apps') return [{ name: 'Goodnotes', pfad: '/Applications/Goodnotes.app' }, { name: 'Grapher', pfad: '/System/Applications/Utilities/Grapher.app' }];
      if (name === 'app_icons') return {};
      if (name === 'fokus_sitzung_status') return { aktiv: false };
      if (name === 'fokus_sitzung_start') { await warte(window.__startMs); return { session: 'fs-' + Date.now(), space: 7, focusSpace: 7, apps: [], minimiert: 2, erstellt: 1 }; }
      if (name === 'fokus_sitzung_ende') { await warte(window.__endeMs); return { geschlossen: 1, zurueck: 2, wartet: 0, priorEnergyMode: 'normal' }; }
      if (name === 'intelligence_settings') return { settings: { level: 'off', ask: true, web: false }, installed: true, loaded: false };
      return null;
    };
    const emitTo = async (ziel, name, payload) => {
      if (rolle === 'main' && ziel === 'einstellungen') return window.__nachEinst(name, payload);
      if (rolle === 'einst' && ziel === 'main') return window.__nachMain(name, payload);
    };
    window.__TAURI__ = { core: { invoke }, event: { listen, emit: async () => {}, emitTo },
      window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
    window.__TAURI_INTERNALS__ = { invoke };
  };
  await main.addInitScript(tauriMock, 'main');
  await einst.addInitScript(tauriMock, 'einst');
  main.on('pageerror', e => console.log('main pageerror:', e.message));
  einst.on('pageerror', e => console.log('einst pageerror:', e.message));
  await main.goto(`http://127.0.0.1:${port}/index.html`);
  await main.waitForFunction(() => window.NokiEinstellungen && window.NokiWerk, null, { timeout: 60000 });
  await main.waitForTimeout(1600);         // persisted Focus state is read at 1.3 s
  await einst.goto(`http://127.0.0.1:${port}/settings.html`);
  await einst.waitForTimeout(300);

  await main.evaluate(() => window.NokiEinstellungen.auf('platz'));
  await einst.waitForSelector('#einstellungen [data-e="pwahl"]', { timeout: 5000 });

  const zustand = () => main.evaluate(() => { const z = window.NokiWerk.zustand(); return { fokus: z.fokus, offen: z.offen, was: z.was }; });
  const startAnz = () => main.evaluate(() => window.__calls.filter(c => c.name === 'fokus_sitzung_start').length);
  const endeAnz = () => main.evaluate(() => window.__calls.filter(c => c.name === 'fokus_sitzung_ende').length);
  const aktivSeg = () => einst.evaluate(() => { const b = document.querySelector('#einstellungen .e-seg-btn.aktiv'); return b && b.textContent; });
  // A click in the REAL settings page (mouse, not dispatch): must change the
  // selected profile within 300 ms.
  const segKlick = async (name) => {
    const b = einst.locator('#einstellungen .e-seg-btn', { hasText: new RegExp('^' + name + '$') });
    await b.waitFor({ state: 'visible', timeout: 5000 });
    await b.click({ timeout: 5000, trial: true });
    const t0 = Date.now();
    await b.click({ timeout: 5000 });
    try { await einst.waitForFunction(n => { const x = document.querySelector('#einstellungen .e-seg-btn.aktiv'); return x && x.textContent === n; }, name, { timeout: 1000 }); }
    catch (e) { return -1; }
    return Date.now() - t0;
  };
  const knopf = async (e) => {
    const b = einst.locator(`#einstellungen [data-e="${e}"]`);
    await b.click({ timeout: 5000 });
  };

  // 1. Segment buttons while idle.
  let tot = 0, maxMs = 0;
  for (let i = 0; i < 20; i++) {
    for (const n of ['Uni', 'Coding', 'Normal']) {
      const ms = await segKlick(n);
      if (ms < 0) tot++; else maxMs = Math.max(maxMs, ms);
    }
  }
  check(tot === 0, `Profilwahl idle: 60 Klicks, ${tot} tot (max ${maxMs} ms)`);

  // 2. Start/Beenden cycles; profile switching DURING start and end.
  let totStart = 0, totEnde = 0, totWaehrend = 0;
  for (const n of ['Uni', 'Coding', 'Normal']) {
    for (let i = 0; i < 10; i++) {
      if ((await segKlick(n)) < 0) totWaehrend++;
      const s0 = await startAnz();
      await knopf('pstart');
      await main.waitForTimeout(50);
      if ((await startAnz()) !== s0 + 1) totStart++;
      // While the native start runs: other profile buttons must still react.
      const anderes = n === 'Uni' ? 'Coding' : 'Uni';
      if ((await segKlick(anderes)) < 0) totWaehrend++;
      if ((await segKlick(n)) < 0) totWaehrend++;
      // Beenden is reachable for the running profile immediately.
      const e0 = await endeAnz();
      try { await knopf('pende'); } catch (e) { totEnde++; continue; }
      await main.waitForTimeout(50);
      if ((await endeAnz()) !== e0 + 1) totEnde++;
      // During the native end: profile buttons react.
      if ((await segKlick(anderes)) < 0) totWaehrend++;
      if ((await segKlick(n)) < 0) totWaehrend++;
      await main.waitForTimeout(1300);
    }
  }
  check(totStart === 0, `Start: 30 Starts, ${totStart} ohne Wirkung`);
  check(totEnde === 0, `Beenden: 30x, ${totEnde} ohne Wirkung`);
  check(totWaehrend === 0, `Profilwahl waehrend Start/Ende: ${totWaehrend} tote Klicks`);
  const z = await zustand();
  check(!z.fokus, 'nach dem letzten Beenden kein Fokus aktiv');

  // 3. Settings close/open cycles keep the page clickable.
  let totZyklus = 0;
  for (let i = 0; i < 20; i++) {
    await main.evaluate(() => window.NokiEinstellungen.zu());
    await main.evaluate(() => window.NokiEinstellungen.auf('platz'));
    await einst.waitForTimeout(80);
    if ((await segKlick(i % 2 ? 'Uni' : 'Coding')) < 0) totZyklus++;
  }
  check(totZyklus === 0, `20 Oeffnen/Schliessen-Zyklen: ${totZyklus} tote Klicks`);

  // 4. Configure Uni with an app in Settings: the quick panel must start the
  //    SAME profile config (apps + profile name -> saved window frames).
  await segKlick('Uni');
  await knopf('papp_plus');
  await einst.locator('#einstellungen [data-e="papp_add"]').first().click({ timeout: 2000 });
  await main.waitForTimeout(100);

  // 5. Shortcut 8 (native: noki://werkzeug {was:"platz_schnell"}).
  const acht = () => main.evaluate(() => window.__emit('noki://werkzeug', { was: 'platz_schnell' }));
  // Real mouse click at the button's place (hit-testing incl. the outside
  // catcher), not a synthetic dispatch.
  const schnellKlick = async (name) => {
    const b = main.locator('#werk .w-platz-btn', { hasText: new RegExp('^' + name + '$') });
    await b.waitFor({ state: 'visible', timeout: 5000 });
    const box = await b.boundingBox();
    if (!box) throw new Error('kein Knopf ' + name);
    const t0 = Date.now();
    await main.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    return Date.now() - t0;
  };
  const panel = () => main.evaluate(() => {
    const el = document.getElementById('werk'), z = window.NokiWerk.zustand();
    return { offen: z.offen && z.was === 'platz', fokus: z.fokus,
      knoepfe: el ? [...el.querySelectorAll('.w-platz-btn')].map(b => b.textContent) : [],
      titel: el && el.querySelector('.w-kopf span') ? el.querySelector('.w-kopf span').textContent : '' };
  });
  await main.evaluate(() => { window.__startMs = 400; window.__endeMs = 300; });
  let p = await panel();
  check(!p.fokus, 'vor Shortcut 8: kein Fokus');
  await acht();
  p = await panel();
  check(p.offen && p.titel === 'Arbeitsplatz' && p.knoepfe.join() === 'Uni,Coding,Normal', `Shortcut 8 inaktiv -> Wahl (${p.titel}: ${p.knoepfe.join(', ')})`);
  const einstOffen = await main.evaluate(() => window.NokiEinstellungen.zustand().offen);
  check(!einstOffen, 'Shortcut 8 oeffnet keine Einstellungen');
  const s0 = await startAnz();
  await schnellKlick('Uni');
  const letzterStart = await main.evaluate(() => window.__calls.filter(c => c.name === 'fokus_sitzung_start').pop());
  p = await panel();
  check((await startAnz()) === s0 + 1 && p.fokus && !p.offen, 'Uni gewaehlt -> sofort gestartet, Panel zu');
  check(letzterStart && letzterStart.args.profil === 'Uni' && letzterStart.args.apps.includes('/Applications/Goodnotes.app'),
    `derselbe Profil-Start wie Einstellungen (profil=${letzterStart && letzterStart.args.profil}, apps=${letzterStart && letzterStart.args.apps})`);
  // Pressed again while the session is still being set up: stops, no chooser.
  const e0 = await endeAnz();
  await acht();
  p = await panel();
  check((await endeAnz()) === e0 + 1 && !p.fokus && !p.offen, 'Shortcut 8 bei aktivem Fokus -> sofort Beenden, keine Wahl');
  await main.waitForTimeout(800);
  // Toggle: open, close.
  await acht(); p = await panel(); check(p.offen, 'Shortcut 8 -> Wahl offen');
  await acht(); p = await panel(); check(!p.offen && !p.fokus, 'Shortcut 8 nochmal -> Wahl zu, nichts gestartet');

  // 6. Stress: 10 starts/stops per mode via Shortcut 8 + 20 open/close.
  let fehlS8 = 0;
  for (let i = 0; i < 20; i++) { await acht(); if (!(await panel()).offen) fehlS8++; await acht(); if ((await panel()).offen) fehlS8++; }
  check(fehlS8 === 0, `Shortcut 8: 20 Oeffnen/Schliessen, ${fehlS8} Fehler`);
  const sA = await startAnz(), eA = await endeAnz();
  let fehlZyklus = 0;
  for (const n of ['Uni', 'Coding', 'Normal']) {
    for (let i = 0; i < 10; i++) {
      await acht();
      const s1 = await startAnz();
      const ms = await schnellKlick(n);
      if (ms > 1000 || (await startAnz()) !== s1 + 1) { fehlZyklus++; console.log('  langsam/ohne Start', n, i, ms); }
      if (!(await panel()).fokus) { fehlZyklus++; console.log('  nicht aktiv', n, i); }
      await main.waitForTimeout(i % 2 ? 50 : 500);     // stop during and after setup
      await acht();
      if ((await panel()).fokus) { fehlZyklus++; console.log('  nicht beendet', n, i); }
      await main.waitForTimeout(400);
    }
  }
  const sB = await startAnz(), eB = await endeAnz();
  check(fehlZyklus === 0 && sB - sA === 30 && eB - eA === 30, `30 Start/Stopp ueber Shortcut 8: ${fehlZyklus} Fehler, ${sB - sA} Starts, ${eB - eA} Enden`);
  // No duplicate sessions: the start/stop calls strictly alternate.
  const folge = await main.evaluate(() => window.__calls.filter(c => c.name === 'fokus_sitzung_start' || c.name === 'fokus_sitzung_ende').map(c => c.name === 'fokus_sitzung_start' ? 'S' : 'E').join(''));
  check(!/SS/.test(folge.replace(/^E+/, '')), 'keine doppelte Fokus-Sitzung (Start/Ende wechseln sich ab)');
  // Settings still fully clickable afterwards.
  await main.evaluate(() => window.NokiEinstellungen.auf('platz'));
  await einst.waitForTimeout(100);
  check((await segKlick('Normal')) >= 0 && (await segKlick('Uni')) >= 0, 'Einstellungen danach weiter klickbar');

  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
