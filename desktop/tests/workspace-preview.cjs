const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');

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
  const native = fs.readFileSync(path.join(root, 'src-tauri/src/lib.rs'), 'utf8');
  const shortcutFour = native.match(/\n\s*4 => \{([\s\S]*?)\n\s*\}\n\s*5 =>/);
  assert.ok(shortcutFour, 'native Shortcut-4 branch must be present');
  assert.match(shortcutFour[1], /noki:\/\/vorschau_toggle/);
  assert.doesNotMatch(shortcutFour[1], /arbeitsplatz_(besuchen|springen)|zum_space|HERKUNFT/,
    'native Shortcut 4 must not contain Workspace navigation');
  // No return semantics anywhere on the Shortcut-4 path (section 11).
  assert.doesNotMatch(shortcutFour[1], /rueckweg|office_toggle|aufgabe/i,
    'native Shortcut 4 must not return, open the old Office or start a task');

  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.addInitScript(() => {
    window.__calls = [];
    window.__savedPreview = null;
    window.__activeSpace = 1;
    window.__visitFails = false;
    window.__visitHangs = false;
    // Was WIRKLICH auf Nokis Schreibtisch steht (der native Abgleich).
    window.__wsWindows = [101, 102];
    const invoke = async (name, args) => {
      window.__calls.push({ name, args });
      if (name === 'intelligence_settings') return { settings:{ level:'off', ask:true, web:false }, installed:true, loaded:false };
      if (name === 'werk_daten_lesen' && args && args.schluessel === 'vorschau') return { groesse:'kompakt', sichtbar:true };
      if (name === 'werk_daten_schreiben' && args && args.schluessel === 'vorschau') { window.__savedPreview = args.wert; return true; }
      if (name === 'noki_arbeitsplatz_lage') return { ok:true, aktiv:window.__activeSpace, arbeitsplatz:{ id:44, uuid:'NOKI-UUID' }, nummer:4 };
      if (name === 'noki_arbeitsplatz_besuchen') {
        if (window.__visitHangs) return new Promise(() => {});
        if (window.__visitFails) return { ok:false, grund:'probe', aktiv_vorher:window.__activeSpace, aktiv_nachher:window.__activeSpace, arbeitsplatz:44 };
        window.__activeSpace = 44;
        return { ok:true, aktiv_vorher:1, aktiv_nachher:44, arbeitsplatz:44, uuid:'NOKI-UUID' };
      }
      if (name === 'noki_vorschau_setzen') return { ok:true, aufnahme:window.__wsWindows.length, fenster:window.__wsWindows };
      return null;
    };
    window.__nokiEvents = {};
    const listen = async (name, fn) => { (window.__nokiEvents[name] = window.__nokiEvents[name] || []).push(fn); return () => {}; };
    window.__emit = (name, payload) => (window.__nokiEvents[name] || []).forEach(fn => fn({ payload }));
    window.__TAURI__ = { core:{invoke}, event:{listen,emit:async()=>{}}, window:{getCurrentWindow:()=>({onMoved:async()=>()=>{},onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}})} };
    window.__TAURI_INTERNALS__ = { invoke };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/index.html`);
  await page.waitForFunction(() => window.__nokiReady && window.NokiVorschau);
  await page.evaluate(() => {
    window.NokiVorschau.schreibtisch({ x:0, y:0, w:1280, h:800 });
    window.NokiVorschau.nummer(4);
    window.NokiVorschau.ort({ fenster:101, x:650, y:55, w:560, h:430, app:'Google Chrome' });
    window.NokiVorschau.ort({ fenster:102, x:90, y:250, w:500, h:470, app:'Code' });
    window.NokiVorschau.zeigen({});
  });

  const compact = await page.evaluate(() => {
    const el = document.querySelector('#nokiVorschau'), view = document.querySelector('#nokiVorschauViewport');
    const r = el.getBoundingClientRect(), vr = view.getBoundingClientRect();
    return { r:r.toJSON(), vr:vr.toJSON(), label:document.querySelector('#nokiVorschauLabel').textContent,
      header:!!document.querySelector('#nokiVorschauKopf'), dot:!!document.querySelector('#nokiVorschauPunkt'),
      d:window.NokiVorschau.diag() };
  });
  // Die Nummer ist aus der Beschriftung auf den KNOPF gewandert: sie ist
  // kein Etikett mehr, sondern der einzige Klickweg, der noch navigiert.
  // Die Beschriftung nennt nur noch, WESSEN Schreibtisch man sieht.
  assert.equal(compact.label, 'Noki Schreibtisch');
  const naviRuf = await page.evaluate(() =>
    (window.__calls || []).filter(c => c.name === 'noki_vorschau_navi').map(c => c.args.text));
  assert.ok(naviRuf.includes('Zum Schreibtisch 4'),
    'der Navigationsknopf traegt die DYNAMISCHE Nummer: ' + JSON.stringify(naviRuf));
  assert.equal(compact.header, false); assert.equal(compact.dot, false);
  assert.equal(Math.round(compact.r.x), 7);   // linke Kante an der Widget-Flucht
  assert.ok(Math.abs(800 - compact.r.bottom - 22) <= 1);
  assert.ok(compact.r.width >= 260 && compact.r.width <= 470);
  assert.equal(compact.d.fenster.length, 2);

  // Shortcut 4's event is a pure visibility toggle: it must neither invoke
  // Workspace navigation nor mutate the active ManagedSpaceID.
  const shortcut = await page.evaluate(async () => {
    const before = window.__activeSpace;
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 20));
    const hidden = window.NokiVorschau.diag().an;
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 20));
    return { before, after:window.__activeSpace, hidden, shown:window.NokiVorschau.diag().an,
      visits:window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length };
  });
  assert.equal(shortcut.before, 1); assert.equal(shortcut.after, 1);
  assert.equal(shortcut.hidden, false); assert.equal(shortcut.shown, true);
  assert.equal(shortcut.visits, 0, 'Shortcut 4 must never navigate');
  assert.equal((await page.evaluate(() => window.__savedPreview)).sichtbar, true);

  const navBefore = await page.evaluate(() => window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length);
  // Hover over the compact preview expands it to LARGE smoothly
  await page.hover('#nokiVorschau');
  await page.waitForTimeout(280);
  const large = await page.evaluate(() => {
    const r = document.querySelector('#nokiVorschau').getBoundingClientRect();
    return { r:r.toJSON(), d:window.NokiVorschau.diag(),
      nav:window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length };
  });
  assert.equal(large.nav, navBefore, 'hover resize must not navigate');
  assert.equal(large.d.groesse, 'gross');
  assert.deepEqual(large.d.fenster, compact.d.fenster, 'composition and normalized frames remain identical');
  assert.ok(large.r.width >= compact.r.width * 1.7);
  assert.equal(Math.round(large.r.x), 7);
  assert.ok(Math.abs(800 - large.r.bottom - 22) <= 1);
  const desktopArea = (large.r.height - 24) * large.r.width;
  assert.ok(desktopArea / (1280 * 800) > 0.20 && desktopArea / (1280 * 800) < 0.30);

  // Moving pointer away smoothly returns preview to COMPACT
  await page.mouse.move(0, 0);
  await page.waitForTimeout(280);
  const shrunk = await page.evaluate(() => window.NokiVorschau.diag().groesse);
  assert.equal(shrunk, 'kompakt', 'leaving bounds must return to compact');

  // Re-expand to large to verify click and navigation while in large mode
  await page.evaluate(() => window.NokiVorschau.groesse('gross', true));
  await page.waitForTimeout(280);

  if (process.env.NOKI_PREVIEW_SCREENSHOT) await page.screenshot({ path:process.env.NOKI_PREVIEW_SCREENSHOT });
  // Failed native navigation rolls the expansion back and keeps the preview.
  // Content is a remote control: a click on it NEVER navigates.
  await page.click('#nokiVorschauViewport');
  await page.waitForTimeout(300);
  const nurFern = await page.evaluate(() => ({ nav:window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length,
    active:window.__activeSpace, groesse:window.NokiVorschau.diag().groesse }));
  assert.deepEqual(nurFern, { nav:navBefore, active:1, groesse:'gross' }, 'content click is remote-only');

  // Only the explicit footer ("Zum Schreibtisch N") visits.
  await page.evaluate(() => { window.__visitFails = true; });
  await page.evaluate(() => window.NokiVorschau.besuchen());
  await page.waitForTimeout(300);
  const failed = await page.evaluate(() => ({ active:window.__activeSpace,
    an:window.NokiVorschau.diag().an, navigiert:window.NokiVorschau.diag().navigiert,
    transition:document.querySelector('#nokiVorschau').classList.contains('noki-vorschau-navigation') }));
  assert.deepEqual(failed, { active:1, an:true, navigiert:false, transition:false });

  await page.evaluate(() => { window.__visitFails = false; });
  await page.evaluate(() => window.NokiVorschau.besuchen());
  await page.waitForTimeout(300);
  const visit = await page.evaluate(() => ({
    nav:window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length,
    active:window.__activeSpace, shown:window.NokiVorschau.diag().an,
    transitioning:document.querySelector('#nokiVorschau').classList.contains('noki-vorschau-navigation')
  }));
  assert.equal(visit.nav, navBefore + 2);
  assert.equal(visit.active, 44);
  assert.equal(visit.shown, false);
  assert.equal(visit.transitioning, false);

  // ---- Shortcut 4 must recover the preview AFTER a successful entry ----
  // The user is now physically on Noki's Workspace and the transition
  // surface is gone, but the visibility PREFERENCE was never touched.
  const savedAfterVisit = await page.evaluate(() => window.__savedPreview);
  assert.equal(savedAfterVisit.sichtbar, true, 'a Desktop click must not change the user preference');
  assert.equal(savedAfterVisit.groesse, 'gross', 'navigation must not reset the size preference');

  // Shortcut 4 on the preview target never shows the Desktop the user stands
  // on (no picture in its own picture): it asks the backend for ONE step to
  // another Desktop; the Miniatur appears once that retarget is published.
  const recovery = await page.evaluate(async () => {
    const steps = [];
    const snap = () => ({ an: window.NokiVorschau.diag().an, space: window.__activeSpace,
      groesse: window.NokiVorschau.diag().groesse });
    steps.push(snap());                                   // hidden, on Workspace
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 30));
    steps.push(snap());                                   // still hidden: no recursion
    window.__emit('noki://arbeitsplatz_gewaehlt', { nummer: 3, auf_arbeitsplatz: false });
    await new Promise(r => setTimeout(r, 30));
    steps.push(snap());                                   // other Desktop published -> shown
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 30));
    steps.push(snap());                                   // hidden again
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 30));
    steps.push(snap());                                   // and shown once more
    return { steps, visits: window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length,
      anfragen: window.__calls.filter(c => c.name === 'noki_vorschau_zeigen_anfrage').length };
  });
  assert.deepEqual(recovery.steps.map(s => s.an), [false, false, true, false, true],
    'Shortcut 4 on the target must never show a recursive preview; it shows after the retarget');
  assert.equal(recovery.anfragen, 1, 'Shortcut 4 on the target asks exactly once for another Desktop');
  assert.deepEqual(recovery.steps.map(s => s.space), [44, 44, 44, 44, 44],
    'Shortcut 4 must never change the active Space');
  assert.deepEqual(recovery.steps.map(s => s.groesse), ['gross', 'gross', 'gross', 'gross', 'gross'],
    'Shortcut 4 must never change the size');
  assert.equal(recovery.visits, visit.nav, 'Shortcut 4 must never navigate');

  // Leaving the Workspace by hand follows the preference, it never rewrites it.
  await page.evaluate(async () => {
    window.__activeSpace = 1;
    window.__emit('noki://arbeitsplatz_aktiv', { aktiv: false, space: 1, arbeitsplatz: 44, nummer: 4 });
    await new Promise(r => setTimeout(r, 30));
  });
  const zurueck = await page.evaluate(() => window.NokiVorschau.diag());
  assert.equal(zurueck.an, true, 'back on the user Desktop the preference (visible) applies again');
  assert.equal(zurueck.groesse, 'gross');

  // The legacy lower-right X is no longer a product control.  Hiding remains
  // an explicit preference through the existing preview-toggle command, and
  // that preference survives a Workspace round trip.
  assert.equal(await page.locator('#nokiVorschauZu').isVisible(), false,
    'the obsolete close X must never cover the normal miniature');
  await page.evaluate(async () => {
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 30));
  });
  assert.equal((await page.evaluate(() => window.NokiVorschau.diag().an)), false);
  assert.equal((await page.evaluate(() => window.__savedPreview)).sichtbar, false);
  await page.evaluate(async () => {
    window.__emit('noki://arbeitsplatz_aktiv', { aktiv: true, space: 44, arbeitsplatz: 44, nummer: 4 });
    await new Promise(r => setTimeout(r, 20));
    window.__emit('noki://arbeitsplatz_aktiv', { aktiv: false, space: 1, arbeitsplatz: 44, nummer: 4 });
    await new Promise(r => setTimeout(r, 20));
  });
  assert.equal((await page.evaluate(() => window.NokiVorschau.diag().an)), false,
    'an automatic Space change never overwrites the explicit preference');
  // ... and Shortcut 4 still brings it back. No state is ever a dead end.
  await page.evaluate(async () => {
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 30));
  });
  assert.equal((await page.evaluate(() => window.NokiVorschau.diag().an)), true);

  // A click while the native side never answers must not strand the preview.
  await page.evaluate(() => { window.__visitHangs = true; });
  await page.evaluate(() => window.NokiVorschau.besuchen());
  await page.waitForTimeout(400);
  assert.equal((await page.evaluate(() => window.NokiVorschau.diag().navigation)), 'ENTERING');
  await page.waitForTimeout(6200);
  const haenger = await page.evaluate(() => ({ d: window.NokiVorschau.diag(),
    transition: document.querySelector('#nokiVorschau').classList.contains('noki-vorschau-navigation') }));
  assert.equal(haenger.d.navigation, 'IDLE');
  assert.equal(haenger.d.an, true, 'a hanging navigation rolls back to a live preview');
  assert.equal(haenger.transition, false);

  // ---- Section 18: the preview is a miniature of the REAL Workspace ----
  // What the native reconciliation reports is the only membership authority.
  await page.evaluate(async () => {
    window.NokiVorschau.verbergen(true);
    window.__emit('noki://vorschau_toggle', {});      // show again -> reconcile
    await new Promise(r => setTimeout(r, 60));
  });
  let bestand = await page.evaluate(() => window.NokiVorschau.diag().fenster.map(f => f.id).sort());
  assert.deepEqual(bestand, [101, 102], 'both real Workspace windows are in the preview');

  // A window that really closed disappears on the next reconciliation.
  await page.evaluate(async () => {
    window.__wsWindows = [101];
    window.NokiVorschau.verbergen(true);
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 60));
  });
  bestand = await page.evaluate(() => window.NokiVorschau.diag().fenster.map(f => f.id));
  assert.deepEqual(bestand, [101], 'a closed window leaves the preview');

  // Section 4: a stalled capture stream must NOT empty the Desktop. The
  // window is occluded, ScreenCaptureKit stops delivering - it stays.
  await page.evaluate(async () => {
    window.__emit('noki://vorschau_weg', { fenster: 101 });
    await new Promise(r => setTimeout(r, 30));
  });
  const still = await page.evaluate(() => window.NokiVorschau.diag().fenster);
  assert.equal(still.length, 1, 'a stalled stream never removes the window');
  assert.equal(still[0].id, 101);
  assert.equal(still[0].still, true, 'it is marked as no longer repainting');

  // A foreign window on Nokis Space is never added by the preview itself.
  await page.evaluate(async () => {
    window.__wsWindows = [101];            // reconciliation still reports only 101
    window.NokiVorschau.ort({ fenster: 999, x: 10, y: 10, w: 300, h: 200, app: 'Fremd' });
    window.NokiVorschau.verbergen(true);
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 60));
  });
  bestand = await page.evaluate(() => window.NokiVorschau.diag().fenster.map(f => f.id));
  assert.deepEqual(bestand, [101], 'only reconciled windows survive; a stray frame is dropped');

  // ---- Part B: a click beside a LARGE preview collapses it to compact ----
  // Start from a settled, visible, large preview on the user's own Desktop.
  await page.evaluate(async () => {
    window.__activeSpace = 1;
    window.__emit('noki://arbeitsplatz_aktiv', { aktiv: false, space: 1, arbeitsplatz: 44, nummer: 3 });
    await new Promise(r => setTimeout(r, 60));
    window.NokiVorschau.groesse('gross', true);
    await new Promise(r => setTimeout(r, 320));
  });
  // Der Anker selbst wird VORHER gemessen: die Zusage lautet, dass ein Klick
  // daneben ihn nicht verschiebt - nicht, wo genau er liegt.
  const ankerVorher = await page.evaluate(() => {
    const r = document.querySelector('#nokiVorschau').getBoundingClientRect();
    return { left: Math.round(r.left), bottom: Math.round(r.bottom) };
  });
  let zustand = await page.evaluate(() => window.NokiVorschau.diag());
  assert.equal(zustand.groesse, 'gross');
  assert.equal(zustand.an, true);
  assert.equal(zustand.navigation, 'IDLE', 'settled before the outside-click checks');

  // A mousedown INSIDE the preview must not collapse it — that is navigation.
  const grossRect = await page.evaluate(() => document.querySelector('#nokiVorschau').getBoundingClientRect().toJSON());
  await page.evaluate(async (r) => {
    const el = document.elementFromPoint(r.x + r.width / 2, r.y + 40);
    el.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await new Promise(x => setTimeout(x, 300));
  }, grossRect);
  assert.equal(await page.evaluate(() => window.NokiVorschau.diag().groesse), 'gross',
    'a press inside the preview never collapses it first');

  // A mousedown elsewhere in Nokis own window (the character, the stage).
  await page.evaluate(async () => {
    const neben = document.createElement('div');
    neben.id = 'testNebenan';
    document.body.appendChild(neben);
    neben.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await new Promise(r => setTimeout(r, 320));
    neben.remove();
  });
  assert.equal(await page.evaluate(() => window.NokiVorschau.diag().groesse), 'kompakt',
    'a press beside the preview collapses LARGE -> COMPACT');

  // And the native path: a click that went to ANOTHER application.
  const navVorher = await page.evaluate(() => window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length);
  const spaceVorOutside = await page.evaluate(() => window.__activeSpace);
  await page.evaluate(async () => {
    window.NokiVorschau.groesse('gross', true);
    await new Promise(r => setTimeout(r, 320));
    window.__emit('noki://vorschau_kompakt', {});
    await new Promise(r => setTimeout(r, 700));   // die 240-ms-Kurve zu Ende
  });
  const nachOutside = await page.evaluate(() => ({
    d: window.NokiVorschau.diag(), space: window.__activeSpace, saved: window.__savedPreview,
    r: document.querySelector('#nokiVorschau').getBoundingClientRect().toJSON(),
    nav: window.__calls.filter(c => c.name === 'noki_arbeitsplatz_besuchen').length,
    gross: window.__calls.filter(c => c.name === 'noki_vorschau_gross').map(c => c.args.gross)
  }));
  assert.equal(nachOutside.d.groesse, 'kompakt', 'outside click collapses LARGE -> COMPACT');
  assert.equal(nachOutside.d.an, true, 'it collapses, it does not hide');
  assert.equal(nachOutside.d.sichtbarWunsch, true, 'the visibility preference is untouched');
  assert.equal(nachOutside.space, spaceVorOutside, 'an outside click never changes the Space');
  assert.equal(nachOutside.nav, navVorher, 'and never navigates');
  assert.equal(Math.round(nachOutside.r.x), ankerVorher.left, 'the left anchor never moves');
  assert.equal(Math.round(nachOutside.r.bottom), ankerVorher.bottom,
    'and the bottom anchor never moves: it shrinks rightward/upward, it never recenters');
  assert.equal(nachOutside.saved.groesse, 'kompakt', 'the size preference is remembered');
  assert.equal(nachOutside.gross[nachOutside.gross.length - 1], false,
    'the native watcher is told to stop looking once it is compact again');

  // Compact already: an outside click is a no-op.
  assert.equal(await page.evaluate(() => window.NokiVorschau.nebenan()), 'nein');
  assert.equal(await page.evaluate(() => window.NokiVorschau.diag().groesse), 'kompakt');

  // ---- Part C: a normal -> normal Space change must not disturb anything ----
  const vorWechsel = await page.evaluate(() => ({
    d: window.NokiVorschau.diag(),
    setzen: window.__calls.filter(c => c.name === 'noki_vorschau_setzen').length,
    stop: window.__calls.filter(c => c.name === 'noki_vorschau_stop').length
  }));
  await page.evaluate(async () => {
    // The watcher only reports the one thing that matters; a normal -> normal
    // move is not reported at all. Nothing may react to it.
    window.__activeSpace = 2;
    await new Promise(r => setTimeout(r, 120));
  });
  const nachWechsel = await page.evaluate(() => ({
    d: window.NokiVorschau.diag(),
    setzen: window.__calls.filter(c => c.name === 'noki_vorschau_setzen').length,
    stop: window.__calls.filter(c => c.name === 'noki_vorschau_stop').length
  }));
  assert.equal(nachWechsel.d.an, true, 'the preview stays visible across user Desktops');
  assert.equal(nachWechsel.d.groesse, vorWechsel.d.groesse, 'and keeps its size');
  assert.deepEqual(nachWechsel.d.fenster, vorWechsel.d.fenster, 'and its composited windows');
  assert.equal(nachWechsel.stop, vorWechsel.stop, 'capture is never stopped for a normal Space change');
  assert.equal(nachWechsel.setzen, vorWechsel.setzen, 'and never restarted');

  // ---- Section 22: a window born while the preview is ALREADY visible ----
  // The Desktop gained a window; an already-visible preview must take it in,
  // not wait for the next time it is switched on.
  await page.evaluate(async () => {
    window.__activeSpace = 1;
    window.__emit('noki://arbeitsplatz_aktiv', { aktiv: false, space: 1, arbeitsplatz: 44, nummer: 3 });
    window.__wsWindows = [101];
    window.NokiVorschau.verbergen(true);
    window.__emit('noki://vorschau_toggle', {});
    await new Promise(r => setTimeout(r, 80));
  });
  assert.deepEqual(await page.evaluate(() => window.NokiVorschau.diag().fenster.map(f => f.id)), [101]);

  const setzenVorher = await page.evaluate(() => window.__calls.filter(c => c.name === 'noki_vorschau_setzen').length);
  await page.evaluate(async () => {
    window.__wsWindows = [101, 303];          // Noki just created one there
    window.NokiVorschau.zeigen({});           // no reconcile requested
    await new Promise(r => setTimeout(r, 80));
  });
  assert.deepEqual(await page.evaluate(() => window.NokiVorschau.diag().fenster.map(f => f.id)), [101],
    'a plain show of an already visible preview changes nothing');
  assert.equal(await page.evaluate(() => window.__calls.filter(c => c.name === 'noki_vorschau_setzen').length),
    setzenVorher, 'and does not re-arm the capture for nothing');

  await page.evaluate(async () => {
    window.NokiVorschau.zeigen({ abgleichen: true });   // after a Workspace action
    await new Promise(r => setTimeout(r, 80));
  });
  assert.equal(await page.evaluate(() => window.__calls.filter(c => c.name === 'noki_vorschau_setzen').length),
    setzenVorher + 1, 'a Workspace action re-arms the capture on an already visible preview');
  // The capture now covers the new window, so its position arrives and it
  // joins the composition - and it survives the next reconciliation.
  await page.evaluate(async () => {
    window.NokiVorschau.ort({ fenster: 303, x: 100, y: 60, w: 400, h: 300, app: 'Google Chrome' });
    window.NokiVorschau.zeigen({ abgleichen: true });
    await new Promise(r => setTimeout(r, 80));
  });
  assert.deepEqual(await page.evaluate(() => window.NokiVorschau.diag().fenster.map(f => f.id).sort()), [101, 303],
    'the newly created Workspace window is part of the preview Desktop');

  await browser.close(); server.close();
  console.log('workspace preview: all checks passed');
})().catch(error => { console.error(error); server.close(); process.exit(1); });
