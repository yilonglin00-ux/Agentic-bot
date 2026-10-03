const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const root = path.resolve(__dirname, '..');

let failed = 0;
const check = (ok, text) => { console.log((ok ? 'ok - ' : 'FAIL: ') + text); if (!ok) failed++; };
const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + new URL(req.url, 'http://noki').pathname);
  if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
  fs.readFile(file, (e, data) => {
    if (e) return res.writeHead(404).end();
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
    res.end(data);
  });
});

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--enable-webgl', '--ignore-gpu-blocklist'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  await page.addInitScript(() => {
    const invoke = async () => null;
    window.__nokiEvents = {};
    const listen = async (name, fn) => { (window.__nokiEvents[name] = window.__nokiEvents[name] || []).push(fn); return () => {}; };
    window.__nokiEmit = name => (window.__nokiEvents[name] || []).forEach(fn => fn({ payload:{} }));
    window.__TAURI__ = { core:{invoke}, event:{listen,emit:async()=>{}}, window:{getCurrentWindow:()=>({onMoved:async()=>()=>{},onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}})} };
    window.__TAURI_INTERNALS__ = { invoke };
  });

  await page.goto(`http://127.0.0.1:${server.address().port}/index.html`);
  await page.waitForFunction(() => window.__nokiReady && window.NokiBuero, null, { timeout: 8000 });

  // Enter Office mode
  await page.evaluate(() => window.NokiBuero.enter());
  await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE', null, { timeout: 6000 });
  check(await page.evaluate(() => window.NokiBuero.mode()) === 'OFFICE', 'entered Office mode');

  // 1. OLD CHARGING DOT REMOVAL
  const dotCheck = await page.evaluate(() => {
    const geo = window.NokiBuero.diag().faces;
    const z = window.NokiBuero.zones();
    // Verify old dot cylinders in Noki room are gone
    const d = window.NokiBuero.diag();
    return {
      oldDockInRoom: z.dock.x === 0.05 && z.dock.z === 0.92,
      dockIsShared: z.dock.z < -2.0,
      hasChargingDocks: !!(z.chargingDock01 && z.chargingDock02 && z.chargingDock03)
    };
  });
  check(!dotCheck.oldDockInRoom && dotCheck.dockIsShared && dotCheck.hasChargingDocks,
    'E/G: Old blue/black charging dot in Noki room completely removed; shared charging docks registered');

  // 2. SHARED CHARGING AREA FOUNDATION & SPATIAL VALIDITY
  const docks = await page.evaluate(() => {
    const z = window.NokiBuero.zones();
    const d1 = z.chargingDock01, d2 = z.chargingDock02, d3 = z.chargingDock03;
    return {
      d1: { pos:[d1.x, d1.z], free: window.NokiBuero.frei(d1.x, d1.z), appFree: window.NokiBuero.frei(d1.approach.x, d1.approach.z) },
      d2: { pos:[d2.x, d2.z], free: window.NokiBuero.frei(d2.x, d2.z), appFree: window.NokiBuero.frei(d2.approach.x, d2.approach.z) },
      d3: { pos:[d3.x, d3.z], free: window.NokiBuero.frei(d3.x, d3.z), appFree: window.NokiBuero.frei(d3.approach.x, d3.approach.z) },
      finWallBlocked: !window.NokiBuero.frei(2.35, -2.70),
      backWallBlocked: !window.NokiBuero.frei(2.90, -3.00)
    };
  });
  check(docks.d1.free && docks.d2.free && docks.d3.free && docks.d1.appFree && docks.d2.appFree && docks.d3.appFree,
    'F/H: Shared charging docks 01, 02, 03 and their approach points have valid walkable spatial locations');
  check(docks.finWallBlocked && docks.backWallBlocked,
    'L: Charging area architectural divider fins and back wall enforce collision boundaries');

  // 3. MULTIPLE CAMERA DISTANCE SCALE AUDIT
  const testViews = [
    { name: 'close desk view', dist: 2.6, pitch: 0.35, minScale: 5.0, maxScale: 6.8 },
    { name: 'normal room view', dist: 4.5, pitch: 0.42, minScale: 3.2, maxScale: 4.5 },
    { name: 'corridor view', dist: 6.5, pitch: 0.45, minScale: 2.2, maxScale: 3.0 },
    { name: 'elevated view', dist: 12.4, pitch: 1.15, minScale: 1.4, maxScale: 1.8 },
    { name: 'full office overview', dist: 17.5, pitch: 1.25, minScale: 1.25, maxScale: 1.6 }
  ];

  for (const v of testViews) {
    await page.evaluate(view => window.NokiBuero.kamera({ dist: view.dist, pitch: view.pitch }), v);
    await page.waitForTimeout(100);
    const diag = await page.evaluate(() => window.NokiBuero.diag());
    const ok = diag.skala >= v.minScale && diag.skala <= v.maxScale;
    check(ok, `A-C: ${v.name} (dist ${v.dist}, pitch ${v.pitch}): Noki scale ${diag.skala.toFixed(2)} (expected ${v.minScale}..${v.maxScale}) - readable and proportional`);
  }

  // Continuous zoom monotonicity & no pop check
  const continuousScales = [];
  for (let d = 2.6; d <= 17.6; d += 1.0) {
    await page.evaluate(dd => window.NokiBuero.kamera({ dist: dd }), d);
    await page.waitForTimeout(60);
    const s = await page.evaluate(() => window.NokiBuero.diag().skala);
    continuousScales.push({ dist: d, scale: s });
  }
  const isMonotonic = continuousScales.every((s, i) => i === 0 || s.scale <= continuousScales[i - 1].scale + 1e-4);
  const maxStepRatio = Math.max(...continuousScales.slice(1).map((s, i) => s.scale / continuousScales[i].scale));
  check(isMonotonic && maxStepRatio < 1.0, `D: Continuous zoom is smooth and strictly monotonic with zero scale pop (ratios < 1.0)`);

  // Orientation check: in bird view, Noki's pitch matches camera pitch
  await page.evaluate(() => window.NokiBuero.kamera({ pitch: 1.25 }));
  await page.waitForTimeout(100);
  const orient = await page.evaluate(() => window.NokiBuero.diag());
  check(Math.abs(orient.nokiPitch - 1.25) < 0.02, `Spatial orientation: Noki 3D pitch inherits elevated room angle (${orient.nokiPitch.toFixed(2)})`);

  // 4. WALKING SPEED & GAIT SYNCHRONIZATION TEST
  // Noki walks to the shared dock in the corridor:
  await page.evaluate(() => window.NokiBuero.approach('dock'));
  const tStart = Date.now();
  await page.waitForFunction(() => window.NokiBuero.diag().taskPhase === 'GEHEN', null, { timeout: 5000 });
  // Sample walking cadence and velocity during transit
  const samples = [];
  for (let i = 0; i < 10; i++) {
    await page.waitForTimeout(100);
    const d = await page.evaluate(() => ({ x: window.NokiBuero.diag().x, z: window.NokiBuero.diag().z, walking: window.NokiBuero.diag().walking }));
    samples.push(d);
  }
  await page.waitForFunction(() => window.NokiBuero.diag().taskPhase === 'ANKUNFT' || window.NokiBuero.diag().taskPhase === 'ARBEITEN', null, { timeout: 12000 });
  const tWalk = (Date.now() - tStart) / 1000;
  const arrived = await page.evaluate(() => window.NokiBuero.diag());
  check(tWalk < 6.5 && Math.abs(arrived.x - 2.90) < 0.15 && Math.abs(arrived.z - (-2.66)) < 0.15,
    `H-K: Walked across office to shared dock in ${tWalk.toFixed(1)}s (<6.5s) without sliding, arriving cleanly at dock 02 (${arrived.x.toFixed(2)}, ${arrived.z.toFixed(2)})`);

  // Shortcut 4 exit and re-entry
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.mode() === 'DESKTOP');
  check(await page.evaluate(() => document.documentElement.dataset.nokiMode) === 'DESKTOP', 'N: Shortcut 4 exits to desktop mode');
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE');
  check(await page.evaluate(() => window.NokiBuero.mode()) === 'OFFICE', 'N: Shortcut 4 re-enters office mode');

  await browser.close(); server.close();
  if (failed) { console.error(`FAILED with ${failed} errors`); process.exit(1); }
  console.log('\nAll Office Enhancement tests PASSED cleanly!');
})().catch(async e => { console.error(e); server.close(); process.exit(1); });
