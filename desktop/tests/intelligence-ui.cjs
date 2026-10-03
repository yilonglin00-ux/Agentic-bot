// Ask Noki native-window contract with a deterministic Tauri bridge.
// No external network and no Playwright installation: use the existing module.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const http = require('node:http');

const root = path.resolve(__dirname, '..');

(async () => {
  const server = http.createServer((req, res) => {
    const file = path.resolve(root, '.' + new URL(req.url, 'http://localhost').pathname);
    if (!file.startsWith(root + path.sep)) { res.writeHead(403).end(); return; }
    fs.readFile(file, (error, data) => {
      if (error) { res.writeHead(404).end(); return; }
      res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
      res.end(data);
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));

  let browser;
  try {
    browser = await chromium.launch({ headless: true, channel: 'chrome' });
    const page = await browser.newPage({ viewport: { width: 1280, height: 850 } });
    await page.route('**/*', route => route.request().url().startsWith(`http://127.0.0.1:${server.address().port}/`) ? route.continue() : route.abort());
    await page.addInitScript(() => {
      window.__calls = [];
      let settings = JSON.parse(localStorage.getItem('ni-settings') || 'null') || {
        level: 'off', active_app: false, window_title: false, screen: false,
        ocr: false, shelf: false, patterns: false, ask: true, web: false,
        auto_web: false, mcp: false, active_page: false, unload_min: 10,
        memory: false, memory_auto: false, noki_folder: false,
        selected_text: false, mode: 'normal', notify: true, notify_preview: false
      };
      let nativeAskOpen = false;
      const listeners = {};
      const invoke = async (name, args = {}) => {
        window.__calls.push({ name, args });
        if (name === 'intelligence_settings') return {
          settings, assistant_mode: 'work', installed: true, loaded: false,
          model: 'qwen3.5:9b', model_label: 'Qwen3.5 9B', memory_count: 0
        };
        if (name === 'intelligence_status') return { ask: settings.ask, loaded: false };
        if (name === 'intelligence_save') {
          settings = args.settings;
          localStorage.setItem('ni-settings', JSON.stringify(settings));
          if (!settings.ask) nativeAskOpen = false;
          return settings;
        }
        if (name === 'ask_fenster_zeigen') {
          if (!settings.ask) return false;
          nativeAskOpen = true;
          return true;
        }
        if (name === 'ask_fenster_verbergen') { nativeAskOpen = false; return null; }
        if (name === 'platz_liste') return { profile: [], ax: true };
        if (name === 'fokus_apps' || name === 'ablage_liste' || name === 'clip_liste' || name === 'noki_ordner_liste') return [];
        return null;
      };
      window.__nativeAskOpen = () => nativeAskOpen;
      window.__persistedAsk = () => JSON.parse(localStorage.getItem('ni-settings') || '{}').ask;
      window.__listeners = listeners;
      window.__TAURI__ = {
        core: { invoke },
        event: {
          listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return () => {}; },
          emit: async () => {}
        },
        window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) }
      };
      window.__TAURI_INTERNALS__ = { invoke };
    });

    const url = `http://127.0.0.1:${server.address().port}/index.html`;
    await page.goto(url);
    await page.waitForFunction(() => window.NokiAsk && window.NokiAsk.status);

    // The character page owns no embedded Ask DOM; opening delegates to the native command.
    assert.equal(await page.locator('#askNoki').count(), 0, 'Ask is not embedded in index.html');
    await page.evaluate(() => window.NokiAsk.open());
    await page.waitForFunction(() => window.__nativeAskOpen());
    assert(await page.evaluate(() => window.__calls.some(call => call.name === 'ask_fenster_zeigen')));
    assert.equal(await page.evaluate(() => window.__calls.filter(call => call.name === 'intelligence_prewarm').length), 0, 'native open stays lazy');

    // OFF closes the native window, persists, and blocks both shortcut and direct native open.
    await page.evaluate(() => window.NokiEinstellungen.auf('intelligence'));
    await page.locator('[data-ni="ask"]').uncheck();
    await page.waitForFunction(() => window.NokiAsk.settings.ask === false);
    assert.equal(await page.evaluate(() => window.__nativeAskOpen()), false);
    assert.equal(await page.evaluate(() => window.__persistedAsk()), false);
    const opensWhileOff = await page.evaluate(() => window.__calls.filter(call => call.name === 'ask_fenster_zeigen').length);
    await page.evaluate(() => window.NokiAsk.shortcut());
    assert.equal(await page.evaluate(() => window.__calls.filter(call => call.name === 'ask_fenster_zeigen').length), opensWhileOff, 'OFF shortcut does not request native Ask');
    assert.equal(await page.evaluate(() => window.__TAURI__.core.invoke('ask_fenster_zeigen')), false, 'OFF native command is gated');

    await page.reload();
    await page.waitForFunction(() => window.NokiAsk && window.NokiAsk.status);
    assert.equal(await page.evaluate(() => window.NokiAsk.settings.ask), false, 'OFF persists across restart/reload');

    // ON persists and permits the native window again without prewarming.
    await page.evaluate(() => window.NokiEinstellungen.auf('intelligence'));
    const prewarmBefore = await page.evaluate(() => window.__calls.filter(call => call.name === 'intelligence_prewarm').length);
    await page.locator('[data-ni="ask"]').check();
    await page.waitForFunction(() => window.NokiAsk.settings.ask === true);
    assert.equal(await page.evaluate(() => window.__persistedAsk()), true);
    assert.equal(await page.evaluate(() => window.__calls.filter(call => call.name === 'intelligence_prewarm').length), prewarmBefore, 'ON does not preload');
    assert.equal(await page.evaluate(() => window.__TAURI__.core.invoke('ask_fenster_zeigen')), true, 'ON native command can open Ask');
    assert.equal(await page.evaluate(() => window.__nativeAskOpen()), true);

    await page.reload();
    await page.waitForFunction(() => window.NokiAsk && window.NokiAsk.status);
    assert.equal(await page.evaluate(() => window.NokiAsk.settings.ask), true, 'ON persists across restart/reload');
    console.log('PASS: native Ask ON/OFF gating, lazy open, and persistence');
  } finally {
    if (browser) await browser.close();
    server.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
