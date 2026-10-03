// Global appearance contract: ordinary Noki text follows the user font/color;
// PTY, code, diffs, and raw logs stay monospace.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');

(async () => {
  const server = http.createServer((req, res) => {
    const file = path.resolve(root, '.' + new URL(req.url, 'http://noki').pathname);
    if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
    fs.readFile(file, (error, data) => {
      if (error) return res.writeHead(404).end();
      res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
      res.end(data);
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    await page.addInitScript(() => {
      const invoke = async name => name === 'intelligence_settings'
        ? { settings: { level: 'off', ask: true, mode: 'normal' }, assistant_mode: 'work', installed: false }
        : name === 'intelligence_status' ? { ask: true, loaded: false } : null;
      window.__TAURI__ = { core: { invoke }, event: { listen: async () => () => {}, emit: async () => {} },
        window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
      window.__TAURI_INTERNALS__ = { invoke };
    });
    await page.goto(`http://127.0.0.1:${server.address().port}/index.html`);
    await page.waitForFunction(() => window.NokiOberflaeche && window.NokiRaum);
    const result = await page.evaluate(() => {
      const ask = document.createElement('div');
      ask.id = 'askNoki';
      const normal = document.createElement('span');
      const terminal = document.createElement('div');
      const code = document.createElement('code');
      const diff = document.createElement('pre');
      const log = document.createElement('div');
      terminal.className = 'nc-term';
      log.className = 'nc-v-pre';
      ask.append(terminal, code, diff, log);
      document.body.append(ask, normal);
      const raumGroesse = window.NokiRaum.groesse();
      window.NokiOberflaeche.anwenden({
        flaeche: null, text: { h: 0, s: 0, v: 0.5 }, schrift: 'georgia', material: 'weniger', groesse: 'gross'
      });
      const lesen = el => getComputedStyle(el).fontFamily;
      const themed = {
        normalFont: lesen(normal),
        normalColor: getComputedStyle(normal).color,
        mono: [terminal, code, diff, log].map(lesen),
        raumGroesse: window.NokiRaum.groesse(),
      };
      window.NokiOberflaeche.anwenden(window.NokiOberflaeche.STANDARD);
      themed.resetFont = lesen(normal);
      themed.resetColor = getComputedStyle(normal).color;
      themed.resetUI = window.NokiOberflaeche.wert.groesse;
      themed.characterSizeUnchanged = window.NokiRaum.groesse() === raumGroesse;
      return themed;
    });
    assert.match(result.normalFont, /Georgia/i, `global font applied: ${result.normalFont}`);
    assert.equal(result.normalColor, 'rgb(128, 128, 128)', 'global text color applied');
    assert.ok(result.mono.every(f => /ui-monospace|monospace/i.test(f)), `code-like content remains monospace: ${result.mono.join('; ')}`);
    assert.match(result.resetFont, /^-apple-system,/i, `reset font is System: ${result.resetFont}`);
    assert.equal(result.resetColor, 'rgb(239, 235, 226)', 'reset text color is warm beige default');
    assert.equal(result.resetUI, 'standard', 'reset UI size is standard');
    assert.ok(result.characterSizeUnchanged, 'theme changes do not change Noki character appearance');
    console.log('Appearance theme: global font/color, mono exceptions, reset defaults - ok');
  } finally {
    await browser.close();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
