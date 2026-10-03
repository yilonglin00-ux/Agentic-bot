// Test Work chat scrolling to latest answer on open/switch and auto-follow pause/resume
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
  const page = await browser.newPage({ viewport: { width: 980, height: 720 } });

  await page.addInitScript(() => {
    window.__TAURI__ = {
      core: {
        invoke: async (cmd) => {
          if (cmd === 'intelligence_settings') {
            return { settings: { ask: true, level: 'normal' }, status: { loaded: true } };
          }
          return {};
        }
      },
      event: { listen: async () => () => {}, emit: async () => {} },
      window: { getCurrentWindow: () => ({ setFocus: () => {} }) }
    };

    // Prepopulate 2 chats with multi-turn conversation in localStorage
    const now = Date.now();
    const chat1 = {
      id: 'chat_test_1',
      titel: 'Test Chat 1',
      created_at: now - 100000,
      updated_at: now - 50000,
      turns: []
    };
    for (let i = 1; i <= 12; i++) {
      chat1.turns.push({
        q: `User question ${i}: How do I solve step ${i}?`,
        text: `Assistant answer ${i}: Here is the detailed explanation for step ${i}.\n\nParagraph 1: Background details and analysis.\nParagraph 2: Implementation instructions.\nParagraph 3: Complete code example and test verification.\n\nDone with step ${i}.`,
        sources: [],
        code_actions: []
      });
    }

    const chat2 = {
      id: 'chat_test_2',
      titel: 'Test Chat 2',
      created_at: now - 40000,
      updated_at: now - 10000,
      turns: []
    };
    for (let i = 1; i <= 8; i++) {
      chat2.turns.push({
        q: `Chat 2 question ${i}: What is algorithm ${i}?`,
        text: `Chat 2 answer ${i}: Algorithm ${i} explanation and code walk-through.\n\nLine A\nLine B\nLine C\nLine D\nLine E\nFinished algorithm ${i}.`,
        sources: [],
        code_actions: []
      });
    }

    localStorage.setItem('noki.warp.chats.v1', JSON.stringify([chat2, chat1]));
  });

  await page.goto(`http://127.0.0.1:${port}/ask.html`);
  await page.waitForFunction(() => window.NokiAsk);
  await page.evaluate(() => {
    window.NokiAsk.open();
  });

  const results = await page.evaluate(async () => {
    const sleep = ms => new Promise(r => setTimeout(r, ms));
    const waitFrame = () => new Promise(r => requestAnimationFrame(() => setTimeout(r, 60)));

    const body = document.querySelector('#askNoki .ni-body');
    const drawer = document.querySelector('#askNoki .ni-drawer');

    // 1. Initial load of Work chat: must be at latest answer (conversation bottom)
    await waitFrame();
    await waitFrame();

    const distUntenInitial = body.scrollHeight - body.scrollTop - body.clientHeight;
    const initialAtLatest = distUntenInitial <= 40 && body.scrollTop > 500;

    // 2. Switch to chat 1 from drawer
    const verlaufBtn = document.querySelector('#askNoki .ni-verlauf-btn');
    if (verlaufBtn) verlaufBtn.click();
    await waitFrame();

    const chat1Item = document.querySelector('#askNoki .ni-bib-item[data-chat="chat_test_1"]');
    if (chat1Item) chat1Item.click();
    await waitFrame();
    await waitFrame();

    const distUntenChat1 = body.scrollHeight - body.scrollTop - body.clientHeight;
    const chat1AtLatest = distUntenChat1 <= 40 && body.scrollTop > 1000;

    // 3. User scrolls up to read earlier message (scrollTop = 200)
    body.scrollTop = 200;
    body.dispatchEvent(new Event('scroll'));
    await waitFrame();

    const posAfterUserScrollUp = body.scrollTop;

    // 4. Simulate background streaming or arrival of new response while scrolled up
    // Dist from bottom should still be large (not snapped to bottom)
    await sleep(100);
    const distAfterWait = body.scrollHeight - body.scrollTop - body.clientHeight;
    const preservedPositionWhileScrolledUp = Math.abs(body.scrollTop - 200) < 30 && distAfterWait > 500;

    // 5. User scrolls back down to bottom
    body.scrollTop = body.scrollHeight - body.clientHeight;
    body.dispatchEvent(new Event('scroll'));
    await waitFrame();

    const distUntenAfterReturn = body.scrollHeight - body.scrollTop - body.clientHeight;
    const resumedAutoFollow = distUntenAfterReturn <= 40;

    return {
      initialAtLatest,
      chat1AtLatest,
      preservedPositionWhileScrolledUp,
      resumedAutoFollow,
      initialScrollTop: body.scrollTop,
      distUntenInitial,
      distUntenChat1
    };
  });

  console.log('Work Chat Scroll Results:', JSON.stringify(results, null, 2));

  assert.ok(results.initialAtLatest, 'TEST 1: Initial Work open must be scrolled to latest answer');
  assert.ok(results.chat1AtLatest, 'TEST 2: Switching chat must scroll to latest answer');
  assert.ok(results.preservedPositionWhileScrolledUp, 'TEST 3: Scrolling up must pause auto-follow');
  assert.ok(results.resumedAutoFollow, 'TEST 4: Returning to bottom must resume auto-follow');

  await browser.close();
  server.close();
  console.log('ALL WORK CHAT TESTS PASSED SUCCESSFULLY!');
})().catch(err => {
  console.error('Work chat test failed:', err);
  process.exit(1);
});
