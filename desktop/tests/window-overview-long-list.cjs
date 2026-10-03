// Shortcut 9 ("Offene Fenster") with a long inventory: every entry must be
// reachable (no silent cut at 36), the card column scrolls by DOM wheel and
// by Noki's native wheel/trackpad stream, top->bottom->top, and a click after
// scrolling activates exactly the card under the pointer (no offset).
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const N = 60;

const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + new URL(req.url, 'http://noki').pathname);
  if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
  fs.readFile(file, (error, data) => {
    if (error) return res.writeHead(404).end();
    res.setHeader('Content-Type', 'text/html');
    res.end(data);
  });
});

(async () => {
  // Native side: the 36 hard stop is gone from the inventory loop.
  const native = fs.readFileSync(path.join(root, 'src-tauri/src/window_overview.rs'), 'utf8');
  assert.doesNotMatch(native, /items\.len\(\) >= 36/, 'inventory must not stop at 36 entries');
  assert.match(native, /const MAX_EINTRAEGE: usize = (\d+);/);
  assert.ok(Number(native.match(/const MAX_EINTRAEGE: usize = (\d+);/)[1]) >= 200);

  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage({ viewport: { width: 360, height: 640 } });
  await page.addInitScript(n => {
    const handlers = {};
    window.__calls = [];
    const fenster = Array.from({ length: n }, (_, i) => ({
      id: 1000 + i, pid: 500 + (i % 7), app: i === 0 ? 'Ask Noki' : `App ${i}`, title: `Fenster ${i}`,
      preview: '', icon: '', minimized: false, chrome: false, stand_ms: 0, auf_space: true, drosselt: false,
    }));
    window.__TAURI__ = {
      core: { invoke: async (cmd, args) => {
        window.__calls.push([cmd, args]);
        switch (cmd) {
          case 'window_overview_state': return true;
          case 'window_overview_geometry': return { card_w: 300, card_h: 150 };
          case 'window_overview_list': return fenster;
          case 'window_overview_frames': return [];
          case 'window_overview_action': return true;
          default: return null;
        }
      } },
      event: { listen: async (name, fn) => { handlers[name] = fn; return () => {}; } },
    };
    window.__emit = (name, payload) => handlers[name] && handlers[name]({ payload });
  }, N);
  const port = server.address().port;
  await page.goto(`http://127.0.0.1:${port}/window-overview.html`);
  await page.waitForFunction(n => document.querySelectorAll('.card').length === n, N);

  const info = await page.evaluate(() => {
    const r = document.getElementById('rail');
    return { cards: r.querySelectorAll('.card').length, sh: r.scrollHeight, ch: r.clientHeight, hint: document.getElementById('hint').textContent };
  });
  assert.equal(info.cards, N, 'all entries rendered');
  assert.equal(info.hint, `${N} Fenster`);
  assert.ok(info.sh > info.ch * 5, 'long list overflows the column');
  const top = () => page.evaluate(() => document.getElementById('rail').scrollTop);
  const max = () => page.evaluate(() => { const r = document.getElementById('rail'); return r.scrollHeight - r.clientHeight; });

  // DOM mouse wheel over the rail: down to the very end, then back up.
  await page.mouse.move(180, 300);
  for (let i = 0; i < 80 && (await top()) < (await max()) - 1; i++) await page.mouse.wheel(0, 400);
  assert.ok(Math.abs((await top()) - (await max())) <= 1, 'wheel reaches the last entry');
  const docScroll = await page.evaluate(() => document.scrollingElement.scrollTop + window.scrollY);
  assert.equal(docScroll, 0, 'wheel never scrolls the page/panel itself');
  // Click exactly the card under the pointer after scrolling.
  const ziel = await page.evaluate(() => {
    const el = document.elementFromPoint(180, 400).closest('.card');
    return Number(el.dataset.wid);
  });
  await page.mouse.click(180, 400);
  const akt = await page.evaluate(() => window.__calls.filter(c => c[0] === 'window_overview_action').pop());
  assert.equal(akt[1].id, ziel, 'click after scrolling hits the card under the pointer');
  assert.equal(akt[1].action, 'activate');
  assert.ok(ziel >= 1000 + N - 6, `bottom card reachable (got ${ziel})`);
  // The same card stays clickable after an activation (was stuck 'busy').
  await page.mouse.click(180, 400);
  const zweimal = await page.evaluate(id => window.__calls.filter(c => c[0] === 'window_overview_action' && c[1].id === id).length, ziel);
  assert.equal(zweimal, 2, 'second click on the same card activates again');
  for (let i = 0; i < 80 && (await top()) > 0; i++) await page.mouse.wheel(0, -400);
  assert.equal(await top(), 0, 'wheel returns to the first entry');

  // Native stream (panel never key): classic wheel notches (phase 0) and a
  // trackpad gesture (phases 1/2/4 + momentum) both scroll the column.
  await page.evaluate(() => { for (let i = 0; i < 20; i++) window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -3, phase: 0, momentum: 0 }); });
  const nachRad = await top();
  assert.ok(nachRad > 0, 'native wheel notches scroll down');
  await page.evaluate(() => {
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -10, phase: 1, momentum: 0 });
    for (let i = 0; i < 30; i++) window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -25, phase: 2, momentum: 0 });
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: 0, phase: 4, momentum: 0 });
  });
  assert.ok((await top()) > nachRad + 500, 'native trackpad gesture scrolls down');
  // Release: macOS momentum deltas keep the list gliding (were dropped).
  const vorSchwung = await top();
  await page.evaluate(() => {
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -12, phase: 0, momentum: 1 });
    for (let i = 12; i > 0; i--) window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -i, phase: 0, momentum: 2 });
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: 0, phase: 0, momentum: 3 });
  });
  assert.equal((await top()) - vorSchwung, 12 + 78, 'momentum after release applied exactly as delivered');
  // No dead zone at the start of a vertical gesture on a card without tabs.
  const vorStart = await top();
  await page.evaluate(() => {
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -3, phase: 1, momentum: 0 });
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: -3, phase: 2, momentum: 0 });
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: 0, phase: 4, momentum: 0 });
  });
  assert.equal((await top()) - vorStart, 6, 'first points of a gesture already scroll');
  await page.evaluate(() => {
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: 10, phase: 1, momentum: 0 });
    for (let i = 0; i < 400; i++) window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: 40, phase: 2, momentum: 0 });
    window.__emit('noki://overview-wheel', { x: 180, y: 300, dx: 0, dy: 0, phase: 4, momentum: 0 });
  });
  assert.equal(await top(), 0, 'native trackpad gesture returns to the top');

  // Off-screen cards are not painted (incremental rendering).
  const cv = await page.evaluate(() => getComputedStyle(document.querySelector('.card')).contentVisibility);
  assert.equal(cv, 'auto');

  await browser.close();
  server.close();
  console.log(`window-overview long list: ${N} entries, wheel/trackpad top<->bottom, exact click after scroll - ok`);
})().catch(e => { console.error(e); process.exit(1); });
