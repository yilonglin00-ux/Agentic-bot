const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const server = http.createServer((req, res) => {
  const file = path.resolve(root, '.' + new URL(req.url, 'http://l').pathname);
  if (!file.startsWith(root + path.sep)) return res.writeHead(403).end();
  fs.readFile(file, (err, data) => {
    if (err) return res.writeHead(404).end();
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
    res.end(data);
  });
});

(async () => {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--enable-webgl', '--ignore-gpu-blocklist'] });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    await page.addInitScript(() => {
      const invoke = async name => name === 'noki_schirm_info'
        ? { x: 0, y: 0, w: 1440, h: 900, scale: 1, ax: 0, ay: 24, aw: 1440, ah: 876 }
        : null;
      window.__TAURI__ = { core: { invoke }, event: { listen: async () => () => {}, emit: async () => {} }, window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
      window.__TAURI_INTERNALS__ = { invoke };
    });
    page.on('console', msg => { if (msg.text().includes('[DASHMESS]')) console.log(msg.text()); });
    const seconds = Math.max(60, +(process.argv[2] || 600));
    const mode = process.argv[3] || 'mixmess=1';
    await page.goto(`http://127.0.0.1:${server.address().port}/index.html#selftest=1&enmess=${seconds}&${mode}`);
    await page.waitForFunction(() => document.title === 'DASHMESS FERTIG', null, { timeout: 120000 });
  } finally {
    await browser.close(); server.close();
  }
})().catch(err => { console.error(err); server.close(); process.exit(1); });
