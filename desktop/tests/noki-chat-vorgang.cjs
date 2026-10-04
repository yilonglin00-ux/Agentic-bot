// Noki Chat: dynamische Statusphasen aus echten Router-Events, einklappbares
// „Vorgehen", kompakter Verlauf und die Modellrollen in den Einstellungen.
// Deterministische Tauri-Bruecke, kein Netz.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');

const root = path.resolve(__dirname, '..');
let failed = 0;
const check = (cond, msg) => { if (!cond) { console.error('FAIL:', msg); failed++; } else console.log('ok -', msg); };

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
    const page = await browser.newPage({ viewport: { width: 900, height: 800 } });
    await page.route('**/*', route => route.request().url().startsWith(`http://127.0.0.1:${port}/`) ? route.continue() : route.abort());
    await page.addInitScript(() => {
      window.__calls = [];
      const listeners = {};
      const settings = {
        level: 'normal', active_app: false, window_title: false, screen: false, ocr: false, shelf: false,
        patterns: false, ask: true, web: false, auto_web: false, mcp: false, active_page: false, unload_min: 10,
        memory: false, memory_auto: false, noki_folder: false, selected_text: false, mode: 'normal', notify: false, notify_preview: false
      };
      const rollen = {
        rollen: [
          { rolle: 'chat.general', titel: 'Chat · Allgemein', id: 'qwen3.5-9b', anzeige: 'Qwen3.5 9B', installiert: true, bytes: 5.6e9, quant: 'Q4_K_M', standard: true },
          { rolle: 'chat.reasoning', titel: 'Chat · Reasoning', id: 'qwen3.5-9b', anzeige: 'Qwen3.5 9B', installiert: true, bytes: 5.6e9, quant: 'Q4_K_M', standard: true },
          { rolle: 'chat.tools', titel: 'Chat · Werkzeuge', id: 'qwen3.5-9b', anzeige: 'Qwen3.5 9B', installiert: true, bytes: 5.6e9, quant: 'Q4_K_M', standard: true },
          { rolle: 'code.funktional', titel: 'Code · Funktional', id: 'jackod-9b', anzeige: 'JackOD 9B Coder', installiert: true, bytes: 5.4e9, quant: 'Q4_K_M', standard: true },
          { rolle: 'code.kreativ', titel: 'Code · Kreativ', id: 'jackod-9b', anzeige: 'JackOD 9B Coder', installiert: true, bytes: 5.4e9, quant: 'Q4_K_M', standard: true }
        ],
        presets: [
          { id: 'qwen3.5-9b', anzeige: 'Qwen3.5 9B', installiert: true },
          { id: 'jackod-9b', anzeige: 'JackOD 9B Coder', installiert: true },
          { id: 'qwen3.5-4b', anzeige: 'Qwen3.5 4B', installiert: false }
        ],
        quelle: 'standard', stand: null
      };
      let chatFertig = null;
      const invoke = async (name, args = {}) => {
        window.__calls.push({ name, args });
        if (name === 'intelligence_settings' || name === 'intelligence_status') return {
          settings, assistant_mode: 'work', installed: true, loaded: true, model: 'qwen3.5:9b', model_label: 'Qwen3.5 9B',
          memory_count: 0,
          router: {
            current_defaults: { work: 'Qwen3.5 9B', coding: 'JackOD 9B Coder' }, work_chains: {}, coding_chains: {},
            providers: [{ id: 'local_qwen', local: true, display_name: 'Qwen3.5 9B', local_quantization: 'Q4_K_M', state: 'available' }]
          }
        };
        if (name === 'noki_modell_rollen') return rollen;
        if (name === 'noki_modell_rolle_setzen') { window.__gesetzt = args; return null; }
        if (name === 'intelligence_chat') return new Promise(r => { chatFertig = r; });
        if (name === 'noki_fullscreen_status') return false;
        return null;
      };
      window.__emit = (name, payload) => (listeners[name] || []).forEach(fn => fn({ payload }));
      window.__chatFertig = r => chatFertig && chatFertig(r);
      window.__TAURI__ = {
        core: { invoke },
        event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return () => {}; }, emit: async () => {} },
        window: { getCurrentWindow: () => ({ setFocus() {}, onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) }
      };
      window.__TAURI_INTERNALS__ = { invoke };
    });

    await page.goto(`http://127.0.0.1:${port}/ask.html`);
    await page.waitForFunction(() => window.NokiAsk && window.NokiAsk.status && window.NokiAsk.status.router);
    await page.evaluate(() => window.NokiAsk.open());
    const feld = page.locator('#askNoki > form textarea');
    await feld.waitFor();
    await feld.fill('Vergleiche zwei Umzugspläne und plane die Schritte.');
    await page.locator('#askNoki .ni-send').click();
    await page.waitForFunction(() => window.__calls.some(c => c.name === 'intelligence_chat'));

    const steps = () => page.evaluate(() => [...document.querySelectorAll('#askNoki .ni-answer .ni-vorgang li')].map(li => ({ t: li.textContent, k: li.className })));
    let s = await steps();
    check(s.length === 1 && s[0].k === 'aktiv' && /denkt/.test(s[0].t), 'Start: genau ein aktiver Schritt, kein Fake-Fortschritt');

    // Echte Phasen in Reihenfolge, wie sie der Router meldet.
    await page.evaluate(() => {
      window.__emit('intelligence-research', { phase: 'analyze', n: 0 });
      window.__emit('intelligence-research', { phase: 'route', n: 0 });
      window.__emit('intelligence-research', { phase: 'wechsel', n: 1, modell: 'Qwen3.5 9B Neo' });
    });
    s = await steps();
    check(s.length === 3, 'drei gemeldete Phasen erscheinen als drei Schritte');
    check(s.slice(0, 2).every(x => x.k === 'fertig') && s[2].k === 'aktiv', 'nur die letzte Phase ist aktiv, die anderen abgehakt');
    check(/Qwen3\.5 9B Neo/.test(s[2].t), 'Modellwechsel nennt das Zielmodell');
    await page.evaluate(() => {
      window.__emit('intelligence-research', { phase: 'wechsel', n: 0, modell: 'Qwen3.5 9B Neo' });
      window.__emit('intelligence-research', { phase: 'think', n: 40 });
      window.__emit('intelligence-research', { phase: 'think', n: 120 });
    });
    s = await steps();
    check(s.filter(x => /enkt|achgedacht/.test(x.t) && !/^Noki denkt …$/.test(x.t)).length === 1, 'wiederholte Denk-Meldungen aktualisieren einen Schritt statt neue anzuhängen');
    check(s.some(x => /120/.test(x.t)), 'Denk-Schritt zeigt die gemeldete Token-Zahl');
    const status = await page.evaluate(() => (document.querySelector('#askNoki [data-k="arbeitet"]') || {}).textContent || '');
    check(/Denkt nach/.test(status), 'Statuszeile folgt der echten Phase');

    // Antwort streamt; danach endgültig gerendert.
    await page.evaluate(() => {
      window.__emit('intelligence-research', { phase: 'compose', n: 0 });
      window.__emit('intelligence-token', { token: 'Plan A ist ' });
      window.__emit('intelligence-token', { token: 'günstiger.' });
    });
    await page.waitForFunction(() => /günstiger/.test((document.querySelector('#askNoki .ni-streaming') || {}).textContent || ''));
    check(true, 'Tokens streamen weiterhin');
    await page.evaluate(() => window.__chatFertig({ text: 'Plan A ist günstiger.', route: 'LOCAL', sources: [], runtime_model: null }));
    await page.waitForFunction(() => document.querySelector('#askNoki .ni-answer details.ni-vorgehen'));

    const v = await page.evaluate(() => {
      const d = document.querySelector('#askNoki .ni-answer details.ni-vorgehen');
      const a = document.querySelector('#askNoki .ni-answer');
      return { open: d.open, summary: d.querySelector('summary').textContent, items: [...d.querySelectorAll('li')].map(li => li.textContent), text: a.textContent };
    });
    check(!v.open, '„Vorgehen" ist standardmäßig eingeklappt');
    check(/^Vorgehen · \d+ Schritte$/.test(v.summary) && Number(v.summary.match(/\d+/)[0]) === v.items.length, 'Zusammenfassung zählt die Schritte: ' + v.summary);
    check(v.items.some(t => /Qwen3\.5 9B Neo/.test(t)) && v.items.some(t => /Nachgedacht/.test(t)), 'Vorgehen enthält Modellwechsel und Denk-Zusammenfassung');
    check(/Plan A ist günstiger\./.test(v.text) && !/<\/?think>/.test(v.text), 'Antwort getrennt vom Vorgehen, kein <think>-Text');

    const turn = await page.evaluate(() => {
      const raw = Object.keys(localStorage).map(k => localStorage.getItem(k)).find(x => x && x.includes('Plan A ist günstiger.'));
      if (!raw) return null;
      const finde = o => { if (!o || typeof o !== 'object') return null; if (o.q && o.text === 'Plan A ist günstiger.') return o; for (const k in o) { const r = finde(o[k]); if (r) return r; } return null; };
      return finde(JSON.parse(raw));
    });
    check(turn && Array.isArray(turn.vorgehen) && turn.vorgehen.length <= 8 && turn.vorgehen.every(t => t.length < 80), 'Verlauf speichert das Vorgehen kompakt');
    check(turn && !JSON.stringify(turn).includes('<think>'), 'Verlauf enthält kein rohes Denken');

    // Einstellungen › Intelligence: Rollen statt Modellkarten.
    await page.evaluate(() => window.NokiAsk.settingsHTML());
    await page.waitForFunction(() => window.NokiAsk.settingsHTML().includes('ni-rollen'));
    const html = await page.evaluate(() => window.NokiAsk.settingsHTML());
    await page.evaluate(() => window.NokiAsk.action('ni-rolle', 'code.kreativ=qwen3.5-9b'));
    await page.waitForFunction(() => window.__gesetzt);
    const gesetzt = await page.evaluate(() => window.__gesetzt);
    check(gesetzt.rolle === 'code.kreativ' && gesetzt.id === 'qwen3.5-9b', 'Rollenwahl ruft noki_modell_rolle_setzen');
    await page.setContent('<div id="probe">' + html + '</div>');
    const rollenTitel = await page.$$eval('#probe .ni-rolle-titel', els => els.map(e => e.textContent));
    check(rollenTitel.join('|') === 'Chat · Allgemein|Chat · Reasoning|Chat · Werkzeuge|Code · Funktional|Code · Kreativ', 'fünf Rollen, keine Funktional/Kreativ-Chatmodi');
    const optionen = await page.$$eval('#probe select[data-ni-rolle="code.kreativ"] option', els => els.map(e => e.textContent));
    check(optionen.join('|') === 'Standard|Qwen3.5 9B|JackOD 9B Coder', 'Auswahl bietet nur installierte Modelle an');
    check(/Q4_K_M · 5,6 GB/.test(html), 'Quantisierung und Größe sichtbar');
  } finally {
    if (browser) await browser.close();
    server.close();
  }
  if (failed) { console.error(failed + ' Prüfung(en) fehlgeschlagen'); process.exit(1); }
  console.log('alle Prüfungen bestanden');
})().catch(e => { console.error(e); process.exit(1); });
