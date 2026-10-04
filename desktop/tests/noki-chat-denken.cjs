// Noki Chat „Denken“: Schalter im Eingabebereich (AN/AUS, gespeichert),
// Denken-Bereich live und eingeklappt, nie rohes <think>, Streaming,
// kein haengender Status nach einem Fehler. Deterministische Tauri-Bruecke.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');

const root = path.resolve(__dirname, '..');
let failed = 0;
const check = (cond, msg) => { if (!cond) { console.error('FAIL:', msg); failed++; } else console.log('ok -', msg); };

const bruecke = () => {
  window.__calls = [];
  const listeners = {};
  const gespeichert = JSON.parse(localStorage.getItem('__settings') || 'null');
  const settings = gespeichert || {
    level: 'normal', active_app: false, window_title: false, screen: false, ocr: false, shelf: false,
    patterns: false, ask: true, web: false, auto_web: false, mcp: false, active_page: false, unload_min: 10,
    memory: false, memory_auto: false, noki_folder: false, selected_text: false, mode: 'normal', denken: false,
    notify: false, notify_preview: false
  };
  let chat = null;
  const invoke = async (name, args = {}) => {
    window.__calls.push({ name, args: JSON.parse(JSON.stringify(args)) });
    if (name === 'intelligence_settings' || name === 'intelligence_status') return {
      settings: JSON.parse(JSON.stringify(settings)), assistant_mode: 'work', installed: true, loaded: true,
      model: 'qwen3.5:9b', model_label: 'Qwen3.5 9B', memory_count: 0
    };
    if (name === 'intelligence_denken') {
      settings.denken = !!args.an;
      localStorage.setItem('__settings', JSON.stringify(settings));
      return settings.denken;
    }
    if (name === 'intelligence_chat') return new Promise((ok, fehler) => { chat = { ok, fehler }; });
    if (name === 'noki_fullscreen_status') return false;
    return null;
  };
  window.__emit = (name, payload) => (listeners[name] || []).forEach(fn => fn({ payload }));
  window.__fertig = r => chat && chat.ok(r);
  window.__fehler = e => chat && chat.fehler(e);
  window.__TAURI__ = {
    core: { invoke },
    event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return () => {}; }, emit: async () => {} },
    window: { getCurrentWindow: () => ({ setFocus() {}, onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) }
  };
  window.__TAURI_INTERNALS__ = { invoke };
};

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
  const port = server.address().port;
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 900, height: 800 } });
    await context.route('**/*', route => route.request().url().startsWith(`http://127.0.0.1:${port}/`) ? route.continue() : route.abort());
    await context.addInitScript(bruecke);
    let page = await context.newPage();
    const oeffnen = async () => {
      await page.goto(`http://127.0.0.1:${port}/ask.html`);
      await page.waitForFunction(() => window.NokiAsk && window.NokiAsk.status);
      await page.evaluate(() => window.NokiAsk.open());
      await page.locator('#askNoki .ni-denken').waitFor();
    };
    const fragen = async text => {
      const n = await page.evaluate(() => window.__calls.filter(c => c.name === 'intelligence_chat').length + 1);
      await page.locator('#askNoki > form textarea').fill(text);
      await page.locator('#askNoki .ni-send').click();
      await page.waitForFunction(n => window.__calls.filter(c => c.name === 'intelligence_chat').length === n, n);
    };
    const box = () => page.evaluate(() => {
      const d = document.querySelector('#askNoki .ni-answer details.ni-denken-box');
      return d ? { open: d.open, laeuft: d.classList.contains('laeuft'), titel: d.querySelector('.ni-denken-titel').textContent,
        items: [...d.querySelectorAll('li')].map(li => ({ t: li.textContent, k: li.className })) } : null;
    });
    const antwortText = () => page.evaluate(() => document.querySelector('#askNoki .ni-answer').textContent);
    const letzterTurn = () => page.evaluate(() => {
      for (const k of Object.keys(localStorage)) {
        const raw = localStorage.getItem(k); if (!raw || k === '__settings') continue;
        try {
          const finde = o => { if (!o || typeof o !== 'object') return null; const l = Array.isArray(o.messages) ? o.messages : o.turns; if (Array.isArray(l) && l.length && l[0].q) return l[l.length - 1]; for (const x in o) { const r = finde(o[x]); if (r) return r; } return null; };
          const t = finde(JSON.parse(raw)); if (t) return t;
        } catch (e) {}
      }
      return null;
    });

    await oeffnen();
    // 1. Denken AUS -> normale Antwort, kein Denken-Bereich
    check(await page.getAttribute('#askNoki .ni-denken', 'aria-pressed') === 'false', 'Start: Denken AUS');
    check(await page.isVisible('#askNoki .ni-denken'), 'Denken-Schalter sitzt im Eingabebereich');
    await fragen('Wie spät ist es in Tokio?');
    await page.evaluate(() => { window.__emit('intelligence-research', { phase: 'analyze', n: 0 }); window.__emit('intelligence-research', { phase: 'compose', n: 0 }); });
    check(!(await box()), 'AUS: kein Denken-Bereich während der Antwort');
    await page.evaluate(() => { window.__emit('intelligence-token', { token: 'Dort ist es ' }); window.__emit('intelligence-token', { token: '21 Uhr.' }); });
    await page.waitForFunction(() => /21 Uhr/.test((document.querySelector('#askNoki .ni-streaming') || {}).textContent || ''));
    await page.evaluate(() => window.__fertig({ text: 'Dort ist es 21 Uhr.', route: 'LOCAL', sources: [] }));
    await page.waitForFunction(() => !window.NokiAsk.status || !document.querySelector('#askNoki .ni-streaming'));
    check(!(await box()) && /21 Uhr/.test(await antwortText()), 'AUS: Antwort ohne Denken-Bereich');
    let turn = await letzterTurn();
    check(turn && turn.denken === false, 'AUS: im Verlauf als Denken AUS gespeichert');

    // 2. Schalter AN: echt gespeichert
    await page.click('#askNoki .ni-denken');
    await page.waitForFunction(() => window.__calls.some(c => c.name === 'intelligence_denken'));
    check(await page.getAttribute('#askNoki .ni-denken', 'aria-pressed') === 'true', 'Klick: Denken AN sichtbar (aria-pressed)');
    check(await page.evaluate(() => window.__calls.filter(c => c.name === 'intelligence_denken').pop().args.an === true), 'AN wird an die native Seite übergeben');

    // 3. Denken AN -> Denken-Bereich live, offen, echte Schritte
    await fragen('Vergleiche zwei Umzugspläne und plane die Schritte.');
    let b = await box();
    check(b && b.open && b.laeuft && b.titel === 'Denken …', 'AN: Denken-Bereich erscheint offen mit „Denken …“');
    await page.evaluate(() => {
      window.__emit('intelligence-research', { phase: 'analyze', n: 0 });
      window.__emit('intelligence-research', { phase: 'memory', n: 2 });
      window.__emit('intelligence-research', { phase: 'route', n: 0 });
      window.__emit('intelligence-research', { phase: 'think', n: 0 });
    });
    b = await box();
    check(b.items.map(i => i.t).join('|') === 'Anfrage analysiert|Vorgehen geplant|Noki denkt …', 'Denken zeigt nur Denkschritte aus echten Ereignissen: ' + b.items.map(i => i.t).join('|'));
    check(b.items[2].k === 'aktiv' && b.items[0].k === 'fertig', 'aktueller Schritt markiert, frühere abgehakt');
    const aktionen = await page.$$eval('#askNoki .ni-answer > ul.ni-vorgang li', els => els.map(e => e.textContent));
    check(aktionen.length === 1 && /Gedächtnis/.test(aktionen[0]), 'Vorgehen (echte Aktion) getrennt vom Denken');
    check(!/≈/.test(b.items[2].t), 'ohne gemessene Tokens keine Token-Zahl');
    await page.evaluate(() => window.__emit('intelligence-research', { phase: 'think', n: 824 }));
    b = await box();
    check(/≈ 824 Tokens/.test(b.items[b.items.length - 1].t), 'gemessene Denk-Tokens werden angezeigt');

    // 4. Antwort streamt: Bereich bleibt, klappt zusammen, kein <think>
    await page.evaluate(() => {
      window.__emit('intelligence-research', { phase: 'compose', n: 0 });
      window.__emit('intelligence-token', { token: '<think>interne Gedanken</think>' });
      window.__emit('intelligence-token', { token: 'Plan A ist ' });
      window.__emit('intelligence-token', { token: 'günstiger. <thi' });
    });
    await page.waitForFunction(() => /günstiger/.test((document.querySelector('#askNoki .ni-streaming') || {}).textContent || ''));
    b = await box();
    check(b && !b.open && !b.laeuft && /^Denken · 4 Schritte · ≈ 824 Tokens$/.test(b.titel), 'beim Streamen kompakt: ' + (b && b.titel));
    const live = await page.evaluate(() => document.querySelector('#askNoki .ni-streaming').textContent);
    check(!/think|interne Gedanken|<thi/.test(live), 'Streaming zeigt nie Denk-Tags oder Denktext');
    const reihenfolge = await page.evaluate(() => [...document.querySelector('#askNoki .ni-answer').children].map(e => e.className.split(' ')[0]));
    check(reihenfolge.indexOf('ni-denken-box') < reihenfolge.indexOf('ni-streaming'), 'Denken steht über der Antwort');
    await page.evaluate(() => window.__emit('intelligence-research', { phase: 'verify', n: 0 }));
    check((await box()).titel.startsWith('Denken · 5 Schritte'), 'späte Prüfung aktualisiert den Bereich ohne Neuaufbau');

    // 5./6. Abschluss: eingeklappt, Klick öffnet
    await page.evaluate(() => window.__fertig({ text: '<think>x</think>Plan A ist günstiger.', route: 'LOCAL', sources: [] }));
    await page.waitForFunction(() => !document.querySelector('#askNoki .ni-streaming') && document.querySelector('#askNoki details.ni-denken-box'));
    b = await box();
    check(b && !b.open && /^Denken · 5 Schritte · ≈ 824 Tokens$/.test(b.titel), 'nach Abschluss eingeklappt: ' + (b && b.titel));
    check(!/<\/?think>|interne Gedanken/.test(await antwortText()) && /Plan A ist günstiger\./.test(await antwortText()), 'finale Antwort getrennt, ohne Denk-Tags');
    await page.click('#askNoki details.ni-denken-box summary');
    b = await box();
    check(b.open && b.items.length === 5 && b.items.every(i => i.k === 'fertig'), 'Klick öffnet den Denken-Bereich wieder');
    turn = await letzterTurn();
    check(turn.denken === true && turn.denken_schritte.length === 5 && turn.denken_tokens === 824 && turn.text === 'Plan A ist günstiger.', 'Verlauf: Denken kompakt gespeichert');
    check(!JSON.stringify(turn).includes('interne Gedanken'), 'Verlauf enthält keinen Denktext');
    check(turn.vorgehen.length === 1 && /Gedächtnis/.test(turn.vorgehen[0]), 'Verlauf: Vorgehen nur mit echten Aktionen');

    // 9. Fehler: kein hängender Denken-Status
    await fragen('Rechne das bitte nochmal durch.');
    await page.evaluate(() => window.__emit('intelligence-research', { phase: 'think', n: 12 }));
    await page.evaluate(() => window.__fehler('Lokales Modell antwortet nicht.'));
    await page.waitForFunction(() => /antwortet nicht/.test(document.querySelector('#askNoki .ni-answer').textContent));
    const nachFehler = await page.evaluate(() => ({ laeuft: !!document.querySelector('#askNoki .ni-denken-box.laeuft'), aktiv: !!document.querySelector('#askNoki .ni-answer li.aktiv'), text: document.querySelector('#askNoki .ni-answer').textContent }));
    check(!nachFehler.laeuft && !nachFehler.aktiv && !/Noki denkt|Denken …/.test(nachFehler.text), 'Fehler: kein hängendes „Noki denkt …“');
    check(await page.evaluate(() => !document.querySelector('#askNoki .ni-send').disabled), 'Fehler: Eingabe wieder frei');

    // 7. Zustand nach Schließen/Öffnen (neues Fenster, gespeicherte Einstellungen)
    await page.close();
    page = await context.newPage();
    await oeffnen();
    check(await page.getAttribute('#askNoki .ni-denken', 'aria-pressed') === 'true', 'nach erneutem Öffnen: Denken AN wiederhergestellt');
    await page.click('#askNoki .ni-denken');
    await page.waitForFunction(() => document.querySelector('#askNoki .ni-denken').getAttribute('aria-pressed') === 'false');
    check(await page.evaluate(() => JSON.parse(localStorage.getItem('__settings')).denken === false), 'AUS wird ebenfalls gespeichert');

    // 11. Noki Code: kein Denken-Schalter
    await page.click('#askNoki [data-assistant="code"]');
    await page.waitForTimeout(200);
    check(!(await page.isVisible('#askNoki .ni-denken')), 'Noki Code zeigt keinen Denken-Schalter');
  } finally {
    if (browser) await browser.close();
    server.close();
  }
  if (failed) { console.error(failed + ' Prüfung(en) fehlgeschlagen'); process.exit(1); }
  console.log('alle Prüfungen bestanden');
})().catch(e => { console.error(e); process.exit(1); });
