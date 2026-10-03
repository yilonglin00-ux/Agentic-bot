// Intelligence Engine settings view: does it render the ROUTER payload?
//
// The mock deliberately uses names that appear nowhere in the frontend
// ("TestCloud Mid", "TestLocal Coder"). Anything the panel shows must therefore
// have come from the backend — that is the whole point of this test.
//
// Needs a Playwright module and the installed Chrome (no browser download):
//   npm i --no-save playwright-core
//   PLAYWRIGHT_MODULE=playwright-core node tests/intelligence-engine-ui.cjs
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');

const routerPayload = (localOnly, disabled) => ({
  engine_mode: localOnly ? 'only_local' : 'local_and_cloud',
  local_only: localOnly,
  work_chains: localOnly
    ? { FAST: ['TestLocal Fast'], NORMAL: ['TestLocal Normal'], DEEP: ['TestLocal Normal'], LOCAL: ['TestLocal Normal', 'TestLocal Fast'] }
    : { FAST: ['TestCloud Fast', 'TestCloud Mid', 'TestLocal Fast'], NORMAL: ['TestCloud Mid', 'TestCloud Fast', 'TestLocal Normal'], DEEP: ['TestCloud Mid', 'TestLocal Normal'], LOCAL: ['TestLocal Normal', 'TestLocal Fast'] },
  coding_chains: localOnly
    ? { FAST: ['TestLocal Coder'], NORMAL: ['TestLocal Coder'], COMPLEX: ['TestLocal Coder'], LOCAL: ['TestLocal Coder', 'TestLocal Normal'] }
    : { FAST: ['TestCloud Fast', 'TestLocal Coder'], NORMAL: ['TestCloud Fast', 'TestLocal Coder'], COMPLEX: ['TestLocal Coder', 'TestCloud Fast'], LOCAL: ['TestLocal Coder', 'TestLocal Normal'] },
  current_defaults: { work: localOnly ? 'TestLocal Normal' : 'TestCloud Mid', coding: localOnly ? 'TestLocal Coder' : 'TestCloud Fast' },
  specialist: { id: 'test_specialist', model: 'TestSpecialist', label: 'Limited quota', hint: 'Used only for selected high-value tasks', rules: ['deep_only'] },
  providers: [
    { id: 'p_local_fast', provider: 'local', display_name: 'TestLocal Fast', local: true, local_quantization: 'Q4_K_M', state: 'available', credentials: 'not_required', remaining_usage: null, reset_hint: null, cooldown_remaining_s: null, specialist_only: false, requests: 0, failures: 0, last_429_unix: null, last_error_kind: null, measured: { work_quality: 83.8, work_latency_ms: 4364, code_quality: null, code_latency_ms: null, code_tests_passed: null, availability: 100, completion: 100, note: null } },
    { id: 'p_local_normal', provider: 'local', display_name: 'TestLocal Normal', local: true, local_quantization: 'Q4_K_M', state: 'available', credentials: 'not_required', remaining_usage: null, reset_hint: null, cooldown_remaining_s: null, specialist_only: false, requests: 0, failures: 0, last_429_unix: null, last_error_kind: null, measured: { work_quality: 87.5, work_latency_ms: 7056, code_quality: null, code_latency_ms: null, code_tests_passed: null, availability: 100, completion: 100, note: null } },
    { id: 'p_local_coder', provider: 'local', display_name: 'TestLocal Coder', local: true, local_quantization: 'Q4_K_M', state: 'available', credentials: 'not_required', remaining_usage: null, reset_hint: null, cooldown_remaining_s: null, specialist_only: false, requests: 0, failures: 0, last_429_unix: null, last_error_kind: null, measured: { work_quality: null, work_latency_ms: null, code_quality: 78.8, code_latency_ms: 11026, code_tests_passed: '6/8', availability: 100, completion: 100, note: null } },
    { id: 'p_cloud_fast', provider: 'testrouter', display_name: 'TestCloud Fast', local: false, state: disabled.includes('p_cloud_fast') ? 'disabled' : 'available', credentials: 'connected', remaining_usage: null, reset_hint: null, cooldown_remaining_s: null, specialist_only: false, requests: 0, failures: 0, last_429_unix: null, last_error_kind: null, measured: { work_quality: 74.5, work_latency_ms: 988, code_quality: 80, code_latency_ms: 1066, code_tests_passed: '6/8', availability: 100, completion: 100, note: null } },
    { id: 'p_cloud_mid', provider: 'testmistral', display_name: 'TestCloud Mid', local: false, state: disabled.includes('p_cloud_mid') ? 'disabled' : 'rate_limited', credentials: 'connected', remaining_usage: '17 req', quota_scope: 'shared_provider', reset_hint: null, cooldown_remaining_s: 42, specialist_only: false, requests: 3, failures: 1, last_429_unix: 1, last_error_kind: 'rate_limited', measured: { work_quality: 87.5, work_latency_ms: 1589, code_quality: null, code_latency_ms: null, code_tests_passed: null, availability: 100, completion: 100, note: null } },
    { id: 'p_cloud_code', provider: 'testmistral', display_name: 'TestCloud Code', local: false, state: disabled.includes('p_cloud_code') ? 'disabled' : 'available', credentials: 'connected', remaining_usage: null, reset_hint: null, cooldown_remaining_s: null, specialist_only: false, requests: 0, failures: 0, last_429_unix: null, last_error_kind: null, measured: { work_quality: null, work_latency_ms: null, code_quality: 77, code_latency_ms: 1200, code_tests_passed: null, availability: 100, completion: 100, note: null } },
    { id: 'p_specialist', provider: 'testgoogle', display_name: 'TestSpecialist', local: false, state: disabled.includes('testgoogle') ? 'disabled' : 'quota_exhausted', credentials: 'credentials_required', remaining_usage: null, reset_hint: null, cooldown_remaining_s: 600, specialist_only: true, requests: 0, failures: 0, last_429_unix: null, last_error_kind: 'rate_limited', measured: { work_quality: 88, work_latency_ms: 4167, code_quality: 100, code_latency_ms: 4864, code_tests_passed: '3/8', availability: 50, completion: 50, note: 'small sample' } },
  ],
  parked: [{ id: 'parked_a', display_name: 'TestParked', state: 'credentials_required', note: 'nicht eingerichtet' }],
});

(async () => {
  const server = http.createServer((req, res) => {
    const file = path.resolve(root, '.' + new URL(req.url, 'http://l').pathname);
    if (!file.startsWith(root + path.sep)) { res.writeHead(403).end(); return; }
    fs.readFile(file, (e, d) => {
      if (e) { res.writeHead(404).end(); return; }
      res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
      res.end(d);
    });
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  let failed = 0;
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.on('pageerror', e => { console.error('PAGE:', String(e)); failed++; });
    await page.addInitScript(p => {
      window.__calls = [];
      window.__router = p;
      window.__loaded = true;
      const invoke = async (name, args = {}) => {
        window.__calls.push({ name, args });
        if (name === 'intelligence_settings') return { settings: { level: 'off', ask: true, web: false, unload_min: 10, memory: false, mode: 'normal', engine_mode: window.__router.engine_mode }, assistant_mode: 'work', installed: true, model: 'qwen3.5:4b', model_label: 'Qwen3.5 4B', ready_runtime_model: { canonical_model_id: 'qwen3.5:4b', display_name: 'Qwen3.5 4B', provider_id: 'local', execution_lane: 'LOCAL' }, thinking: false, ram_gb: 8, loaded: window.__loaded, memory_count: 0, router: window.__router };
        if (name === 'intelligence_status') return { ask: true, loaded: window.__loaded, model: 'qwen3.5:4b', model_label: 'Qwen3.5 4B', ready_runtime_model: { canonical_model_id: 'qwen3.5:4b', display_name: 'Qwen3.5 4B', provider_id: 'local', execution_lane: 'LOCAL' }, thinking: false, loading: false, router: window.__router };
        if (name === 'intelligence_chat') return {
          text: 'Lokale Testantwort.', route: 'LOCAL', sources: [], confidence: { level: 'hoch' },
          runtime_model: { canonical_model_id: 'qwen3.5:9b', display_name: 'Qwen3.5 9B', provider_id: 'local', execution_lane: 'LOCAL' }
        };
        if (name === 'intelligence_unload') { window.__loaded = false; return invoke('intelligence_status'); }
        if (name === 'intelligence_load') { window.__loaded = true; return invoke('intelligence_status'); }
        if (name === 'ax_status') return { trusted: false, bundle: 'com.noki.test', pid: 1, exe: '/test/Noki' };
        if (name === 'platz_liste') return { profile: [{ name: 'Uni', apps: [] }, { name: 'Coding', apps: [] }, { name: 'Normal', apps: [] }], ax: true };
        if (name === 'fokus_apps') return [];
        if (name === 'intelligence_engine_set_mode') { window.__router = window.__build(args.mode === 'only_local', window.__disabled || []); return window.__router; }
        if (name === 'intelligence_engine_toggle_provider') {
          window.__disabled = window.__disabled || [];
          if (args.enabled) window.__disabled = window.__disabled.filter(x => x !== args.id);
          else if (!window.__disabled.includes(args.id)) window.__disabled.push(args.id);
          window.__router = window.__build(window.__router.local_only, window.__disabled);
          return window.__router;
        }
        if (name === 'intelligence_engine_toggle_model') {
          window.__disabled = window.__disabled || [];
          if (args.enabled) window.__disabled = window.__disabled.filter(x => x !== args.id);
          else if (!window.__disabled.includes(args.id)) window.__disabled.push(args.id);
          window.__router = window.__build(window.__router.local_only, window.__disabled);
          return window.__router;
        }
        return null;
      };
      window.__listeners = {};
      window.__TAURI__ = { core: { invoke }, event: { listen: async (n, f) => { (window.__listeners[n] = window.__listeners[n] || []).push(f); return () => {}; }, emit: async () => {} }, window: { getCurrentWindow: () => ({ onMoved: async () => () => {}, onResized: async () => () => {}, onFocusChanged: async () => () => {} }) } };
      window.__TAURI_INTERNALS__ = { invoke };
    }, routerPayload(false, []));
    await page.goto(`http://127.0.0.1:${server.address().port}/index.html`);
    await page.waitForFunction(() => window.NokiAsk && typeof window.NokiAsk.settingsHTML === 'function');
    await page.evaluate(b => { window.__build = new Function('localOnly', 'disabled', 'return (' + b + ')(localOnly, disabled)'); }, routerPayload.toString());

    // The real settings page, so the assertions and the clicks act on one tree.
    await page.waitForFunction(() => window.NokiEinstellungen && window.NokiAsk.status && window.NokiAsk.status.router);
    await page.evaluate(() => window.NokiEinstellungen.auf('intelligence'));
    const engine = () => page.locator('#einstellungen .e-sektion').first();
    const render = async () => {
      await engine().waitFor();
      return { text: await engine().textContent(), html: await engine().innerHTML() };
    };
    const check = (cond, msg) => { if (!cond) { console.error('FAIL:', msg); failed++; } else console.log('ok -', msg); };

    let view = await render();
    // --- Local + Cloud ---
    check(view.text.includes('TestCloud Mid'), 'chains are rendered from the backend payload');
    check(/Work Default[\s\S]*?TestCloud Mid/.test(view.text), 'current work default comes from the router');
    check(/Coding Default[\s\S]*?TestCloud Fast/.test(view.text), 'current coding default comes from the router');
    check(!['FAST', 'NORMAL', 'DEEP', 'COMPLEX'].some(t => view.text.includes(t)), 'internal task classes are not duplicated in settings');
    check(/Work Routing[\s\S]*?TestCloud Mid → TestCloud Fast → TestLocal Normal/.test(view.text), 'work summary order is the backend normal chain');
    check(/Coding Routing[\s\S]*?TestCloud Fast → TestLocal Coder → TestLocal Normal/.test(view.text), 'coding summary includes the backend local fallback once');
    check(view.text.includes('Rate limit erreicht'), 'a rate limit is named only where meaningful');
    check(view.text.includes('Kontingent aufgebraucht'), 'exhausted quota is named only where meaningful');
    check(view.text.includes('Nicht verbunden'), 'a missing credential is named only where meaningful');
    check(!view.text.includes('Verfügbar') && !/ni-provider-status/.test(view.html), 'available providers have no badge or status clutter');
    check(!/Free (?:Tier|Endpoint)/.test(view.text) && !/\((?:OpenRouter|Mistral|Google)\)/.test(view.text), 'model names have no provider or tier suffix');
    check(!/sk-|Bearer |API_KEY|••|\*\*\*/.test(view.html), 'no key material, not even masked');
    check(!view.text.includes('Limited quota') && !view.text.includes('Premium'), 'inactive specialist detail is absent');
    check(!/Work \d+(?:\.\d+)?%|Coding \d+(?:\.\d+)?%|~\d/.test(view.text), 'benchmark quality and latency stay out of settings');
    check(!view.text.includes('Usage unavailable'), 'missing usage data is hidden, not shown as a status');
    check(view.text.includes('Verbleibend: 17 req'), 'a real reported allowance is shown');
    check(view.text.includes('Geteiltes Provider-Kontingent'), 'shared quota is never presented as per-model quota');
    check(view.text.includes('Wieder in 42 s'), 'a live cooldown is shown');
    check(!/Reset /.test(view.text), 'no reset is invented when the provider sent none');
    check(!view.text.includes('Derzeit nicht aktiv') && !view.text.includes('TestParked'), 'parked providers do not clutter settings');
    check(view.text.includes('TestLocal Coder') && view.text.includes('Q4_K_M') && !view.text.includes('6/8'), 'local floor and canonical quantization are shown without benchmark statistics');

    // --- one model toggle goes through the backend without touching its sibling ---
    const mistralSwitches = engine().locator('[data-e="ni-toggle-model"][data-v^="p_cloud_"]');
    check((await mistralSwitches.count()) >= 3, 'cloud models each use the existing Noki toggle control');
    await engine().locator('[data-e="ni-toggle-model"][data-v^="p_cloud_mid:"]').click();
    await page.waitForFunction(() => window.__calls.some(c => c.name === 'intelligence_engine_toggle_model'));
    const toggle = await page.evaluate(() => window.__calls.filter(c => c.name === 'intelligence_engine_toggle_model').pop());
    check(toggle.args.id === 'p_cloud_mid' && toggle.args.enabled === false, 'the toggle disables exactly one model via the backend command');
    await page.waitForFunction(() => window.NokiAsk.status.router.providers.find(p => p.id === 'p_cloud_mid').state === 'disabled');
    view = await render();
    check(view.text.includes('Deaktiviert'), 'the disabled state becomes visible as plain text');
    check(await engine().locator('[data-e="ni-toggle-model"][data-v^="p_cloud_code:"]').isChecked(), 'disabling one Mistral model leaves its sibling enabled');

    // --- local model action stays present and reverses through the backend ---
    check((await engine().locator('[data-e="ni-unload"]').textContent()) === 'Modell entladen', 'loaded model offers Modell entladen');
    await engine().locator('[data-e="ni-unload"]').click();
    await page.waitForFunction(() => window.__loaded === false);
    check((await engine().locator('[data-e="ni-load"]').textContent()) === 'Modell laden', 'after unload the same place offers Modell laden');
    await engine().locator('[data-e="ni-load"]').click();
    await page.waitForFunction(() => window.__loaded === true);
    check((await engine().locator('[data-e="ni-unload"]').textContent()) === 'Modell entladen', 'after reload the action returns to Modell entladen');

    // --- Only Local ---
    await engine().locator('[data-e="ni-engine"][data-v="only_local"]').click();
    await page.waitForFunction(() => window.NokiAsk.status.router.local_only === true);
    view = await render();
    check(!view.text.includes('Keine Daten werden an Cloudmodelle gesendet.'), 'local-only does not repeat an obvious explanation');
    check(/Work Default[\s\S]*?TestLocal Normal/.test(view.text), 'local-only default comes from the router');
    check(!view.text.includes('TestCloud Mid') && !view.text.includes('TestSpecialist'), 'no cloud provider is offered in local-only');
    check(view.text.includes('TestLocal Coder'), 'the local floor stays visible');

    // --- newly added status dots are absent outside Intelligence too ---
    await page.locator('[data-e="tab"][data-v="fenster"]').click();
    await page.waitForFunction(() => document.querySelector('#einstellungen .e-inhalt').textContent.includes('Freigabe fehlt'));
    check((await page.locator('#einstellungen .e-punkt').count()) === 0, 'Freigabe fehlt has no status dot');
    await page.locator('[data-e="tab"][data-v="platz"]').click();
    await page.waitForFunction(() => document.querySelector('#einstellungen .e-inhalt').textContent.includes('Kein Fokus aktiv'));
    const focusMarker = await page.locator('#einstellungen .n-status').evaluate(el => getComputedStyle(el, '::before').content);
    check(focusMarker === 'none', 'Kein Fokus aktiv has no generated status dot');

    // --- Offline response provenance ---
    await page.evaluate(() => window.NokiAsk.open());
    await page.locator('#askNoki textarea').fill('Lokaler Provenance-Test');
    await page.locator('#askNoki textarea').press('Enter');
    await page.waitForFunction(() => document.querySelector('#askNoki .ni-model-provenance'));
    check((await page.locator('#askNoki .ni-model-provenance').last().textContent()) === 'Local · Qwen3.5 9B', 'assistant response keeps its actual local model provenance');
    const lastTurn = await page.evaluate(() => window.NokiAsk.lastTurn());
    check(lastTurn.runtime_model && lastTurn.runtime_model.canonical_model_id === 'qwen3.5:9b', 'per-message provenance is retained in conversation state');
    await page.evaluate(() => window.NokiAsk.action('ni-engine', 'only_local'));
    await page.waitForFunction(() => document.querySelector('#askNoki .ni-model-indicator').textContent.includes('Qwen3.5 9B'));
    check((await page.locator('#askNoki .ni-model-indicator').textContent()).includes('Qwen3.5 9B'), 'startup Qwen 4B readiness cannot overwrite successful response provenance');

    // --- settings never benchmark ---
    check(!(await page.evaluate(() => window.__calls.some(c => c.name === 'intelligence_engine_run_benchmarks'))), 'opening settings runs no benchmark');
  } finally {
    await browser.close();
    server.close();
  }
  if (failed) { console.error(`\n${failed} check(s) failed`); process.exit(1); }
  console.log('\nIntelligence Engine UI: all checks passed');
})();
