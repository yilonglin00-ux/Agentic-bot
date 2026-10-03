// Test terminal scrolling, alternate screen, and auto-follow logic
const { chromium } = require('playwright-core');
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
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const port = server.address().port;
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1000, height: 700 } });
  page.on('console', msg => console.log('PAGE:', msg.text()));

  await page.addInitScript(() => {
    window.__TAURI__ = {
      core: { invoke: async () => ({}) },
      event: { listen: async () => () => {}, emit: async () => {} },
      window: { getCurrentWindow: () => ({ setFocus: () => {} }) }
    };
  });

  await page.goto(`http://127.0.0.1:${port}/ask.html`);
  await page.waitForFunction(() => window.NokiCodeView);

  const results = await page.evaluate(async () => {
    const rootEl = document.createElement('div');
    rootEl.id = 'askNoki';
    rootEl.style.display = 'flex';
    document.body.appendChild(rootEl);
    const cv = window.NokiCodeView(rootEl, {
      call: async () => ({}),
      send: () => {},
      onStatus: () => {},
      sLaden: async () => {},
      sitzung: () => null,
      sitzungen: () => [],
      formatZeit: () => '',
      zeitRelative: () => '',
      escape: s => s
    });

    const pane = cv.createShellPane('test-pane', 'Terminal 1', '~', 80, 24);
    rootEl.appendChild(pane.paneEl);
    pane.paneEl.style.width = '800px';
    pane.paneEl.style.height = '500px';
    pane.paneEl.hidden = false;
    pane.screenEl.style.height = '480px';
    pane.fit();

    const waitFrame = () => new Promise(r => requestAnimationFrame(() => setTimeout(r, 20)));

    // TEST 1: Normal mode auto-follow and scroll up
    let lines = '';
    for (let i = 1; i <= 80; i++) {
      lines += `line ${i} output from zsh command\r\n`;
    }
    pane.feed(lines);
    await waitFrame();

    const initialScrollTop = pane.screenEl.scrollTop;
    const initialScrollHeight = pane.screenEl.scrollHeight;
    const initialClientHeight = pane.screenEl.clientHeight;
    const atBottomInitially = (initialScrollHeight - initialClientHeight - initialScrollTop) <= 28;

    // Scroll up to line 20
    pane.screenEl.scrollTop = 200;
    pane.screenEl.dispatchEvent(new Event('scroll'));
    await waitFrame();

    const scrolledPos = pane.screenEl.scrollTop;

    // Feed new output while user is scrolled up
    let newLines = '';
    for (let i = 81; i <= 100; i++) {
      newLines += `line ${i} background output\r\n`;
    }
    pane.feed(newLines);
    await waitFrame();

    const posAfterBackgroundOutput = pane.screenEl.scrollTop;
    const distFromBottomAfter = pane.screenEl.scrollHeight - pane.screenEl.clientHeight - posAfterBackgroundOutput;
    const didNotSnapToBottom = distFromBottomAfter > 100;

    // Scroll back down to bottom
    pane.screenEl.scrollTop = pane.screenEl.scrollHeight - pane.screenEl.clientHeight;
    pane.screenEl.dispatchEvent(new Event('scroll'));
    await waitFrame();

    // Feed more output after returning to bottom
    pane.feed('more output at bottom\r\n');
    await waitFrame();

    const finalScrollTop = pane.screenEl.scrollTop;
    const finalScrollHeight = pane.screenEl.scrollHeight;
    const finalClientHeight = pane.screenEl.clientHeight;
    const followedToBottomAgain = (finalScrollHeight - finalClientHeight - finalScrollTop) <= 28;

    // TEST 2: Alternate screen buffer (Anti-Gravity / agy / TUI)
    // Enter alt screen (?1049h)
    pane.feed('\x1b[?1049h');
    await waitFrame();

    const overflowYInAlt = pane.screenEl.style.overflowY;

    // Output 40 lines in alt screen that scroll off top
    let altLines = '';
    for (let i = 1; i <= 40; i++) {
      altLines += `[Anti-Gravity AGY ${i}] Processing task...\r\n`;
    }
    pane.feed(altLines);
    await waitFrame();

    const altScrollHeight = pane.screenEl.scrollHeight;
    const altClientHeight = pane.screenEl.clientHeight;
    const altScrollTop = pane.screenEl.scrollTop;
    const altAtBottom = (altScrollHeight - altClientHeight - altScrollTop) <= 28;

    // User scrolls up in alt screen to read earlier output
    pane.screenEl.scrollTop = 150;
    pane.screenEl.dispatchEvent(new Event('scroll'));
    await waitFrame();

    // Agent continues running and printing in alt screen
    let moreAgyOutput = '';
    for (let i = 41; i <= 60; i++) {
      moreAgyOutput += `[Anti-Gravity AGY ${i}] Still running background step...\r\n`;
    }
    pane.feed(moreAgyOutput);
    await waitFrame();

    const posDuringAgyRun = pane.screenEl.scrollTop;
    const distFromBottomDuringAgy = pane.screenEl.scrollHeight - pane.screenEl.clientHeight - posDuringAgyRun;
    const agyDidNotSnapToBottom = distFromBottomDuringAgy > 100;

    // Exit alt screen (?1049l)
    pane.feed('\x1b[?1049l');
    await waitFrame();

    pane.destroy();

    return {
      atBottomInitially,
      didNotSnapToBottom,
      followedToBottomAgain,
      overflowYInAlt,
      altAtBottom,
      agyDidNotSnapToBottom
    };
  });

  console.log('Test results:', JSON.stringify(results, null, 2));

  assert.ok(results.atBottomInitially, 'TEST 1: Normal mode initial output must be at bottom');
  assert.ok(results.didNotSnapToBottom, 'TEST 1: Normal mode must pause auto-follow when user scrolls up');
  assert.ok(results.followedToBottomAgain, 'TEST 1: Normal mode must resume auto-follow when user scrolls to bottom');
  assert.strictEqual(results.overflowYInAlt, 'auto', 'TEST 2: Alternate screen overflowY must be auto');
  assert.ok(results.altAtBottom, 'TEST 2: Alt screen initial output followed to bottom');
  assert.ok(results.agyDidNotSnapToBottom, 'TEST 2: Alt screen (Anti-Gravity) MUST NOT snap user to bottom while scrolled up');

  await browser.close();
  server.close();
  console.log('ALL VERIFICATION CHECKS PASSED SUCCESSFULLY!');
})().catch(err => {
  console.error('Test failed:', err);
  process.exit(1);
});
