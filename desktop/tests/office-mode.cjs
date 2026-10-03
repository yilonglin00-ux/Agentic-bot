// Nokis Büro: live frontend regression, using installed Chrome and no downloads.
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
    const invoke = async name => name === 'intelligence_settings'
      ? { settings: { level:'off', ask:true, web:false }, installed:true, loaded:false }
      : null;
    // Die Ereignisse der nativen Seite werden mitgeschnitten, damit der
    // Test Kuerzel 4 genau so ausloesen kann wie Rust es tut.
    window.__nokiEvents = {};
    const listen = async (name, fn) => { (window.__nokiEvents[name] = window.__nokiEvents[name] || []).push(fn); return () => {}; };
    window.__nokiEmit = name => (window.__nokiEvents[name] || []).forEach(fn => fn({ payload:{} }));
    window.__TAURI__ = { core:{invoke}, event:{listen,emit:async()=>{}}, window:{getCurrentWindow:()=>({onMoved:async()=>()=>{},onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}})} };
    window.__TAURI_INTERNALS__ = { invoke };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/index.html`);
  await page.waitForFunction(() => window.__nokiReady && window.NokiBuero, null, { timeout:8000 }).catch(async e => {
    const state = await page.evaluate(() => ({ ready:window.__nokiReady, buero:typeof window.NokiBuero, raum:typeof window.NokiRaum }));
    throw new Error(`Frontend nicht bereit: ${JSON.stringify(state)}; ${errors.join(' | ')}; ${e.message}`);
  });
  const desktop = await page.evaluate(() => window.NokiRaum.ort());

  // Kuerzel 4 gehoert allein der Miniatur (noki://vorschau_toggle). Das
  // alte office_toggle ist bewusst abgehaengt: es darf NICHTS oeffnen.
  await page.evaluate(() => window.__nokiEmit('noki://office_toggle'));
  await page.waitForTimeout(400);
  check(await page.evaluate(() => window.NokiBuero.mode()) === 'DESKTOP',
    'shortcut 4 / office_toggle no longer opens the office (Miniatur owns shortcut 4)');
  // The office itself stays programmatically reachable until it is deleted.
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE', null, { timeout: 6000 });
  check(await page.evaluate(() => window.NokiBuero.mode()) === 'OFFICE', 'the programmatic toggle opens the office');
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.mode() === 'DESKTOP', null, { timeout: 6000 });
  check(await page.evaluate(() => document.documentElement.dataset.nokiMode) === 'DESKTOP',
    'the same toggle closes it again — no mixed state');

  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE');
  const entered = await page.evaluate(() => ({ d:window.NokiBuero.diag(), zones:Object.keys(window.NokiBuero.zones()), attr:document.documentElement.dataset.nokiMode, canvas:getComputedStyle(document.querySelector('#nokiBuero')).display, vorn:getComputedStyle(document.querySelector('#nokiBueroVorn')).display, bar:getComputedStyle(document.querySelector('#bueroSichten')).display, spur:getComputedStyle(document.querySelector('#spur')).display, unleash:getComputedStyle(document.querySelector('#unleash')).display, effekte:getComputedStyle(document.querySelector('#effekte')).display }));
  check(entered.attr === 'OFFICE' && entered.canvas === 'block', 'Shortcut target enters the dedicated OFFICE scene');
  check(entered.vorn === 'block' && entered.d.faces > 500,
    `the room is built as a real scene on both depth layers (${entered.d.faces} faces, front layer ${entered.vorn})`);
  check(entered.spur === 'none' && entered.unleash === 'none' && entered.effekte === 'none',
    'no flight trail, speed dots or effect overlays inside the office');
  check(entered.zones.includes('flur') && entered.zones.includes('lounge'),
    'the corridor outside the glass front is part of the walkable area');

  // ---- Architektur IST Kollision -------------------------------------
  const plan = await page.evaluate(() => window.NokiBuero.plan());
  check(plan && plan.boden === 4 && plan.sperren > 30 && plan.tueren.length === 3,
    `collision comes from the building plan (${plan && plan.boden} floors, ${plan && plan.sperren} blockers, ${plan && plan.tueren.length} doors)`);
  const tuerAuf = plan.tueren.filter(t => t.offen);
  check(tuerAuf.length === 1, `exactly one door stands open — Nokis own (${plan.tueren.map(t=>t.tag+':'+t.offen).join(' ')})`);
  const T = tuerAuf[0], tx = (T.x0 + T.x1) / 2;

  const k = await page.evaluate(([tx]) => ({
    // A: massive Trennwand zwischen zwei Modulen
    wandPunkt : window.NokiBuero.frei(-1.94, 1.8),
    wandWeg   : window.NokiBuero.wegFrei(-1.2, 1.8, -2.8, 1.8),
    // B: Glasfront eines Nachbarraums
    glasPunkt : window.NokiBuero.frei(-3.9, 0.0),
    glasWeg   : window.NokiBuero.wegFrei(-3.9, -0.8, -3.9, 0.8),
    // C: geschlossene Tuer von Agent 01
    zuWeg     : window.NokiBuero.wegFrei(-5.19, -0.8, -5.19, 0.8),
    // D: offene Tuer von Agent 02
    aufWeg    : window.NokiBuero.wegFrei(tx, -0.8, tx, 0.9),
    aufPunkt  : window.NokiBuero.frei(tx, 0.0),
    // G: Moebel
    tisch     : window.NokiBuero.frei(1.40, 1.95),
    sideboard : window.NokiBuero.frei(-1.56, 2.40),
    bank      : window.NokiBuero.frei(-4.05, -2.43),
    // Sprung ueber die Glaslinie neben der Tuer
    sprung    : window.NokiBuero.wegFrei(0.9, 0.6, 0.9, -0.6),
    frei1     : window.NokiBuero.frei(-0.6, 1.2),
    frei2     : window.NokiBuero.frei(0.3, -1.4)
  }), [tx]);
  check(!k.wandPunkt && !k.wandWeg, 'A: a solid divider wall cannot be stood in or crossed');
  check(!k.glasPunkt && !k.glasWeg, 'B: a glass panel cannot be stood in or crossed');
  check(!k.zuWeg, 'C: a closed door blocks passage');
  check(k.aufWeg && k.aufPunkt, 'D: the open door is a real, passable opening');
  check(!k.tisch && !k.sideboard && !k.bank, 'G: desk, sideboard and bench are solid');
  check(!k.sprung, 'no tunnelling: a step across the glass line beside the door is rejected');
  check(k.frei1 && k.frei2, 'room floor and corridor floor are both walkable');

  // ---- H/I: Nokis Bildgroesse folgt der Projektion --------------------
  const skalen = [];
  for (const d of [2.6, 4.0, 6.0, 9.0, 13.0, 17.0]) {
    await page.evaluate(dd => window.NokiBuero.kamera({ dist: dd }), d);
    await page.waitForTimeout(120);
    const g = await page.evaluate(() => ({ d: window.NokiBuero.diag(), k: window.NokiBuero.kamera(),
      t: getComputedStyle(document.querySelector('#gl')).transform }));
    const m = g.t.match(/matrix\(([^,]+),/);
    // Gegen den TATSAECHLICHEN Kamerastand messen, nicht gegen den gewuenschten
    // Abstand: die Huelle darf die Kamera heranziehen.
    const echt = Math.hypot(g.k.pos.x - g.d.x, g.k.pos.y - (g.d.y + 0.5), g.k.pos.z - g.d.z);
    skalen.push({ dist: echt, scale: m ? parseFloat(m[1]) : 0 });
  }
  const skalenFallend = skalen.every((s, i) => i === 0 || s.scale < skalen[i - 1].scale);
  const lesbarFar = skalen[5].scale >= 1.25;
  check(skalenFallend && skalen[0].scale > skalen[5].scale * 4 && lesbarFar,
    `H/I: Nokis screen size follows perspective with perceptual readability compensation (${skalen.map(s => s.scale.toFixed(2)).join(' → ')}, near vs far ratio ${(skalen[0].scale / skalen[5].scale).toFixed(2)})`);
  check(['computer','printer','chair','work','chargingDock01','chargingDock02','chargingDock03','dock','data','system','files','clipboard','timer','tools','documents','tasks','ai'].every(x => entered.zones.includes(x)), 'current, shared charging docks and future interaction anchors are registered');
  check(entered.d.x >= -6.1 && entered.d.x <= 6.1 && entered.d.z >= -3.1 && entered.d.z <= 3.7, 'Noki starts inside office-local bounds');

  // ---- Noki steht IM Raum, nicht davor -------------------------------
  check(entered.d.labels === 0, 'the Agent 01/02/03 glass labels are gone (room ids stay internal)');
  const hoehen = [];
  for (const pi of [0.14, 0.45, 0.80, 1.15]) {
    await page.evaluate(pp => window.NokiBuero.kamera({ pitch: pp }), pi);
    await page.waitForTimeout(120);
    hoehen.push(await page.evaluate(() => window.NokiBuero.diag()));
  }
  check(hoehen.every(h => Math.abs(h.nokiPitch - h.camera.pitch) < 0.02),
    `Nokis own render camera inherits the room camera's elevation (${hoehen.map(h=>h.nokiPitch.toFixed(2)).join(' ')})`);
  check(hoehen[3].nokiPitch - hoehen[0].nokiPitch > 0.9,
    'in bird view Noki is seen from above, not frontally');
  check(hoehen.every(h => h.schweb === 0),
    'no hover/thruster/SPEED channel is active inside the office');

  // ---- Jeder anlaufbare Punkt ist auch erreichbar ---------------------
  const zonenFrei = await page.evaluate(() => {
    const Z = window.NokiBuero.zones(), r = {};
    Object.keys(Z).forEach(k => { if (!Z[k].future) r[k] = window.NokiBuero.frei(Z[k].x, Z[k].z); });
    return r;
  });
  check(Object.values(zonenFrei).every(Boolean),
    `every interaction point lies in free space (${Object.entries(zonenFrei).map(([k,v])=>k+':'+(v?'ok':'BLOCKED')).join(' ')})`);

  await page.evaluate(() => window.NokiBuero.uebersicht(true));
  await page.waitForTimeout(150);
  check(entered.d.heim === true && entered.d.camera.pitch > 0.5 && entered.d.camera.dist > 10,
    `Shortcut 4 starts on the elevated overview (pitch ${entered.d.camera.pitch.toFixed(2)}, dist ${entered.d.camera.dist.toFixed(1)})`);

  const cam0 = entered.d.camera;
  if (process.env.NOKI_OFFICE_QUICK) console.log('pointer target:', await page.evaluate(() => {
    const e=document.elementFromPoint(900,360), c=document.querySelector('#nokiBuero'), s=document.querySelector('#stage');
    return { id:e&&e.id, office:getComputedStyle(c).pointerEvents, stage:getComputedStyle(s).pointerEvents, rect:c.getBoundingClientRect().toJSON() };
  }));
  // Zwei-Finger-Bewegung (wheel ohne ctrlKey) dreht; Zusammenziehen
  // (wheel MIT ctrlKey) zoomt. Ziehen bleibt als zweiter Weg zum Drehen.
  await page.mouse.move(900, 360); await page.mouse.down(); await page.mouse.move(760, 430, {steps:8}); await page.mouse.up();
  await page.mouse.wheel(-120, 90);
  await page.waitForTimeout(700);
  const cam1 = await page.evaluate(() => window.NokiBuero.diag().camera);
  check(Math.abs(cam1.yaw-cam0.yaw) > .2 && Math.abs(cam1.pitch-cam0.pitch) > .1 && Math.abs(cam1.dist-cam0.dist) < .01,
    `two-finger movement orbits and tilts without changing distance (${JSON.stringify(cam0)} -> ${JSON.stringify(cam1)})`);
  await page.evaluate(() => { const c = document.querySelector('#nokiBuero');
    c.dispatchEvent(new WheelEvent('wheel', { deltaY: -50, ctrlKey: true, bubbles: true, cancelable: true })); });
  await page.waitForTimeout(700);
  const cam2 = await page.evaluate(() => window.NokiBuero.diag().camera);
  check(cam2.dist < cam1.dist - 0.2, `pinch zooms in (${cam1.dist.toFixed(1)} -> ${cam2.dist.toFixed(1)})`);
  if (process.env.NOKI_OFFICE_QUICK) {
    await browser.close(); server.close();
    if (failed) process.exit(1);
    return;
  }

  // Der Weg zum Stuhl: erst wenden, dann GEHEN (mit laufenden Beinen,
  // nicht geflogen), dann sitzen — und zwar AUF der Sitzflaeche.
  await page.evaluate(() => window.NokiBuero.approach('chair'));
  await page.waitForFunction(() => { const d = window.NokiBuero.diag(); return d.taskPhase === 'GEHEN' && d.walking; }, null, { timeout: 12000 });
  const laufend = await page.evaluate(() => window.NokiBuero.diag());
  check(laufend.walking && laufend.y === 0 && laufend.taskPhase === 'GEHEN', 'the chair route is walked on foot, on the floor');
  await page.waitForFunction(() => window.NokiBuero.diag().sitzU > .99, null, { timeout: 40000 });
  const seated = await page.evaluate(() => window.NokiBuero.diag());
  check(seated.task === 'chair' && seated.taskPhase === 'SITZEN' && seated.seated > .8, 'chair route reaches the articulated seated state');
  check(Math.abs(seated.x - 0.68) < .02 && Math.abs(seated.z - 1.95) < .02 && seated.y > .30 && seated.y < .36 && !seated.walking,
    `seated on the chair, not beside or inside it (x ${seated.x.toFixed(2)} y ${seated.y.toFixed(3)} z ${seated.z.toFixed(2)})`);
  // ---- Freie Kamera: kein Einrasten, keine Rueckholung ----------------
  const rund2 = a => Math.atan2(Math.sin(a), Math.cos(a));
  const wo = await page.evaluate(() => window.NokiBuero.kamera({ yaw: -2.2, pitch: 0.33, dist: 4.4 }));
  await page.waitForTimeout(1500);
  const bleibt = await page.evaluate(() => window.NokiBuero.diag().camera);
  check(Math.abs(rund2(bleibt.yaw - (-2.2))) < 1e-6 && Math.abs(bleibt.dist - 4.4) < 1e-6 && seated.heim === false,
    `a hand-set camera stays exactly where the user left it (yaw ${bleibt.yaw.toFixed(2)}, dist ${bleibt.dist.toFixed(2)})`);
  const rund = [];
  for (let a = -3.0; a <= 3.0; a += 0.5) rund.push(await page.evaluate(y => window.NokiBuero.kamera({ yaw: y }).yaw, a));
  check(rund.every((v, n) => Math.abs(rund2(v - (-3.0 + n * 0.5))) < 1e-6),
    `free rotation is continuous — no snapping to directions (${rund.length} angles)`);
  // Pfeiltasten: weich, stetig, ohne Sprung.
  await page.evaluate(() => window.NokiBuero.kamera({ dist: 12.0 }));
  await page.evaluate(() => window.NokiBuero.taste('in', true));
  const zoomfolge = [];
  for (let i = 0; i < 7; i++) { await page.waitForTimeout(130); zoomfolge.push(await page.evaluate(() => window.NokiBuero.diag().camera.dist)); }
  await page.evaluate(() => window.NokiBuero.taste('in', false));
  const fallend = zoomfolge.every((v, i) => i === 0 || v < zoomfolge[i-1] + 1e-9);
  const spruenge = zoomfolge.slice(1).map((v, i) => Math.abs(v - zoomfolge[i]));
  check(fallend && zoomfolge[6] < zoomfolge[0] - 1.0 && Math.max(...spruenge) < 1.6,
    `ArrowUp zooms in smoothly and continuously (${zoomfolge.map(v=>v.toFixed(1)).join(' → ')})`);
  // Nach dem Loslassen laeuft der Zoom bewusst noch kurz aus (rund 0.8 s)
  // und steht dann. Geprueft wird der STILLSTAND danach, nicht ein
  // abruptes Ende — ruckfrei war ausdruecklich gefordert.
  await page.waitForTimeout(1200);
  const nachLos = await page.evaluate(() => window.NokiBuero.diag().camera.dist);
  await page.waitForTimeout(600);
  const spaeter = await page.evaluate(() => window.NokiBuero.diag().camera.dist);
  check(Math.abs(spaeter - nachLos) < 0.01,
    `releasing the key brings the zoom to a full stop (${nachLos.toFixed(3)} → ${spaeter.toFixed(3)})`);

  const grenz = await page.evaluate(() => [window.NokiBuero.kamera({ pitch: 3.0, dist: 999 }), window.NokiBuero.kamera({ pitch: -3.0, dist: 0.01 })]);
  check(grenz[0].pitch < 1.4 && grenz[0].dist <= 18 && grenz[1].pitch > 0.05 && grenz[1].dist >= 2.0 && grenz[0].pos.y > 0,
    `camera stays inside safe bounds (${grenz[0].pitch.toFixed(2)}/${grenz[0].dist.toFixed(1)} and ${grenz[1].pitch.toFixed(2)}/${grenz[1].dist.toFixed(1)})`);
  await page.evaluate(() => window.NokiBuero.uebersicht());
  await page.waitForFunction(() => window.NokiBuero.diag().heim && window.NokiBuero.diag().camera.dist > 11, null, { timeout: 6000 });
  check(true, 'the single "Übersicht" control returns to the initial overview');

  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.mode() === 'DESKTOP');
  const restored = await page.evaluate(() => ({ p:window.NokiRaum.ort(), attr:document.documentElement.dataset.nokiMode, d:window.NokiBuero.diag() }));
  check(restored.attr === 'DESKTOP' && Math.abs(restored.p.x-desktop.x)<.01 && Math.abs(restored.p.y-desktop.y)<.01 && Math.abs(restored.p.dreh-desktop.dreh)<.01, 'exit restores the exact Desktop position/orientation');

  for (let i=0;i<20;i++) {
    await page.evaluate(() => window.NokiBuero.toggle());
    await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE');
    await page.evaluate(() => window.NokiBuero.toggle());
    await page.waitForFunction(() => window.NokiBuero.mode() === 'DESKTOP');
  }
  // 50+ echte Zeiger-/Radereignisse: es darf kein Zuhoerer und kein
  // zweiter Renderlauf dazukommen.
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE');
  for (let i = 0; i < 18; i++) {
    await page.mouse.move(640 + (i % 5) * 20, 380);
    await page.mouse.down();
    await page.mouse.move(600 - i * 3, 400 + (i % 7) * 4, { steps: 3 });
    await page.mouse.up();
    await page.mouse.wheel(0, i % 2 ? 120 : -120);
  }
  const nachInput = await page.evaluate(() => window.NokiBuero.diag());
  check(nachInput.listeners === 8 && nachInput.mode === 'OFFICE' && nachInput.camera.dist >= 2.0 && nachInput.camera.dist <= 18.1,
    `54 orbit/zoom interactions keep listeners at ${nachInput.listeners} and the camera in bounds (dist ${nachInput.camera.dist.toFixed(1)})`);
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.mode() === 'DESKTOP');

  const after = await page.evaluate(() => window.NokiBuero.diag());
  // 5 Zuhoerer auf der Leinwand + 1 delegierter auf der Blickwinkelleiste.
  check(after.listeners === 8 && after.toggles === 46, '20 enter/exit cycles keep one fixed listener set');
  check(errors.length === 0, 'no uncaught errors during office lifecycle' + (errors[0] ? ': '+errors[0] : ''));
  await page.evaluate(() => window.NokiBuero.toggle());
  await page.waitForFunction(() => window.NokiBuero.diag().phase === 'ACTIVE');
  // Der Bereich ist groesser als das fruehere Zimmer; bis Blickrichtung und
  // Kamerafahrt ausgelaufen sind, vergeht etwas mehr Zeit. Geprueft wird
  // unveraendert, DASS die stehende Szene in den tiefen Leerlauf faellt.
  await page.waitForFunction(() => window.NokiBuero.diag().deepIdle, null, { timeout: 8000 });
  const idle0 = await page.evaluate(() => window.NokiBuero.diag());
  await page.waitForTimeout(700);
  const idle1 = await page.evaluate(() => window.NokiBuero.diag());
  check(idle1.deepIdle && idle1.draws-idle0.draws <= 4, `static office joins Deep Idle (${idle1.draws-idle0.draws} draws / 700 ms)`);
  if (process.env.NOKI_OFFICE_SHOT) await page.screenshot({ path:process.env.NOKI_OFFICE_SHOT });
  await browser.close(); server.close();
  if (failed) process.exit(1);
  console.log('\nNokis Büro: alle Live-Prüfungen bestanden');
})().catch(async e => { console.error(e); server.close(); process.exit(1); });
