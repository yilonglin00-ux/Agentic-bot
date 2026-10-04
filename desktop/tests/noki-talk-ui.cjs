// Noki Talk UI, end to end in the browser with the real Settings mirror
// (settings.html <-> index.html): listening panel (state, live level,
// preview, close), Settings › Noki Talk (status, language, auto-insert),
// history (open, full text, play a real AAC recording, copy, delete) and
// the error path (missing microphone permission opens the right page).
// The native side (noki_talk_*) is mocked with the same command contract.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
const fs = require('node:fs');
const os = require('node:os');
const http = require('node:http');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');

let fehler = 0;
const check = (ok, msg) => { console.log((ok ? 'ok   ' : 'FAIL ') + msg); if (!ok) fehler++; };

// A real local recording (as NokiTalk.app writes it): AAC, 16 kHz mono,
// 32 kbit/s. Open-source Chromium has no AAC decoder (WebKit on macOS has);
// there the same player is exercised with the WAV of the same signal.
function aufnahme(aac) {
  try {
    const f = path.join(os.tmpdir(), `noki-talk-${process.pid}.${aac ? 'm4a' : 'wav'}`);
    execFileSync('ffmpeg', ['-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'sine=frequency=330:duration=3', '-ac', '1', '-ar', '16000',
      ...(aac ? ['-c:a', 'aac', '-b:a', '32k'] : ['-c:a', 'pcm_s16le']), f]);
    const url = `data:audio/${aac ? 'mp4' : 'wav'};base64,` + fs.readFileSync(f).toString('base64');
    fs.unlinkSync(f);
    return url;
  } catch (e) { console.log('(ffmpeg fehlt: Wiedergabe-Test entfaellt)'); return null; }
}
let audioUrl = null;

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
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--autoplay-policy=no-user-gesture-required'] });
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const main = await ctx.newPage();
  const einst = await ctx.newPage();
  await main.exposeBinding('__nachEinst', (_, name, payload) => einst.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [name, payload]).catch(() => {}));
  await einst.exposeBinding('__nachMain', (_, name, payload) => main.evaluate(([n, p]) => window.__emit && window.__emit(n, p), [name, payload]).catch(() => {}));
  const mock = ([rolle, audio]) => {
    window.__calls = []; window.__nokiEvents = {};
    window.__emit = (name, payload) => (window.__nokiEvents[name] || []).forEach(fn => fn({ payload }));
    const listen = async (name, fn) => { (window.__nokiEvents[name] = window.__nokiEvents[name] || []).push(fn); return () => {}; };
    const jetzt = Date.now();
    window.__talk = {
      status: { server: 'aus', programm: '/Users/x/NOKI/.local/whisper-src/build/bin/whisper-server', modell: 'ggml-large-v3-turbo-q5_0.bin',
        modelle: ['ggml-large-v3-turbo-q5_0.bin', 'ggml-small.bin'], modell_dir: '/Users/x/NOKI/.local/whisper-models',
        helfer: true, mikrofon: 3, geraet: 'MacBook Pro-Mikrofon', sprache: 'auto', auto_einfuegen: true, laeuft: false },
      liste: Array.from({ length: 30 }, (_, i) => ({ id: 100 - i, created_at: jetzt - i * 3600e3, duration_ms: 18000 + i * 1000,
        transcript: (i === 0 ? 'Ich wollte morgen eigentlich noch zur Uni fahren und danach in die Bibliothek, um die Notizen für die Prüfung fertig zu schreiben.' : 'Diktat Nummer ' + i),
        audio_path: '/x/' + i + '.m4a', language: 'de', source_app: 'Notes' })),
    };
    const invoke = async (name, args) => {
      window.__calls.push({ name, args });
      const T = window.__talk;
      if (name === 'noki_talk_status') return JSON.parse(JSON.stringify(T.status));
      if (name === 'noki_talk_verlauf') return T.liste.slice();
      if (name === 'noki_talk_audio') return audio;
      if (name === 'noki_talk_kopieren') return true;
      if (name === 'noki_talk_loeschen') { T.liste = T.liste.filter(e => e.id !== args.id); return true; }
      if (name === 'noki_talk_einstellung') { T.status[args.schluessel] = args.wert; return null; }
      if (name === 'noki_talk_engine_starten') { T.status.server = 'bereit'; return null; }
      if (name === 'fokus_sitzung_status') return { aktiv: false };
      if (name === 'intelligence_settings') return { settings: { level: 'off', ask: true, web: false }, installed: true, loaded: false };
      return null;
    };
    const emitTo = async (ziel, name, payload) => {
      if (rolle === 'main' && ziel === 'einstellungen') return window.__nachEinst(name, payload);
      if (rolle === 'einst' && ziel === 'main') return window.__nachMain(name, payload);
    };
    window.__TAURI__ = { core: { invoke }, event: { listen, emit: async () => {}, emitTo },
      window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
    window.__TAURI_INTERNALS__ = { invoke };
  };
  {
    const probe = await browser.newPage();
    const aac = await probe.evaluate(() => new Audio().canPlayType('audio/mp4; codecs="mp4a.40.2"') !== '');
    await probe.close();
    audioUrl = aufnahme(aac);
    console.log(`  Wiedergabe-Test mit ${aac ? 'AAC (m4a)' : 'WAV (kein AAC-Decoder in diesem Chromium)'}`);
  }
  await main.addInitScript(mock, ['main', audioUrl]);
  await einst.addInitScript(mock, ['einst', audioUrl]);
  main.on('pageerror', e => { console.log('main pageerror:', e.message); fehler++; });
  einst.on('pageerror', e => { console.log('einst pageerror:', e.message); fehler++; });
  await main.goto(`http://127.0.0.1:${port}/index.html`, { timeout: 180000 });
  await main.waitForFunction(() => window.NokiEinstellungen && window.NokiWerk, null, { timeout: 120000 });
  await main.waitForTimeout(1600);
  await einst.goto(`http://127.0.0.1:${port}/settings.html`);
  await einst.waitForTimeout(300);
  const calls = (n) => main.evaluate(n => window.__calls.filter(c => c.name === n), n);
  const ev = (n, p) => main.evaluate(([n, p]) => window.__emit(n, p), [n, p]);
  const tafel = () => main.evaluate(() => ({
    zustand: document.documentElement.getAttribute('data-noki-stimme'),
    label: document.getElementById('nokiStimmeLabel').textContent,
    text: document.getElementById('nokiStimmeText').textContent,
    eq: getComputedStyle(document.getElementById('nokiStimmeEq')).display,
    bars: [...document.querySelectorAll('#nokiStimmeEq i')].map(b => +(/scaleY\(([\d.]+)\)/.exec(b.style.transform) || [0, 0])[1]),
  }));

  // ---- 1. Listening panel ----------------------------------------------
  await ev('noki://stimme', { was: 'start' });
  let t = await tafel();
  check(t.zustand === '1' && t.label === 'Noki hört zu' && t.eq !== 'none', `Start: "Noki hört zu" mit Pegel (${t.zustand}, ${t.label}, eq=${t.eq})`);
  check((await calls('intelligence_voice_start')).length === 0, 'kein Ask-Noki-Sprachweg (Diktat ist kein Chat)');
  for (const l of [0.2, 0.5, 0.9, 0.7]) await ev('noki-talk', { pegel: l });
  t = await tafel();
  check(t.bars[t.bars.length - 1] > 0.6 && t.bars[t.bars.length - 2] > 0.8 && t.bars[0] < 0.2, `Equalizer folgt dem Pegel (${t.bars.slice(-4).join(', ')})`);
  await ev('noki-talk', { zustand: 'hoert', text: 'Ich wollte morgen eigentlich noch zur Uni …' });
  t = await tafel();
  check(t.text === 'Ich wollte morgen eigentlich noch zur Uni …', 'Live-Vorschau erscheint');
  await ev('noki://stimme', { was: 'stop' });
  t = await tafel();
  check(t.label === 'Noki schreibt …' && t.zustand === 'arbeitet', `Stopp: "${t.label}"`);
  await ev('noki-talk', { zustand: 'fertig', text: 'Ich wollte morgen eigentlich noch zur Uni.', meldung: '', verlauf: true });
  t = await tafel();
  check(t.label === 'Eingefügt' && t.text === 'Ich wollte morgen eigentlich noch zur Uni.', `Fertig: ${t.label}`);
  await main.waitForFunction(() => document.documentElement.getAttribute('data-noki-stimme') === '0', null, { timeout: 15000 }).catch(() => {});
  check((await tafel()).zustand === '0', 'Tafel schliesst danach');
  // fast repeated dictations + "nothing understood" + cancel
  for (let i = 0; i < 5; i++) {
    await ev('noki://stimme', { was: 'start' });
    await ev('noki-talk', { zustand: i % 2 ? 'leer' : 'aus', meldung: i % 2 ? 'Nichts verstanden' : undefined });
  }
  await main.waitForFunction(() => document.documentElement.getAttribute('data-noki-stimme') === '0', null, { timeout: 15000 }).catch(() => {});
  check((await tafel()).zustand === '0', '5 schnelle Diktate/Abbrueche: Tafel bleibt nicht haengen');

  // ---- 2. Settings › Noki Talk -----------------------------------------
  await main.evaluate(() => window.NokiEinstellungen.auf('talk'));
  await einst.waitForSelector('#einstellungen [data-e="talk_auf"]', { timeout: 20000 });
  const navTalk = await einst.evaluate(() => [...document.querySelectorAll('#einstellungen .e-nav-btn')].map(b => b.textContent));
  check(navTalk.includes('Noki Talk') && navTalk.indexOf('Noki Talk') === navTalk.indexOf('Intelligence') + 1, 'eigener Hauptpunkt "Noki Talk" neben Intelligence');
  const seite = await einst.evaluate(() => document.querySelector('#einstellungen .e-inhalt').textContent);
  for (const s of ['Lokale Engine', 'Mikrofon', 'MacBook Pro-Mikrofon', 'Modell', 'ggml-large-v3-turbo-q5_0.bin', 'Kurzbefehl', 'Sprache', 'Automatisch einfügen', 'Verlauf'])
    check(seite.includes(s), `Einstellung zeigt "${s}"`);
  const klick = async (sel, txt) => {
    const b = txt ? einst.locator(`#einstellungen ${sel}`, { hasText: txt }) : einst.locator(`#einstellungen ${sel}`).first();
    await b.click({ timeout: 10000 });
    await main.waitForTimeout(200);
  };
  await klick('.e-seg-btn[data-e="talk_sprache"]', 'Deutsch');
  let e = (await calls('noki_talk_einstellung')).pop();
  check(e && e.args.schluessel === 'sprache' && e.args.wert === 'de', 'Sprache Deutsch wird gespeichert');
  await klick('.e-seg-btn[data-e="talk_auto"]', 'Aus');
  e = (await calls('noki_talk_einstellung')).pop();
  check(e && e.args.schluessel === 'auto_einfuegen' && e.args.wert === false, 'Auto-Einfügen aus wird gespeichert');
  await klick('.e-seg-btn[data-e="talk_modell"]', 'small');
  e = (await calls('noki_talk_einstellung')).pop();
  check(e && e.args.schluessel === 'modell' && e.args.wert === 'ggml-small.bin', 'Modellwahl wird gespeichert');
  await klick('[data-e="talk_engine"]');
  check((await calls('noki_talk_engine_starten')).length === 1, 'Engine jetzt laden');

  // ---- 3. History --------------------------------------------------------
  let zeilen = await einst.evaluate(() => document.querySelectorAll('#einstellungen button.t-e').length);
  check(zeilen === 30, `Verlauf: 30 Eintraege (${zeilen})`);
  const kopf = await einst.evaluate(() => document.querySelector('#einstellungen .t-zeit').textContent);
  check(/^Heute · \d\d:\d\d · 00:18$/.test(kopf), `Kopfzeile "Heute · hh:mm · 00:18" (${kopf})`);
  await klick('button.t-e');
  const voll = await einst.evaluate(() => (document.querySelector('#einstellungen .t-voll') || {}).textContent || '');
  check(voll.startsWith('Ich wollte morgen') && voll.endsWith('fertig zu schreiben.'), 'Eintrag oeffnet den vollstaendigen Text');
  await klick('[data-e="talk_kopie"]');
  const k = (await calls('noki_talk_kopieren')).pop();
  check(k && k.args.id === 100, 'Kopieren (nur Text) fuer genau diesen Eintrag');
  if (audioUrl) {
    await klick('[data-e="talk_play"]');
    await einst.waitForFunction(() => /Pause/.test((document.querySelector('#einstellungen [data-e="talk_play"]') || {}).textContent || ''), null, { timeout: 15000 }).catch(() => {});
    await einst.waitForFunction(() => /^00:0[1-3] \//.test((document.getElementById('talkUhr') || {}).textContent || ''), null, { timeout: 30000 }).catch(() => {});
    const p = await einst.evaluate(() => ({ uhr: (document.getElementById('talkUhr') || {}).textContent, breite: (document.getElementById('talkBalken') || {}).style.width, knopf: document.querySelector('#einstellungen [data-e="talk_play"]').textContent }));
    check(p.knopf === 'Pause' && /^00:0[1-3] \/ 00:0[23]$/.test(p.uhr) && parseInt(p.breite) > 0, `Abspielen der lokalen Aufnahme mit Fortschritt (${p.uhr}, ${p.breite})`);
    await klick('[data-e="talk_play"]');
    await einst.waitForFunction(() => (document.querySelector('#einstellungen [data-e="talk_play"]') || {}).textContent === 'Abspielen', null, { timeout: 5000 }).catch(() => {});
    const st = await einst.evaluate(() => document.querySelector('#einstellungen [data-e="talk_play"]').textContent);
    if (st !== 'Abspielen') console.log('  debug main:', await main.evaluate(() => (document.querySelector('#einstellungen [data-e="talk_play"]') || {}).textContent));
    check(st === 'Abspielen', 'Pause/Stopp');
  }
  await klick('[data-e="talk_weg"]');
  await einst.waitForFunction(() => document.querySelectorAll('#einstellungen button.t-e').length === 29, null, { timeout: 10000 }).catch(() => {});
  const l = (await calls('noki_talk_loeschen')).pop();
  zeilen = await einst.evaluate(() => document.querySelectorAll('#einstellungen button.t-e').length);
  check(l && l.args.id === 100 && zeilen === 29, `Loeschen entfernt den Eintrag (${zeilen})`);

  // ---- 4. Dictation while Settings is open -------------------------------
  await ev('noki://stimme', { was: 'start' });
  await klick('.e-seg-btn[data-e="talk_sprache"]', 'Englisch');
  e = (await calls('noki_talk_einstellung')).pop();
  check(e && e.args.wert === 'en' && (await tafel()).zustand === '1', 'Einstellungen bleiben bedienbar waehrend der Aufnahme');
  const vorher = (await calls('noki_talk_verlauf')).length;
  await ev('noki-talk', { zustand: 'fertig', text: 'Hello.', meldung: 'Text kopiert', verlauf: true });
  await main.waitForTimeout(400);
  check((await calls('noki_talk_verlauf')).length > vorher, 'neues Diktat aktualisiert den offenen Verlauf');
  check((await tafel()).label === 'Text kopiert', 'Kein Textfeld: Meldung "Text kopiert"');

  // ---- 5. Missing microphone permission ----------------------------------
  await main.evaluate(() => window.NokiEinstellungen.zu());
  await main.evaluate(() => { window.__talk.status.mikrofon = 2; });
  await ev('noki://stimme', { was: 'start' });
  await ev('noki-talk', { zustand: 'fehler', meldung: 'Noki Talk braucht die Mikrofon-Freigabe.', aktion: 'mikrofon' });
  const z = await main.evaluate(() => window.NokiEinstellungen.zustand());
  check(z.offen && z.tab === 'talk', 'fehlende Freigabe oeffnet Einstellungen › Noki Talk');
  await einst.waitForSelector('#einstellungen [data-e="talk_mikro"]', { timeout: 10000 }).catch(() => {});
  await klick('[data-e="talk_mikro"]');
  check((await calls('noki_talk_mikrofon_freigabe')).length === 1, '"Freigabe öffnen" ruft die macOS-Einstellung');
  check((await tafel()).label === 'Noki Talk braucht die Mikrofon-Freigabe.', 'klare Meldung in der Tafel');

  await browser.close();
  server.close();
  console.log(fehler ? `${fehler} FEHLER` : 'ALLES OK');
  process.exit(fehler ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
