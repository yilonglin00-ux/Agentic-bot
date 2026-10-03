// Boot-Regressionstest: Noki muss starten, auch wenn der Assistent ausfaellt.
//
// Hintergrund: ein Parsefehler in intelligence.js liess `window.NokiIntelligence`
// undefiniert werden. Das Hauptskript von index.html brach daraufhin an
// `NokiIntelligence.create(...)` ab — Character, Renderschleife und alle
// Bedienflaechen kamen nie zustande. Noki war unsichtbar, Ask Noki blieb blank.
// Dieser Test haelt genau das fest.
//
// Braucht ein Playwright-Modul und das installierte Chrome (kein Browser-Download):
//   npm i --no-save playwright-core
//   PLAYWRIGHT_MODULE=playwright-core node tests/frontend-boot.cjs
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const { execFileSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');

const FRONTEND = ['index.html', 'ask.html', 'kompakt.html', 'noki-oberflaeche.js', 'intelligence.js', 'intelligence.css', 'code-view.js', 'code-view.css'];

let failed = 0;
const check = (cond, msg) => { if (!cond) { console.error('FAIL:', msg); failed++; } else console.log('ok -', msg); };

// 1. Jede ausgelieferte JS-Datei muss syntaktisch gueltig sein. Das ist der
//    billigste Test, der den urspruenglichen Ausfall verhindert haette.
for (const file of FRONTEND.filter(f => f.endsWith('.js'))) {
  try {
    execFileSync(process.execPath, ['--check', path.join(root, file)], { stdio: 'pipe' });
    check(true, `${file} ist syntaktisch gueltig`);
  } catch (e) {
    check(false, `${file} hat einen Syntaxfehler: ${String(e.stderr || e).split('\n').slice(0, 3).join(' ')}`);
  }
}

// Ein Frontend-Verzeichnis starten, optional mit kaputter/fehlender Datei.
const serveDir = async (mutate) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'noki-boot-'));
  for (const f of FRONTEND) fs.copyFileSync(path.join(root, f), path.join(dir, f));
  if (mutate) mutate(dir);
  const server = http.createServer((req, res) => {
    const f = path.resolve(dir, '.' + new URL(req.url, 'http://l').pathname);
    if (!f.startsWith(dir + path.sep)) { res.writeHead(403).end(); return; }
    fs.readFile(f, (e, d) => {
      if (e) { res.writeHead(404).end(); return; }
      res.setHeader('Content-Type', f.endsWith('.js') ? 'text/javascript' : f.endsWith('.css') ? 'text/css' : 'text/html');
      res.end(d);
    });
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  return { dir, server, port: server.address().port };
};

// `invoke`-Antworten pro Szenario; `null` heisst: dieser Aufruf wirft.
const boot = async (browser, port, responses) => {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on('pageerror', e => errors.push(String(e).split('\n')[0]));
  await page.addInitScript(r => {
    window.__nokiInvokes = [];
    const invoke = async (name, args) => {
      window.__nokiInvokes.push({ name, args: args || null });
      if (name in r) {
        if (r[name] === null) throw new Error('backend unavailable: ' + name);
        return r[name];
      }
      return null;
    };
    window.__TAURI__ = { core: { invoke }, event: { listen: async () => () => {}, emit: async () => {} }, window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
    window.__TAURI_INTERNALS__ = { invoke };
  }, responses);
  await page.goto(`http://127.0.0.1:${port}/index.html`);
  await page.waitForTimeout(2500);
  const probe = await page.evaluate(() => {
    const canvas = document.querySelector('canvas');
    return {
      hasIntelligence: typeof window.NokiIntelligence,
      hasAsk: typeof window.NokiAsk,
      hasSettings: typeof window.NokiEinstellungen,
      // Der Renderer skaliert die Leinwand auf die Fenstergroesse. Bleibt sie
      // auf 300x150, ist die Renderschleife nie angelaufen.
      canvasWidth: canvas ? canvas.width : 0,
      canvasHeight: canvas ? canvas.height : 0,
      // Es gibt mehrere Leinwaende (2D-Overlays und die 3D-Buehne); gefragt ist,
      // ob ueberhaupt eine einen WebGL-Kontext haelt.
      webgl: [...document.querySelectorAll('canvas')].some(c => {
        try { return !!(c.getContext('webgl2') || c.getContext('webgl')); } catch (e) { return false; }
      }),
      pose: typeof window.NokiPose === 'function' ? !!window.NokiPose() : false,
    };
  });
  return { page, errors, probe };
};

const SETTINGS_OK = { intelligence_settings: { settings: { level: 'off', ask: true, web: false, unload_min: 10, memory: false, mode: 'normal', engine_mode: 'local_and_cloud' }, assistant_mode: 'work', installed: true, model: 'qwen3.5:9b', model_label: 'Qwen3.5 9B', ram_gb: 8, loaded: false, memory_count: 0 } };

(async () => {
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--enable-webgl', '--ignore-gpu-blocklist'] });
  try {
    // A. Normalfall: alles da.
    {
      const s = await serveDir(null);
      const { errors, probe } = await boot(browser, s.port, SETTINGS_OK);
      check(errors.length === 0, 'Boot ohne uncaught Exception' + (errors.length ? ': ' + errors[0] : ''));
      check(probe.hasIntelligence === 'object', 'NokiIntelligence geladen');
      check(probe.hasAsk === 'object', 'NokiAsk vorhanden');
      check(probe.hasSettings === 'object', 'Einstellungen vorhanden');
      check(probe.canvasWidth > 400 && probe.canvasHeight > 300, `Leinwand ist auf Fenstergroesse gesetzt (${probe.canvasWidth}x${probe.canvasHeight})`);
      check(probe.webgl, 'WebGL-Kontext steht');
      check(probe.pose, 'Character-Zustand existiert');
      s.server.close();
    }

    // B. intelligence.js kaputt: Noki muss trotzdem laufen (der echte Ausfall).
    {
      const s = await serveDir(dir => {
        const f = path.join(dir, 'intelligence.js');
        fs.writeFileSync(f, fs.readFileSync(f, 'utf8').replace('function action(a, v) {', 'function action(a, v) { ;;;}}}'));
      });
      const { errors, probe } = await boot(browser, s.port, SETTINGS_OK);
      check(probe.canvasWidth > 400, 'kaputte intelligence.js: Leinwand trotzdem initialisiert');
      check(probe.webgl, 'kaputte intelligence.js: WebGL trotzdem initialisiert');
      check(probe.pose, 'kaputte intelligence.js: Character trotzdem vorhanden');
      check(probe.hasAsk === 'object', 'kaputte intelligence.js: Attrappe steht statt undefined');
      check(!errors.some(e => e.includes("reading 'create'")), 'kein Abbruch an NokiIntelligence.create');
      s.server.close();
    }

    // C. intelligence.js fehlt ganz.
    {
      const s = await serveDir(dir => fs.unlinkSync(path.join(dir, 'intelligence.js')));
      const { probe } = await boot(browser, s.port, SETTINGS_OK);
      check(probe.canvasWidth > 400 && probe.pose, 'fehlende intelligence.js: Noki startet trotzdem');
      s.server.close();
    }

    // D. Backend-Aufrufe schlagen fehl (kein Router, kein Status, keine Settings).
    {
      const s = await serveDir(null);
      const { errors, probe, page } = await boot(browser, s.port, { intelligence_settings: null, intelligence_status: null });
      check(probe.canvasWidth > 400 && probe.pose, 'Settings-Invoke wirft: Noki startet trotzdem');
      check(!errors.some(e => e.includes('Unhandled') || e.includes('TypeError')), 'kein TypeError aus fehlgeschlagenen Invokes');
      // Und die Settings-Seite selbst faellt auf einen Hinweis zurueck.
      const html = await page.evaluate(() => {
        try { window.NokiEinstellungen.auf('intelligence'); } catch (e) { return 'THREW: ' + e; }
        const el = document.querySelector('#einstellungen .e-inhalt');
        return el ? el.textContent : 'KEIN INHALT';
      });
      check(!String(html).startsWith('THREW'), 'Settings oeffnen wirft nicht, wenn das Backend schweigt');
      s.server.close();
    }

    // E. Router-Payload unbrauchbar: providers fehlt, usage null, Ketten leer.
    {
      const s = await serveDir(null);
      const kaputt = { ...SETTINGS_OK, intelligence_status: { ask: true, loaded: false, model: 'qwen3.5:9b', model_label: 'Qwen3.5 9B', router: { engine_mode: 'local_and_cloud', local_only: false } } };
      const { probe, page } = await boot(browser, s.port, kaputt);
      check(probe.canvasWidth > 400 && probe.pose, 'unvollstaendiger Routerstatus: Noki startet trotzdem');
      const text = await page.evaluate(() => {
        try {
          window.NokiEinstellungen.auf('intelligence');
          return document.querySelector('#einstellungen .e-inhalt').textContent;
        } catch (e) { return 'THREW: ' + e; }
      });
      check(!String(text).startsWith('THREW'), 'unvollstaendiger Routerstatus wirft nicht');
      check(String(text).includes('Intelligence Engine'), 'Engine-Abschnitt rendert trotzdem');
      s.server.close();
    }

    // F. Settings sind nur UI; Appearance und Timer-Parade behalten ihre
    //    vereinbarten Besitzer-/Reset-Grenzen.
    {
      const s = await serveDir(null);
      const { page } = await boot(browser, s.port, SETTINGS_OK);
      await page.evaluate(() => {
        window.NokiEnergie.setzen('energisch');
        window.NokiEinstellungen.auf('darstellung');
      });
      const vor = await page.evaluate(() => window.NokiRaum.ort());
      await page.waitForTimeout(5000);
      const nach = await page.evaluate(() => window.NokiRaum.ort());
      check(Math.hypot(nach.x - vor.x, nach.y - vor.y) > 2,
        `offene Settings stoppen Noki nicht (${Math.round(vor.x)},${Math.round(vor.y)} -> ${Math.round(nach.x)},${Math.round(nach.y)})`);

      const feld = page.locator('#einstellungen [data-farbfeld]');
      const box = await feld.boundingBox();
      await page.mouse.move(box.x + box.width * 0.55, box.y + box.height - 1);
      await page.mouse.down(); await page.mouse.up();
      const schwarz = await page.evaluate(() => {
        const z = window.NokiEinstellungen.zustand();
        return { farbe: z.werte.farbe, muster: getComputedStyle(document.querySelector('.e-farb-muster')).backgroundColor };
      });
      check(schwarz.farbe && schwarz.farbe.v <= 0.01 && schwarz.muster === 'rgb(0, 0, 0)',
        `Farbfeld erreicht echtes Schwarz (${schwarz.muster})`);

      await page.locator('#einstellungen [data-groesse]').evaluate(el => {
        el.value = '2.5'; el.dispatchEvent(new Event('input', { bubbles: true }));
      });
      await page.locator('#einstellungen [data-e="farbe_standard"]').click();
      await page.waitForTimeout(1300);
      const reset = await page.evaluate(() => ({
        farbe: window.NokiEinstellungen.zustand().werte.farbe,
        groesse: window.NokiRaum.groesse(), energie: window.NokiEnergie.lesen()
      }));
      check(reset.farbe === null && Math.abs(reset.groesse - 54) < 0.2, `Standard setzt Farbe und Groesse zurueck (${reset.groesse})`);
      check(reset.energie === 'energisch', 'Standard laesst Energy unveraendert');

      await page.evaluate(() => window.NokiTimer.start(1 / 60));
      await page.waitForTimeout(250);
      const tiefe = await page.evaluate(() => {
        const p = document.querySelector('#timerProp'), e = document.querySelector('#einstellungen');
        return { parent: p && p.parentElement && p.parentElement.id, pz: +(getComputedStyle(p).zIndex || 0), ez: +(getComputedStyle(e).zIndex || 0) };
      });
      check(tiefe.parent === 'stage' && tiefe.pz < tiefe.ez, `Timer folgt Nokis Stapel unter Settings (${tiefe.pz} < ${tiefe.ez})`);
      await page.waitForTimeout(900);
      const ende = await page.evaluate(() => ({
        prop: getComputedStyle(document.querySelector('#timerProp')).display,
        parade: window.NokiSchwarm.paradeZustand(),
        layer: window.__nokiInvokes.filter(x => x.name === 'noki_parade_ebene').length
      }));
      check(ende.prop === 'none', 'Uhr ist bei 00:00 sofort weg');
      check(ende.parade && ende.parade.phase === 'winken', `Parade wartet sichtbar auf das Winken (${ende.parade && ende.parade.phase})`);
      check(ende.layer > 0, 'ein gemeinsamer Parade-Layer wurde aktiviert');
      await page.keyboard.press('a');
      await page.waitForTimeout(150);
      const abbruch = await page.evaluate(() => ({ schwarm: window.NokiSchwarm.zustand().an, parade: window.NokiSchwarm.paradeZustand() }));
      check(!abbruch.schwarm && !abbruch.parade, 'Any-Key raeumt Schwarm und echte Parade auf');

      s.server.close();
    }
  } finally {
    await browser.close();
  }
  if (failed) { console.error(`\n${failed} Pruefung(en) fehlgeschlagen`); process.exit(1); }
  console.log('\nFrontend-Boot: alle Pruefungen bestanden');
})();
