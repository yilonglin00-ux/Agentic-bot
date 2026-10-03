// Shortcut 9 + Safari tabs: the card shows exactly the CURRENTLY open tabs.
// 3 tabs -> one closed -> 2 pages (inactive-but-open tab stays) -> down to 1
// tab -> no paged view left over (closed tabs must never remain visible).
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');

const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + new URL(req.url, 'http://noki').pathname);
  if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
  fs.readFile(file, (e, d) => { if (e) return res.writeHead(404).end(); res.setHeader('Content-Type', 'text/html'); res.end(d); });
});

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage({ viewport: { width: 360, height: 640 } });
  await page.addInitScript(() => {
    const tab = (k, t, a) => ({ key: 'safari:' + k + ':0', title: t, url: '', active: a, preview: '' });
    window.__tabs = [tab('a', 'NokiTab-A', true), tab('b', 'NokiTab-B', false), tab('c', 'NokiTab-C', false)];
    const w = { id: 7, pid: 70, app: 'Safari', title: 'NokiTab-A', preview: '', icon: '', minimized: false, chrome: true, stand_ms: 0, auf_space: true, drosselt: false };
    window.__TAURI__ = {
      core: { invoke: async (cmd) => {
        switch (cmd) {
          case 'window_overview_state': return true;
          case 'window_overview_geometry': return { card_w: 300, card_h: 150 };
          case 'window_overview_list': return [w];
          case 'window_overview_tabs': return window.__tabs;
          case 'window_overview_frames': return [];
          default: return null;
        }
      } },
      event: { listen: async () => () => {} },
    };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/window-overview.html`);
  const seiten = () => page.evaluate(() => [...document.querySelectorAll('.card .page')].map(p => p.textContent.trim()));
  await page.waitForFunction(() => document.querySelectorAll('.card .page').length === 3);
  assert.deepEqual(await seiten(), ['NokiTab-A', 'NokiTab-B', 'NokiTab-C']);

  // Tab B closed, C active, A inactive but still open.
  await page.evaluate(() => { const t = window.__tabs; window.__tabs = [{ ...t[0], active: false }, { ...t[2], active: true }]; });
  await page.waitForFunction(() => document.querySelectorAll('.card .page').length === 2, null, { timeout: 8000 });
  assert.deepEqual(await seiten(), ['NokiTab-A', 'NokiTab-C'], 'closed tab gone, inactive open tab kept');

  // Down to ONE real tab: the paged view (with its closed tabs) must go.
  await page.evaluate(() => { window.__tabs = [window.__tabs[1]]; });
  await page.waitForFunction(() => document.querySelectorAll('.card .page').length === 0
    && document.querySelectorAll('.card').length === 1, null, { timeout: 8000 });
  assert.equal(await page.evaluate(() => document.querySelectorAll('.tabzahl').length), 0);

  await browser.close();
  server.close();
  console.log('window-overview safari tabs: 3 -> 2 (inactive kept) -> 1 (no stale pages) - ok');
})().catch(e => { console.error(e); process.exit(1); });
