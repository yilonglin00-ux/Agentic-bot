/* Noki Code – desktop view on the persistent code session.
 * Features:
 * 1. Noki Code Agent Terminal: persistent JackOD 9B session, R0–R3 Capability Policies.
 * 2. Normal Local Shell Terminals: real macOS login shell (zsh) over PTY via libc openpty.
 * 3. Multi-terminal Tab Bar: [ ◆ Noki ] [ Terminal 1 ] [ Terminal 2 ] [+]
 * 4. Edge-to-edge workspace filling 100% of code area, calm clean hierarchy.
 * 5. Strict security isolation between user shell PTYs and JackOD. */
(function () {
  'use strict';

  var KINDS = {
    LAUF: ['', 'Lauf', 'done'], EINGABE: ['', 'Eingabe', 'plan'], OUTPUT: ['', 'Ausgabe', 'look'], MODEL: ['◆', 'Modell', 'plan'], NOKI: ['→', 'Noki', 'plan'], THINK: ['◇', 'Denken', 'look'], AUDIT: ['◆', 'Prüfung', 'plan'], FILE: ['✎', 'Datei', 'edit'],
    INSPECT: ['◇', 'Inspect', 'look'], SEARCH: ['⌕', 'Search', 'look'], PLAN: ['◆', 'Plan', 'plan'],
    PATCH: ['╱', 'Patch', 'edit'], REPAIR: ['', 'Repair', 'edit'], TEST: ['▶', 'Test', 'test'],
    VERIFY: ['✓', 'Verified', 'ok'], DONE: ['✓', 'Done', 'done'], CONFIRM: ['☐', 'Confirm', 'warn'],
    WARNING: ['!', 'Warning', 'warn'], ERROR: ['×', 'Error', 'err']
  };
  // GLYPH-REGEL. Die Ansicht rendert in --nc-mono ("SF Mono", ui-monospace,
  // Menlo, Monaco). SF Mono ist auf vielen Macs NICHT installiert; dann
  // zeichnet Menlo. Menlo deckt den hier benutzten Vorrat vollstaendig ab —
  // mit genau zwei Ausnahmen, die frueher hier standen: U+26BF (SQUARED KEY)
  // und U+29D7 (BLACK HOURGLASS). Beide kamen als Ersatzglyph heraus, und
  // genau das waren die zwei "??" im Terminal. Beide Stellen meinten
  // "wartet auf deine Bestaetigung" und benutzen jetzt U+2610 (BALLOT BOX),
  // das Menlo kennt. Wer hier ein Zeichen ergaenzt, prueft es vorher gegen
  // Menlo — nicht gegen die eigene Vorschau.
  var RESULT = { PASS: ['✓', 'Tests bestanden', 'ok'], FAIL: ['✗', 'Tests fehlgeschlagen', 'err'], WARN: ['!', 'Mit Warnungen', 'warn'],
    ERROR: ['×', 'Fehler', 'err'], CANCELLED: ['–', 'Abgebrochen', 'warn'] };
  var ASCII_WORDMARK =
"███╗   ██╗ ██████╗ ██╗  ██╗██╗\n" +
"████╗  ██║██╔═══██╗██║ ██╔╝██║\n" +
"██╔██╗ ██║██║   ██║█████╔╝ ██║\n" +
"██║╚██╗██║██║   ██║██╔═██╗ ██║\n" +
"██║ ╚████║╚██████╔╝██║  ██╗██║\n" +
"╚═╝  ╚═══╝ ╚═════╝ ╚═╝  ╚═╝╚═╝\n" +
"          N O K I   C O D E";
  var HELP = ['/functional  /creative   Stil umschalten', '/status  /history  /sessions', '/clear   Ansicht leeren (Verlauf bleibt)',
    '/clear-history   Verlauf löschen (mit Bestätigung)', '/exit    Terminal ausblenden', '', '↑↓ Eingabeverlauf · ⌃C abbrechen · ⌘K leeren'];

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }
  function styleName(s) { return s === 'creative' ? 'Kreativ' : 'Funktional'; }
  function dayKey(ts) { var d = new Date(ts * 1000); return d.getFullYear() + '-' + d.getMonth() + '-' + d.getDate(); }
  function dayLabel(ts) {
    var d = new Date(ts * 1000), t = new Date(); t.setHours(0, 0, 0, 0);
    var diff = Math.round((t - new Date(d.getFullYear(), d.getMonth(), d.getDate())) / 86400000);
    return diff === 0 ? 'Heute' : diff === 1 ? 'Gestern' : d.toLocaleDateString('de-DE', { weekday: 'short', day: 'numeric', month: 'short' });
  }
  function clock(ts) { var d = new Date(ts * 1000); return String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0'); }

  // 256-color palette to RGB string
  function color256(n) {
    if (n < 0 || n > 255) return null;
    if (n < 8) {
      var std = ['#1d1f21', '#ff7b72', '#7ee787', '#f2cc60', '#79c0ff', '#d2a8ff', '#56d4dd', '#f0f6fc'];
      return std[n];
    }
    if (n < 16) {
      var bright = ['#6e7681', '#ffa198', '#56d364', '#e3b341', '#a5d6ff', '#e2c5ff', '#79e6f3', '#ffffff'];
      return bright[n - 8];
    }
    if (n < 232) {
      var idx = n - 16;
      var r = Math.floor(idx / 36) * 51;
      var g = Math.floor((idx % 36) / 6) * 51;
      var b = (idx % 6) * 51;
      return 'rgb(' + r + ',' + g + ',' + b + ')';
    }
    var gray = (n - 232) * 10 + 8;
    return 'rgb(' + gray + ',' + gray + ',' + gray + ')';
  }

  window.NokiCodeView = function (root, opts) {
    var call = opts.call, onStatus = opts.onStatus || function () {};
    var headerEl = opts.header || (root.parentElement && root.parentElement.querySelector('header')) || document.querySelector('#askNoki header');
    var status = {}, meta = null, entered = false, attached = false, attaching = false, reattach = 0;
    // Exactly one Code style is selected at any time; this holds it across re-renders.
    var gewaehlterStil = 'functional';
    var pending = null, stick = true, lastDay = '', inputHistory = [], histPos = -1, draft = '', pollT = 0;
    var remember = function (v) { try { if (v == null) return localStorage.getItem('noki.code.embedded') === '1'; localStorage.setItem('noki.code.embedded', v ? '1' : '0'); } catch (e) { return false; } };

    // Multi-terminal tabs: Code Space sessions, zsh shells, repo agent.
    var activeTab = 'noki', repoOffen = false;
    var shellTabs = [];
    var shellCount = 0;

    root.classList.add('nc');
    root.innerHTML =
      '<section class="nc-over">' +
        '<div class="nc-brand">' +
          '<div class="nc-brand-title">NOKI TERMINAL</div>' +
          '<div class="nc-brand-sub">Local Shell & JackOD Coder</div>' +
        '</div>' +
        '<dl class="nc-kv">' +
          '<dt>Status</dt><dd class="nc-o-state"></dd>' +
          '<dt>Model</dt><dd class="nc-o-model"></dd>' +
          '<dt>Style</dt><dd class="nc-o-style"></dd>' +
          '<dt>Project</dt><dd class="nc-o-project"></dd>' +
          '<dt>Letzter Lauf</dt><dd class="nc-o-result"></dd>' +
        '</dl>' +
        '<div class="nc-actions"><button type="button" class="nc-btn nc-primary" data-act="here"></button>' +
        '<button type="button" class="nc-btn" data-act="ext">↗ Im Terminal öffnen</button>' +
        '<button type="button" class="nc-btn" data-act="projekte">Code-Projekte</button></div>' +
        '<p class="nc-o-err" hidden></p>' +
      '</section>' +
        // ONE fixed header for Terminal AND Projekte: [left slot][spacer][nav].
        // The nav never moves; the left slot shows the tabs, or in the
        // projects view the way back to the Noki Terminal.
        '<div class="nc-tab-bar">' +
          '<div class="nc-kopf-links">' +
          '<button type="button" class="nc-kopf-zurueck" data-act="p-terminal" title="Zurück zum Noki Terminal">Noki Terminal</button>' +
          '<div class="nc-tabs" role="tablist">' +
            '<button type="button" class="nc-tab active" role="tab" data-tab="agent"><span>Noki Terminal</span></button>' +
            '<button type="button" class="nc-tab-add" data-act="add-shell" title="Neues Terminal öffnen" aria-label="Neues Terminal öffnen">+</button>' +
          '</div>' +
          '</div>' +
          '<div class="nc-tab-bar-spacer"></div>' +
          // ONE navigation: Projekte | Funktional | Kreativ (same segment look).
          '<div class="nc-style-status-wrap"><div class="nc-nav">' +
            '<button type="button" class="nc-nav-proj" data-act="projekte" title="Von Noki gebaute Projekte">Projekte</button>' +
            '<div class="nc-seg ni-code-styles" role="radiogroup" aria-label="Modus wählen">' +
              '<button type="button" role="radio" data-style="functional" title="Funktional: präzise, minimale Patches">Funktional</button>' +
              '<button type="button" role="radio" data-style="creative" title="Kreativ: freier Entwurf, Alternativen">Kreativ</button>' +
            '</div>' +
          '</div></div>' +
        '</div>' +
      '<section class="nc-term" hidden>' +
        '<div class="nc-bench" hidden></div>' +
        '<div class="nc-agent-view">' +
          '<div class="nc-menu" hidden role="menu">' +
            // RUNTIME-SEKTION. Sie steht im vorhandenen Mehr-Menue derselben
            // Leiste, in der auch das Terminal erreichbar ist. Bewusst NICHT
            // im "+": der legt mit einem Klick ein Terminal an, und genau
            // das soll er weiter tun. Inhalt kommt aus intelligence_status
            // (status.runtime) — hier steht kein fester Text.
            '<div class="nc-menu-sek" data-sek="runtime"><span class="nc-menu-h">Runtime</span>' +
              '<span class="nc-menu-z" data-r="kind">—</span>' +
              '<span class="nc-menu-z" data-r="runtime">—</span>' +
              '<span class="nc-menu-h">Model</span>' +
              '<span class="nc-menu-z" data-r="model">—</span>' +
            '</div>' +
            '<button type="button" data-act="repo" role="menuitem">Repo-Agent (JackOD · ~/NOKI)</button>' +
            '<button type="button" data-act="clear" role="menuitem">Ansicht leeren<kbd>⌘K</kbd></button>' +
            '<button type="button" data-act="status" role="menuitem">Status</button>' +
            '<button type="button" data-act="sessions" role="menuitem">Sessions</button>' +
            '<button type="button" data-act="wipe" role="menuitem" class="nc-danger">Verlauf löschen …</button>' +
            '<button type="button" data-act="settings" role="menuitem">Einstellungen …</button>' +
          '</div>' +
          '<div class="nc-screen"><div class="nc-log" role="log" aria-live="polite"></div></div>' +
          '<button type="button" class="nc-jump" hidden>↓ Neue Ereignisse</button>' +
          '<div class="nc-live" hidden><span class="nc-live-t">JackOD arbeitet</span><span class="nc-live-k">⌃C abbrechen</span></div>' +
          '<div class="nc-confirm" hidden tabindex="-1" role="alertdialog" aria-label="Bestätigung erforderlich">' +
            '<div class="nc-c-head">Bestätigung erforderlich</div><div class="nc-c-body"><span class="nc-c-lead">Noki möchte:</span><ul></ul></div>' +
            '<div class="nc-c-act"><span class="nc-c-q">Ausführen? [y/N]</span><button type="button" class="nc-btn" data-c="no">Abbrechen</button><button type="button" class="nc-btn nc-primary" data-c="yes">Ausführen</button></div>' +
          '</div>' +
          '<form class="nc-prompt"><span class="nc-caret" aria-hidden="true">›</span><textarea rows="1" spellcheck="false" autocomplete="off" aria-label="Coding-Aufgabe an JackOD" placeholder="Coding-Aufgabe beschreiben …   /help"></textarea></form>' +
          '<div class="nc-foot"><span>↵ senden</span><span>↑↓ Verlauf</span><span>⌃C abbrechen</span><span>⌘K leeren</span></div>' +
        '</div>' +
        '<div class="nc-shells-view" hidden></div>' +
        // Code Space sessions: same terminal component as the Noki Terminal.
        '<div class="nc-sessions-view" hidden></div>' +
      '</section>' +
      // CODE SPACE · Projekt: what Noki really does in a chat-started build.
      // Pure view - no terminal host, no model load.
      '<section class="nc-proj" hidden>' +
        '<div class="nc-p-bar">' +
          '<span class="nc-p-name"></span>' +
          '<span class="nc-p-state"></span>' +
        '</div>' +
        '<div class="nc-p-list" hidden><div class="nc-p-list-h">Code-Projekte · ~/Documents/Noki/Code</div><ul class="nc-p-items"></ul></div>' +
        '<div class="nc-p-main">' +
          '<div class="nc-p-meta"><div class="nc-p-model"></div><div class="nc-p-path"></div><div class="nc-p-task"></div></div>' +
          '<div class="nc-p-cols">' +
            '<div class="nc-p-col"><div class="nc-p-h">Schritte</div><ol class="nc-p-steps"></ol><div class="nc-p-h nc-p-rh" hidden>Ergebnis</div><div class="nc-p-result"></div></div>' +
            '<div class="nc-p-col nc-p-col2"><div class="nc-p-h">Dateien</div><ul class="nc-p-files"></ul><pre class="nc-p-code" hidden></pre></div>' +
          '</div>' +
          '<div class="nc-p-actions">' +
            '<button type="button" class="nc-btn nc-primary" data-act="p-vorschau">Vorschau öffnen</button>' +
            '<button type="button" class="nc-btn" data-act="p-finder">Im Finder zeigen</button>' +
            '<span class="nc-p-err"></span>' +
          '</div>' +
          '<form class="nc-p-folge"><span class="nc-caret" aria-hidden="true">›</span><textarea rows="1" spellcheck="false" aria-label="Änderung an diesem Projekt" placeholder="Änderung an diesem Projekt beschreiben … (Noki arbeitet im selben Projekt weiter)"></textarea></form>' +
        '</div>' +
      '</section>';

    var q = function (s) { return root.querySelector(s); };
    var over = q('.nc-over'), term = q('.nc-term'), log = q('.nc-log'), screen = q('.nc-screen'), input = q('.nc-prompt textarea');
    var live = q('.nc-live'), jump = q('.nc-jump'), menu = q('.nc-menu'), box = q('.nc-confirm');
    var tabsEl = q('.nc-tabs'), agentView = q('.nc-agent-view'), shellsView = q('.nc-shells-view');

    // Runtime-Sektion des Mehr-Menues: EINE Quelle (intelligence_status),
    // keine zweite Textfassung. Schlaegt der Ruf fehl, bleiben die
    // Gedankenstriche stehen — lieber nichts als etwas Falsches.
    function compactQuantization(value) {
      var match = String(value || '').match(/\b(Q\d+)\b/i);
      return match ? match[1].toUpperCase() : '';
    }
    function laufzeitMenue() {
      if (typeof call !== 'function') return;
      call('intelligence_status').then(function (st) {
        var r = st && st.runtime;
        // Code has its own last-success value. A Work response must never be
        // relabelled as Code provenance merely because it was most recent.
        var actual = st && st.active_code_runtime_model;
        var actualLabel = actual && actual.display_name ?
          ({ LOCAL: 'Local', FREE_CLOUD: 'Free Cloud', PAID_CLOUD: 'Paid Cloud' }[actual.execution_lane] || '') +
            ' · ' + actual.display_name + (compactQuantization(actual.quantization) ? ' · ' + compactQuantization(actual.quantization) : '') : '';
        var sek = q('.nc-menu-sek[data-sek="runtime"]');
        if (sek && r) {
          var setz = function (k, v) { var e = sek.querySelector('[data-r="' + k + '"]'); if (e && v) e.textContent = v; };
          setz('kind', r.active_provider_kind === 'cloud' ? 'Cloud' : 'Local');
          setz('runtime', (r.runtime || '') + (r.accelerator ? ' · ' + r.accelerator : ''));
          setz('model', actualLabel || r.active_model_label || r.active_model || '');
        }
        var overviewModel = q('.nc-o-model');
        if (overviewModel && actualLabel) overviewModel.textContent = actualLabel;
        var rtEl = (headerEl || root.parentElement || root).querySelector('.ni-runtime-status, .nc-runtime-status');
        if (rtEl && ((projSection && !projSection.hidden) || activeTab !== 'repo')) { kopfSetzen(); rtEl = null; }
        if (rtEl) {
          var kind = (r && r.active_provider_kind === 'cloud') ? 'Cloud' : 'Local';
          var rt = (r && r.runtime) ? r.runtime : 'llama.cpp';
          var acc = (r && r.accelerator) ? ' · ' + r.accelerator : ' · Metal';
          var mod = (r && (r.active_model_label || r.active_model)) ? (r.active_model_label || r.active_model) : '';
          var label = kind + ' · ' + rt + acc;
          var txt = rtEl.querySelector('.ni-runtime-text, .nc-runtime-text') || rtEl;
          if (txt) txt.textContent = label;
          rtEl.title = label + (mod ? ' (' + mod + ')' : '');
        }
      }).catch(function () {});
    }

    // ── state rendering (quiet typography, no green dots, no 'Terminal aktiv') ───
    function stateOf(s) {
      if (s.confirm_pending) return ['☐', 'Wartet auf Bestätigung', 'warn'];
      if (s.busy) return ['●', 'JackOD arbeitet', 'busy'];
      if (s.model_state === 'loading') return ['◌', 'JackOD wird geladen', 'busy'];
      if (s.model_state === 'error') return ['×', 'Modell nicht bereit', 'err'];
      if (s.terminal || s.host) return ['○', 'Aktiv', 'idle'];
      return ['○', 'Bereit', 'idle'];
    }
    function setState(node, st) { node.textContent = st[0] + ' ' + st[1]; node.dataset.k = st[2]; }
    function renderOverview() {
      var s = status || {};
      var st = stateOf(s);
      setState(q('.nc-o-state'), st);
      q('.nc-o-model').textContent = 'JackOD 9B Coder · Local · Q4';
      laufzeitMenue();
      q('.nc-o-style').textContent = styleName(s.style);
      q('.nc-o-project').textContent = s.project || '~/NOKI';
      var r = RESULT[s.last_result];
      var res = q('.nc-o-result');
      res.textContent = r ? r[0] + ' ' + r[1] : '—';
      res.dataset.k = r ? r[2] : '';
      q('.nc-over [data-act="here"]').textContent = (s.terminal || s.host) ? 'Hier anzeigen' : 'Terminal hier öffnen';
    }
    function renderBar() {
      var m = meta || {};
      if (activeTab !== 'repo') { modusSichtbar(); live.hidden = true; return; }
      // The picked mode stays picked. A session message that carries no style
      // (or arrives before the host echoed the switch) must never silently
      // reset the selection back to "Funktional".
      var curStyle = m.style || gewaehlterStil || 'functional';
      gewaehlterStil = curStyle;
      root.dataset.codeStyle = curStyle;
      // The picker sits in the tab bar inside `root`; `headerEl` is the panel
      // header and holds none of these buttons. Querying only the header left
      // BOTH buttons unmarked, so no mode ever looked selected. Mark every
      // instance, wherever it is mounted.
      [root, headerEl].forEach(function (scope) {
        if (!scope) return;
        scope.querySelectorAll('.nc-seg button, .ni-code-styles button').forEach(function (b) {
          var on = b.dataset.style === curStyle;
          b.classList.toggle('on', on); b.setAttribute('aria-checked', String(on));
        });
      });
      if (input) {
        input.placeholder = curStyle === 'creative'
          ? 'Coding-Aufgabe (Kreativ · freier Entwurf) …   /help'
          : 'Coding-Aufgabe (Funktional · strikt & minimal) …   /help';
      }
      live.hidden = !m.busy;
    }
    function syncStatus(m) {
      status.style = m.style; status.busy = m.busy; status.last_result = m.last_result || '';
      status.host = true; status.terminal = m.terminal_views > 0; status.embedded = m.embedded_views > 0;
      status.project = m.project; status.session_id = m.session_id; status.model_state = m.model_state; status.confirm_pending = m.confirm_pending;
      onStatus(status);
      renderOverview();
    }

    // ── multi-terminal tabs manager ──────────────────────────────────
    // Code Space tabs are TERMINALS: "Noki Terminal" (permanent) and
    // "Terminal N" (closable). What runs inside (zsh, claude, antigravity,
    // noki code) is the user's choice - a tab never starts an agent.
    var NOKI_TERM = 'noki';   // Noki's own terminal (not a shell)
    var nokiCode = {};   // terminal id -> running `noki code` session
    function renderTabs() {
      tabsEl.textContent = '';
      var nt = el('button', 'nc-tab' + (activeTab === NOKI_TERM ? ' active' : ''));
      nt.type = 'button'; nt.setAttribute('role', 'tab'); nt.dataset.tab = NOKI_TERM;
      nt.appendChild(el('span', null, 'Noki Terminal'));
      var nsz = sitzung(NOKI_TERM);
      if (nsz && nsz.laeuft) { var tr = el('span', 'nc-ring nc-ring-tab'); tr.setAttribute('aria-label', 'läuft'); nt.appendChild(tr); }
      tabsEl.appendChild(nt);
      shellTabs.forEach(function (s) {
        var tabBtn = el('button', 'nc-tab' + (activeTab === s.id ? ' active' : ''));
        tabBtn.type = 'button';
        tabBtn.setAttribute('role', 'tab');
        tabBtn.dataset.tab = s.id;
        tabBtn.appendChild(el('span', null, s.title));
        if (s.id !== NOKI_TERM) {
          var closeBtn = el('span', 'nc-tab-close', '×');
          closeBtn.dataset.close = s.id;
          closeBtn.title = 'Terminal schließen';
          tabBtn.appendChild(closeBtn);
        }
        tabsEl.appendChild(tabBtn);
      });
      if (repoOffen) {
        var agentBtn = el('button', 'nc-tab' + (activeTab === 'repo' ? ' active' : ''));
        agentBtn.type = 'button';
        agentBtn.setAttribute('role', 'tab');
        agentBtn.dataset.tab = 'repo';
        agentBtn.appendChild(el('span', null, 'Repo-Agent'));
        tabsEl.appendChild(agentBtn);
      }
      var addBtn = el('button', 'nc-tab-add', '+');
      addBtn.type = 'button';
      addBtn.dataset.act = 'add-terminal';
      addBtn.title = 'Neues Terminal';
      addBtn.setAttribute('aria-label', 'Neues Terminal öffnen');
      tabsEl.appendChild(addBtn);
      modusSichtbar();
    }
    function naechsterTitel() {
      for (var n = 1; n < 100; n++) {
        if (!shellTabs.some(function (s) { return s.title === 'Terminal ' + n; })) return 'Terminal ' + n;
      }
      return 'Terminal';
    }
    // The mode buttons belong to a running Noki Code session - only then are
    // they shown, and they show THAT session's mode.
    function modusSichtbar() {
      var nsz = activeTab === NOKI_TERM ? sitzung(NOKI_TERM) : null;
      var nc = nsz ? { sitzung: NOKI_TERM, modus: nsz.modus } : nokiCode[activeTab];
      [root, headerEl].forEach(function (scope) {
        if (!scope) return;
        scope.querySelectorAll('.nc-seg, .ni-code-styles').forEach(function (seg) { seg.hidden = !nc; });
        if (nc) scope.querySelectorAll('.nc-seg button, .ni-code-styles button').forEach(function (b) {
          var on = b.dataset.style === nc.modus;
          b.classList.toggle('on', on); b.setAttribute('aria-checked', String(on));
        });
      });
      kopfSetzen();
    }
    function nokiTerminal() {
      if (shellTabs.some(function (s) { return s.id === NOKI_TERM; })) return Promise.resolve();
      var size = measureCharSize(shellsView.clientWidth > 40 ? shellsView : root);
      var cols = Math.max(40, Math.floor(((shellsView.clientWidth || 900) - 20) / (size.w || 7.55)));
      var rows = Math.max(10, Math.floor(((shellsView.clientHeight || 500) - 8) / (size.h || 18)));
      // Pane first, so neither the welcome visual nor the first prompt is lost.
      var shell = createShellPane(NOKI_TERM, 'Noki Terminal', '~', cols, rows);
      shell.feed('\x1b[38;5;180m' + ASCII_WORDMARK.replace(/\n/g, '\r\n') + '\x1b[0m\r\n' +
        '\x1b[2mEin normales Terminal. Starte Programme wie gewohnt: noki code · claude · antigravity · git …\x1b[0m\r\n\r\n');
      shellTabs.unshift(shell);
      shellsView.appendChild(shell.paneEl);
      return call('intelligence_shell_spawn', { id: NOKI_TERM, title: 'Noki Terminal', cwd: '~', cols: cols, rows: rows })
        .catch(function (err) { shell.feed('\r\nShell konnte nicht gestartet werden: ' + String(err) + '\r\n'); });
    }

    function switchTab(tabId) {
      if (tabId !== 'repo' && tabId !== NOKI_TERM && !shellTabs.some(function (s) { return s.id === tabId; })) tabId = NOKI_TERM;
      // Isolation: the previous terminal's input gives up focus explicitly.
      if (tabId !== activeTab && document.activeElement && root.contains(document.activeElement)) document.activeElement.blur();
      var prevShell = shellTabs.find(function (s) { return s.id === activeTab; });
      if (prevShell && prevShell.saveScroll) prevShell.saveScroll();
      activeTab = tabId;
      renderTabs();
      var nokiAn = tabId === NOKI_TERM;
      if (sessionsView) sessionsView.hidden = !nokiAn;
      var nsz = sitzung(NOKI_TERM);
      if (nsz && nsz.pane) nsz.pane.hidden = !nokiAn;
      if (nokiAn) {
        agentView.hidden = true;
        shellsView.hidden = true;
        shellTabs.forEach(function (s) { s.paneEl.hidden = true; });
        if (nsz) { if (nsz.stick) nsz.screen.scrollTop = nsz.screen.scrollHeight; setTimeout(function () { nsz.input.focus(); }, 10); }
        kopfSetzen();
      } else if (tabId === 'repo') {
        agentView.hidden = false;
        shellsView.hidden = true;
        shellTabs.forEach(function (s) { s.paneEl.hidden = true; });
        // The repo agent (JackOD on ~/NOKI) connects only when opened.
        if (!attached) { clearView(true); note('Verbinde mit der Repo-Session …'); attach(); }
        setTimeout(function () { input.focus(); }, 10);
      } else {
        agentView.hidden = true;
        shellsView.hidden = false;
        shellTabs.forEach(function (s) {
          var show = s.id === tabId;
          s.paneEl.hidden = !show;
          if (show) {
            if (s.fit) s.fit();
            if (s.restoreScroll) s.restoreScroll();
            s.focus();
          }
        });
      }
    }

    function spawnShellTab(title, cwd) {
      shellCount++;
      var sTitle = title || naechsterTitel();
      var sId = 'shell-' + shellCount + '-' + Date.now().toString(36);
      var sCwd = cwd || '~';
      var shell = createShellPane(sId, sTitle, sCwd, 80, 24);
      shellTabs.push(shell);
      shellsView.appendChild(shell.paneEl);
      switchTab(sId);
      // Measure first, then start zsh with exactly that size. Starting it
      // with an estimate made zsh draw its partial-line mark ("%" + spaces
      // to the line end) for a wider line than the emulator had: the mark
      // wrapped and stayed as an orphan "%" at the top.
      var g = shell.groesse();
      return call('intelligence_shell_spawn', { id: sId, title: sTitle, cwd: sCwd, cols: g.cols, rows: g.rows })
        .then(function () {
          var n = shell.groesse();
          if (n.cols !== g.cols || n.rows !== g.rows) call('intelligence_shell_resize', { id: sId, cols: n.cols, rows: n.rows }).catch(function () {});
          return shell;
        })
        .catch(function (err) { shell.feed('Shell konnte nicht gestartet werden: ' + String(err) + '\r\n'); });
    }

    function closeShellTab(id) {
      var idx = -1;
      for (var i = 0; i < shellTabs.length; i++) {
        if (shellTabs[i].id === id) { idx = i; break; }
      }
      if (idx < 0) return;

      var shell = shellTabs[idx];
      call('intelligence_shell_close', { id: id }).catch(function () {});
      shell.destroy();
      shellTabs.splice(idx, 1);

      if (activeTab === id) {
        if (idx > 0) {
          switchTab(shellTabs[idx - 1].id);
        } else if (shellTabs.length > 0) {
          switchTab(shellTabs[0].id);
        } else {
          switchTab(NOKI_TERM);
        }
      } else {
        renderTabs();
      }
    }

    // ── Unicode character cell width (POSIX wcwidth implementation) ─
    function wcwidth(cp) {
      if (cp === 0 || cp === 0x07 || cp === 0x08 || cp === 0x09 || cp === 0x0a || cp === 0x0d) return 0;
      if ((cp >= 0 && cp < 32) || (cp >= 0x7f && cp < 0xa0)) return 0;
      // Combining characters & zero-width marks
      if (cp >= 0x0300 && cp <= 0x036f) return 0;
      if (cp >= 0x0483 && cp <= 0x0489) return 0;
      if (cp >= 0x0591 && cp <= 0x05bd) return 0;
      if (cp >= 0x05bf && cp <= 0x05c7) return 0;
      if (cp >= 0x0610 && cp <= 0x061a) return 0;
      if (cp >= 0x064b && cp <= 0x065f) return 0;
      if (cp >= 0x0670 && cp <= 0x06ed) return 0;
      if (cp >= 0x0711 && cp <= 0x074a) return 0;
      if (cp >= 0x0b82 && cp <= 0x0bc8) return 0;
      if (cp >= 0x0e31 && cp <= 0x0e3a) return 0;
      if (cp >= 0x0eb1 && cp <= 0x0ebc) return 0;
      if (cp >= 0x17b4 && cp <= 0x17d3) return 0;
      if (cp >= 0x180b && cp <= 0x180f) return 0;
      if (cp >= 0x1dc0 && cp <= 0x1dff) return 0;
      if (cp >= 0x200b && cp <= 0x200f) return 0; // ZWSP, ZWNJ, ZWJ, LRM, RLM
      if (cp >= 0x202a && cp <= 0x202e) return 0;
      if (cp >= 0x2060 && cp <= 0x206f) return 0;
      if (cp >= 0x20d0 && cp <= 0x20ff) return 0;
      if (cp >= 0xfe00 && cp <= 0xfe0f) return 0; // Variation selectors
      if (cp >= 0xfe20 && cp <= 0xfe2f) return 0;
      if (cp >= 0xe0100 && cp <= 0xe01ef) return 0;

      // Wide characters (East Asian Wide / Fullwidth / Emoji)
      if (cp >= 0x1100 && cp <= 0x115f) return 2; // Hangul Jamo
      if (cp >= 0x2329 && cp <= 0x232a) return 2;
      if (cp >= 0x2e80 && cp <= 0x303e) return 2; // CJK Radicals, Punctuation
      if (cp >= 0x3040 && cp <= 0xa4cf) return 2; // Hiragana, Katakana, CJK Ideographs
      if (cp >= 0xac00 && cp <= 0xd7a3) return 2; // Hangul Syllables
      if (cp >= 0xf900 && cp <= 0xfaff) return 2; // CJK Compatibility Ideographs
      if (cp >= 0xfe10 && cp <= 0xfe19) return 2; // Vertical forms
      if (cp >= 0xfe30 && cp <= 0xfe6f) return 2; // CJK Compatibility Forms
      if (cp >= 0xff00 && cp <= 0xff60) return 2; // Fullwidth Forms
      if (cp >= 0xffe0 && cp <= 0xffe6) return 2;
      if (cp >= 0x1f300 && cp <= 0x1f64f) return 2; // Misc Symbols, Emoji
      if (cp >= 0x1f680 && cp <= 0x1f6ff) return 2; // Transport & Map
      if (cp >= 0x1f900 && cp <= 0x1f9ff) return 2; // Supplemental Symbols & Pictographs
      if (cp >= 0x1fa70 && cp <= 0x1faff) return 2; // Symbols & Pictographs Extended-A
      if (cp >= 0x20000 && cp <= 0x3fffd) return 2; // CJK Extensions

      return 1;
    }

    function measureCharSize(container) {
      var probe = document.createElement('div');
      probe.className = 'nc-shell-line';
      probe.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;font-family:var(--nc-mono, monospace);font-size:12px;';
      probe.textContent = 'WWWWWWWWWW';
      container.appendChild(probe);
      var rect = probe.getBoundingClientRect();
      var w = (rect.width > 0) ? (rect.width / 10) : 7.55;
      var h = (rect.height > 0) ? rect.height : 18;
      probe.remove();
      return { w: w, h: h };
    }

    // ── real shell terminal pane & ANSI engine ───────────────────────
    function createShellPane(id, title, cwd, initialCols, initialRows) {
      var paneEl = el('div', 'nc-shell-pane blur');
      paneEl.dataset.shellId = id;
      paneEl.setAttribute('tabindex', '0');

      var screenEl = el('div', 'nc-shell-screen');
      screenEl.style.overflowY = 'auto';
      var linesEl = el('div', 'nc-shell-lines');
      screenEl.appendChild(linesEl);
      paneEl.appendChild(screenEl);

      var hiddenInput = document.createElement('textarea');
      hiddenInput.className = 'nc-shell-hidden-input';
      hiddenInput.setAttribute('aria-label', title || 'Terminal');
      hiddenInput.setAttribute('autocomplete', 'off');
      hiddenInput.setAttribute('autocorrect', 'off');
      hiddenInput.setAttribute('autocapitalize', 'off');
      hiddenInput.setAttribute('spellcheck', 'false');
      hiddenInput.style.cssText = 'position:fixed;left:-9999px;top:-9999px;opacity:0;pointer-events:none;width:1px;height:1px;';
      paneEl.appendChild(hiddenInput);

      // Sizing
      var cols = initialCols || 80, rows = initialRows || 24;
      var charW = 7.55, charH = 18;

      // Terminal state:
      // True terminal emulation separates scrollback from the screen grid.
      // screenGrid has a fixed height of `rows`.
      // Cursor coordinates (0 to cols-1, 0 to rows-1) address cells within screenGrid.
      // Lines pushed off the top margin move into scrollback.
      var scrollback = [];
      var altScrollback = [];
      var screenGrid = [];
      for (var r = 0; r < rows; r++) screenGrid.push([]);

      var altGrid = [];
      for (var r = 0; r < rows; r++) altGrid.push([]);
      var isAlt = false;

      var cursorX = 0, cursorY = 0;
      var savedMainCursor = { x: 0, y: 0, fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false };
      var savedAltCursor = { x: 0, y: 0, fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false };

      var curFg = null, curBg = null, curBold = false, curDim = false, curItalic = false, curUnderline = false, curInverse = false;
      var cursorVisible = true;
      var originMode = false;
      var autoWrap = true;
      var bracketedPaste = false;

      var scrollTopMargin = 0;
      var scrollBottomMargin = rows - 1;

      var renderScheduled = false;
      var savedMainScrollTop = 0;
      var pendingEsc = '';

      var autoFollow = true;
      var userScrolledUp = false;
      var pendingScrollbackLines = 0;

      function activeBuffer() {
        return isAlt ? altGrid : screenGrid;
      }

      function ensureLine(buf, y) {
        while (buf.length <= y) {
          buf.push([]);
        }
        return buf[y];
      }

      function saveCursor() {
        var target = isAlt ? savedAltCursor : savedMainCursor;
        target.x = cursorX;
        target.y = cursorY;
        target.fg = curFg;
        target.bg = curBg;
        target.bold = curBold;
        target.dim = curDim;
        target.italic = curItalic;
        target.underline = curUnderline;
        target.inverse = curInverse;
      }

      function restoreCursor() {
        var src = isAlt ? savedAltCursor : savedMainCursor;
        cursorX = Math.max(0, Math.min(cols - 1, src.x));
        cursorY = Math.max(0, Math.min(rows - 1, src.y));
        curFg = src.fg;
        curBg = src.bg;
        curBold = src.bold;
        curDim = src.dim;
        curItalic = src.italic;
        curUnderline = src.underline;
        curInverse = src.inverse;
      }

      function enterAltScreen() {
        if (isAlt) return;
        saveCursor();
        isAlt = true;
        altGrid = [];
        for (var r = 0; r < rows; r++) altGrid.push([]);
        altScrollback = [];
        cursorX = 0;
        cursorY = 0;
        scrollTopMargin = 0;
        scrollBottomMargin = rows - 1;
        screenEl.style.overflowY = 'auto';
        autoFollow = true;
        userScrolledUp = false;
        pendingScrollbackLines = 0;
        scheduleRender();
      }

      function exitAltScreen() {
        if (!isAlt) return;
        isAlt = false;
        altGrid = [];
        altScrollback = [];
        restoreCursor();
        scrollTopMargin = 0;
        scrollBottomMargin = rows - 1;
        screenEl.style.overflowY = 'auto';
        pendingScrollbackLines = 0;
        scheduleRender();
      }

      function scrollRegionUp() {
        var buf = activeBuffer();
        ensureLine(buf, scrollBottomMargin);
        var popped = buf.splice(scrollTopMargin, 1)[0] || [];
        buf.splice(scrollBottomMargin, 0, []);
        if (scrollTopMargin === 0) {
          var targetSb = isAlt ? altScrollback : scrollback;
          targetSb.push(popped);
          var maxScrollback = 5000;
          if (targetSb.length > maxScrollback) {
            targetSb.splice(0, targetSb.length - maxScrollback);
          }
          if (!autoFollow) {
            pendingScrollbackLines++;
          }
        }
      }

      function scrollRegionDown() {
        var buf = activeBuffer();
        ensureLine(buf, scrollBottomMargin);
        buf.splice(scrollBottomMargin, 1);
        buf.splice(scrollTopMargin, 0, []);
      }

      function scheduleRender() {
        if (renderScheduled) return;
        renderScheduled = true;
        requestAnimationFrame(function () {
          renderScheduled = false;
          renderBuffer();
        });
      }

      function renderBuffer() {
        var buf = activeBuffer();
        while (buf.length < rows) buf.push([]);
        while (buf.length > rows) buf.pop();

        var wasAtBottom = autoFollow && !userScrolledUp;
        var prevScrollTop = screenEl.scrollTop;
        var addedLines = pendingScrollbackLines;
        pendingScrollbackLines = 0;

        var sb = isAlt ? (scrollback.concat(altScrollback)) : scrollback;
        var totalCount = sb.length + rows;
        var cursorGlobalY = sb.length + cursorY;

        var frag = document.createDocumentFragment();

        for (var idx = 0; idx < totalCount; idx++) {
          var line = (idx >= sb.length)
            ? buf[idx - sb.length]
            : sb[idx];
          var lineDiv = el('div', 'nc-shell-line');
          var isCursorLine = (idx === cursorGlobalY && cursorVisible);

          if ((!line || !line.length) && !isCursorLine) {
            lineDiv.innerHTML = '&nbsp;';
          } else {
            var lineArr = line || [];
            var span = null;
            var spanKey = '';
            var maxCol = Math.max(lineArr.length, isCursorLine ? cursorX + 1 : 0);

            for (var c = 0; c < maxCol; c++) {
              var cell = lineArr[c] || { ch: ' ', width: 1, fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false };
              if (cell.isContinuation) continue;

              var atCursor = (isCursorLine && c === cursorX);
              if (atCursor) {
                if (span) { lineDiv.appendChild(span); span = null; spanKey = ''; }
                var cursorEl = el('span', 'nc-shell-cursor');
                if (cell.width === 2) cursorEl.classList.add('nc-wide');
                var cellChar = cell.ch || ' ';
                cursorEl.textContent = cellChar === ' ' ? '\u00a0' : cellChar;
                lineDiv.appendChild(cursorEl);
                continue;
              }

              var effectiveFg = cell.inverse ? (cell.bg || '#1c1d1f') : cell.fg;
              var effectiveBg = cell.inverse ? (cell.fg || '#e8e7e1') : cell.bg;

              var key = (effectiveFg || '') + '|' + (effectiveBg || '') + '|' + (cell.bold ? 'b' : '') + (cell.dim ? 'd' : '') + (cell.italic ? 'i' : '') + (cell.underline ? 'u' : '') + '|' + (cell.width || 1);

              if (!span || spanKey !== key) {
                if (span) lineDiv.appendChild(span);
                span = document.createElement('span');
                spanKey = key;
                if (cell.width === 2) span.className = 'nc-wide';
                if (effectiveFg) {
                  if (effectiveFg.charAt(0) === '#') span.style.color = effectiveFg;
                  else if (/^ansi-/.test(effectiveFg)) span.classList.add(effectiveFg);
                  else span.style.color = effectiveFg;
                }
                if (effectiveBg) {
                  if (effectiveBg.charAt(0) === '#') span.style.backgroundColor = effectiveBg;
                  else if (/^ansi-/.test(effectiveBg)) span.classList.add(effectiveBg);
                  else span.style.backgroundColor = effectiveBg;
                }
                if (cell.bold) span.classList.add('ansi-bold');
                if (cell.dim) span.classList.add('ansi-dim');
                if (cell.italic) span.classList.add('ansi-italic');
                if (cell.underline) span.classList.add('ansi-underline');
              }
              span.textContent += (cell.ch || ' ');
            }
            if (span) lineDiv.appendChild(span);
          }
          frag.appendChild(lineDiv);
        }

        linesEl.replaceChildren(frag);

        if (wasAtBottom) {
          screenEl.scrollTop = screenEl.scrollHeight;
        } else {
          var targetScrollTop = prevScrollTop + (addedLines * charH);
          screenEl.scrollTop = targetScrollTop;
        }
      }

      function handleEsc2(cmd) {
        switch (cmd) {
          case 'M': // Reverse Index (RI) - move cursor up, scroll down if at top
            if (cursorY <= scrollTopMargin) {
              scrollRegionDown();
            } else {
              cursorY = Math.max(0, cursorY - 1);
            }
            break;
          case 'D': // Index (IND) - move cursor down, scroll up if at bottom
            var bot = (cursorY <= scrollBottomMargin) ? scrollBottomMargin : rows - 1;
            if (cursorY >= bot) {
              scrollRegionUp();
            } else {
              cursorY = Math.min(rows - 1, cursorY + 1);
            }
            break;
          case 'E': // Next Line (NEL)
            cursorX = 0;
            var bot = (cursorY <= scrollBottomMargin) ? scrollBottomMargin : rows - 1;
            if (cursorY >= bot) {
              scrollRegionUp();
            } else {
              cursorY = Math.min(rows - 1, cursorY + 1);
            }
            break;
          case '7': // Save Cursor
            saveCursor();
            break;
          case '8': // Restore Cursor
            restoreCursor();
            break;
          case 'c': // RIS (Reset)
            curFg = null; curBg = null; curBold = false; curDim = false; curItalic = false; curUnderline = false; curInverse = false;
            cursorX = 0; cursorY = 0;
            originMode = false; autoWrap = true;
            scrollTopMargin = 0; scrollBottomMargin = rows - 1;
            if (isAlt) exitAltScreen();
            scrollback = [];
            screenGrid = [];
            for (var r = 0; r < rows; r++) screenGrid.push([]);
            break;
          default:
            break;
        }
      }

      function handleOSC(content) {
        var semi = content.indexOf(';');
        if (semi > 0) {
          var type = content.slice(0, semi);
          var val = content.slice(semi + 1);
          if (type === '0' || type === '2') {
            if (val && val.trim()) {
              title = val.trim();
              renderTabs();
            }
          }
        }
      }

      function handleCSI(cmd, params) {
        var isPrivate = params.startsWith('?');
        var rawParams = isPrivate ? params.slice(1) : params;
        var args = rawParams ? rawParams.split(';').map(function (p) { return parseInt(p, 10); }) : [0];
        if (isNaN(args[0])) args[0] = 0;

        var buf = activeBuffer();

        switch (cmd) {
          case 'm': // SGR
            if (!params || params === '0') {
              curFg = null; curBg = null; curBold = false; curDim = false; curItalic = false; curUnderline = false; curInverse = false;
              break;
            }
            var p = 0;
            while (p < args.length) {
              var code = args[p];
              if (code === 0) {
                curFg = null; curBg = null; curBold = false; curDim = false; curItalic = false; curUnderline = false; curInverse = false;
              } else if (code === 1) { curBold = true; }
              else if (code === 2) { curDim = true; }
              else if (code === 3) { curItalic = true; }
              else if (code === 4) { curUnderline = true; }
              else if (code === 7) { curInverse = true; }
              else if (code === 22) { curBold = false; curDim = false; }
              else if (code === 23) { curItalic = false; }
              else if (code === 24) { curUnderline = false; }
              else if (code === 27) { curInverse = false; }
              else if (code >= 30 && code <= 37) {
                var fgs = ['ansi-black', 'ansi-red', 'ansi-green', 'ansi-yellow', 'ansi-blue', 'ansi-magenta', 'ansi-cyan', 'ansi-white'];
                curFg = fgs[code - 30];
              } else if (code === 39) {
                curFg = null;
              } else if (code >= 40 && code <= 47) {
                var bgs = ['ansi-bg-black', 'ansi-bg-red', 'ansi-bg-green', 'ansi-bg-yellow', 'ansi-bg-blue', 'ansi-bg-magenta', 'ansi-bg-cyan', 'ansi-bg-white'];
                curBg = bgs[code - 40];
              } else if (code === 49) {
                curBg = null;
              } else if (code >= 90 && code <= 97) {
                var bfgs = ['ansi-bright-black', 'ansi-bright-red', 'ansi-bright-green', 'ansi-bright-yellow', 'ansi-bright-blue', 'ansi-bright-magenta', 'ansi-bright-cyan', 'ansi-bright-white'];
                curFg = bfgs[code - 90];
              } else if (code >= 100 && code <= 107) {
                var bbgs = ['ansi-bg-bright-black', 'ansi-bg-bright-red', 'ansi-bg-bright-green', 'ansi-bg-bright-yellow', 'ansi-bg-bright-blue', 'ansi-bg-bright-magenta', 'ansi-bg-bright-cyan', 'ansi-bg-bright-white'];
                curBg = bbgs[code - 100];
              } else if (code === 38 && args[p + 1] === 5) {
                curFg = color256(args[p + 2] || 0);
                p += 2;
              } else if (code === 48 && args[p + 1] === 5) {
                curBg = color256(args[p + 2] || 0);
                p += 2;
              } else if (code === 38 && args[p + 1] === 2) {
                curFg = 'rgb(' + (args[p + 2] || 0) + ',' + (args[p + 3] || 0) + ',' + (args[p + 4] || 0) + ')';
                p += 4;
              } else if (code === 48 && args[p + 1] === 2) {
                curBg = 'rgb(' + (args[p + 2] || 0) + ',' + (args[p + 3] || 0) + ',' + (args[p + 4] || 0) + ')';
                p += 4;
              }
              p++;
            }
            break;

          case 'A': // CUU (Cursor Up)
            var n = args[0] || 1;
            var top = (cursorY >= scrollTopMargin) ? scrollTopMargin : 0;
            cursorY = Math.max(top, cursorY - n);
            break;

          case 'B': // CUD (Cursor Down)
            var n = args[0] || 1;
            var bot = (cursorY <= scrollBottomMargin) ? scrollBottomMargin : rows - 1;
            cursorY = Math.min(bot, cursorY + n);
            ensureLine(buf, cursorY);
            break;

          case 'C': // CUF (Cursor Forward)
            cursorX = Math.min(cols - 1, cursorX + (args[0] || 1));
            break;

          case 'D': // CUB (Cursor Back)
            cursorX = Math.max(0, cursorX - (args[0] || 1));
            break;

          case 'E': // CNL (Cursor Next Line)
            cursorX = 0;
            var bot = (cursorY <= scrollBottomMargin) ? scrollBottomMargin : rows - 1;
            cursorY = Math.min(bot, cursorY + (args[0] || 1));
            ensureLine(buf, cursorY);
            break;

          case 'F': // CPL (Cursor Previous Line)
            cursorX = 0;
            var top = (cursorY >= scrollTopMargin) ? scrollTopMargin : 0;
            cursorY = Math.max(top, cursorY - (args[0] || 1));
            break;

          case 'G': // CHA (Cursor Horizontal Absolute)
            cursorX = Math.max(0, Math.min(cols - 1, (args[0] || 1) - 1));
            break;

          case 'H': // CUP
          case 'f': // HVP
            var r = (args[0] || 1) - 1;
            var c = (args[1] || 1) - 1;
            if (originMode) r += scrollTopMargin;
            cursorY = Math.max(0, Math.min(rows - 1, r));
            cursorX = Math.max(0, Math.min(cols - 1, c));
            ensureLine(buf, cursorY);
            break;

          case 'd': // VPA (Line Position Absolute)
            var r = (args[0] || 1) - 1;
            if (originMode) r += scrollTopMargin;
            cursorY = Math.max(0, Math.min(rows - 1, r));
            ensureLine(buf, cursorY);
            break;

          case 'J': // ED (Erase in Display)
            var modeJ = args[0] || 0;
            if (modeJ === 0) {
              if (buf[cursorY]) buf[cursorY].splice(cursorX);
              for (var row = cursorY + 1; row < rows; row++) buf[row] = [];
            } else if (modeJ === 1) {
              for (var row = 0; row < cursorY && row < rows; row++) buf[row] = [];
              if (buf[cursorY]) {
                for (var col = 0; col <= cursorX && col < buf[cursorY].length; col++) {
                  buf[cursorY][col] = { ch: ' ', width: 1 };
                }
              }
            } else if (modeJ === 2 || modeJ === 3) {
              for (var row = 0; row < rows; row++) buf[row] = [];
              if (modeJ === 3 && !isAlt) {
                scrollback = [];
              }
            }
            break;

          case 'K': // EL (Erase in Line)
            var modeK = args[0] || 0;
            var curLine = ensureLine(buf, cursorY);
            if (modeK === 0) {
              curLine.splice(cursorX);
            } else if (modeK === 1) {
              for (var k = 0; k <= cursorX && k < curLine.length; k++) {
                curLine[k] = { ch: ' ', width: 1 };
              }
            } else if (modeK === 2) {
              buf[cursorY] = [];
            }
            break;

          case 'L': // IL (Insert Line)
            var n = args[0] || 1;
            for (var k = 0; k < n; k++) {
              if (cursorY <= scrollBottomMargin) {
                buf.splice(scrollBottomMargin, 1);
                buf.splice(cursorY, 0, []);
              }
            }
            break;

          case 'M': // DL (Delete Line)
            var n = args[0] || 1;
            for (var k = 0; k < n; k++) {
              if (cursorY <= scrollBottomMargin) {
                buf.splice(cursorY, 1);
                buf.splice(scrollBottomMargin, 0, []);
              }
            }
            break;

          case 'P': // DCH (Delete Character)
            var n = args[0] || 1;
            var curLine = ensureLine(buf, cursorY);
            curLine.splice(cursorX, n);
            break;

          case '@': // ICH (Insert Character)
            var n = args[0] || 1;
            var curLine = ensureLine(buf, cursorY);
            var blanks = [];
            for (var k = 0; k < n; k++) blanks.push({ ch: ' ', width: 1 });
            curLine.splice.apply(curLine, [cursorX, 0].concat(blanks));
            if (curLine.length > cols) curLine.length = cols;
            break;

          case 'X': // ECH (Erase Character)
            var n = args[0] || 1;
            var curLine = ensureLine(buf, cursorY);
            for (var k = 0; k < n && (cursorX + k) < cols; k++) {
              curLine[cursorX + k] = { ch: ' ', width: 1 };
            }
            break;

          case 'r': // DECSTBM (Set Top and Bottom Margins)
            var top = (args[0] || 1) - 1;
            var bot = args[1] ? (args[1] - 1) : (rows - 1);
            scrollTopMargin = Math.max(0, Math.min(rows - 1, top));
            scrollBottomMargin = Math.max(scrollTopMargin, Math.min(rows - 1, bot));
            cursorX = 0;
            cursorY = originMode ? scrollTopMargin : 0;
            break;

          case 's': // SCP (Save Cursor)
            saveCursor();
            break;

          case 'u': // RCP (Restore Cursor) or Kitty keyboard protocol
            if (params && (params.indexOf('?') !== -1 || params.indexOf('=') !== -1 || params.indexOf('<') !== -1 || params.indexOf('>') !== -1 || params.indexOf(';') !== -1)) {
              if (params === '?') {
                sendShell('\x1b[?0u');
              }
              break;
            }
            restoreCursor();
            break;

          case 'n': // DSR (Device Status Report)
            if (args[0] === 6) {
              // Cursor Position Report (CPR): 1-indexed row and col
              sendShell('\x1b[' + (cursorY + 1) + ';' + (cursorX + 1) + 'R');
            } else if (args[0] === 5) {
              sendShell('\x1b[0n');
            }
            break;

          case 'c': // DA1 (Device Attributes)
            if (!params || params === '' || params === '0') {
              sendShell('\x1b[?62;c');
            }
            break;

          case 'h': // SM / DECSET
            for (var p = 0; p < args.length; p++) {
              var m = args[p];
              if (isPrivate) {
                if (m === 1049 || m === 1047 || m === 47) enterAltScreen();
                else if (m === 25) cursorVisible = true;
                else if (m === 6) { originMode = true; cursorX = 0; cursorY = scrollTopMargin; }
                else if (m === 7) autoWrap = true;
                else if (m === 2004) bracketedPaste = true;
              }
            }
            break;

          case 'l': // RM / DECRST
            for (var p = 0; p < args.length; p++) {
              var m = args[p];
              if (isPrivate) {
                if (m === 1049 || m === 1047 || m === 47) exitAltScreen();
                else if (m === 25) cursorVisible = false;
                else if (m === 6) { originMode = false; cursorX = 0; cursorY = 0; }
                else if (m === 7) autoWrap = false;
                else if (m === 2004) bracketedPaste = false;
              }
            }
            break;
        }
      }

      function feed(incoming) {
        var data = pendingEsc ? (pendingEsc + incoming) : incoming;
        pendingEsc = '';

        var i = 0, len = data.length;
        while (i < len) {
          var ch = data[i];

          // 1. Escape sequences
          if (ch === '\x1b') {
            if (i + 1 >= len) {
              pendingEsc = '\x1b';
              break;
            }
            var next = data[i + 1];

            if (next === '[') {
              var j = i + 2;
              while (j < len && (data.charCodeAt(j) < 0x40 || data.charCodeAt(j) > 0x7e)) {
                j++;
              }
              if (j < len) {
                var finalChar = data[j];
                var params = data.slice(i + 2, j);
                handleCSI(finalChar, params);
                i = j + 1;
                continue;
              } else {
                pendingEsc = data.slice(i);
                break;
              }
            } else if (next === ']') {
              var j = i + 2;
              while (j < len && data[j] !== '\x07' && !(data[j] === '\x1b' && j + 1 < len && data[j + 1] === '\\')) {
                j++;
              }
              if (j < len) {
                var oscContent = data.slice(i + 2, j);
                handleOSC(oscContent);
                i = (data[j] === '\x07') ? j + 1 : j + 2;
                continue;
              } else {
                pendingEsc = data.slice(i);
                break;
              }
            } else if (next === '(' || next === ')' || next === '*' || next === '+') {
              if (i + 2 < len) {
                i += 3;
                continue;
              } else {
                pendingEsc = data.slice(i);
                break;
              }
            } else {
              handleEsc2(next);
              i += 2;
              continue;
            }
          }

          // 2. Control characters
          if (ch === '\r') {
            cursorX = 0;
            i++;
            continue;
          }
          if (ch === '\n') {
            var bot = (cursorY <= scrollBottomMargin) ? scrollBottomMargin : rows - 1;
            if (cursorY >= bot) {
              scrollRegionUp();
            } else {
              cursorY++;
            }
            ensureLine(activeBuffer(), cursorY);
            i++;
            continue;
          }
          if (ch === '\b') {
            cursorX = Math.max(0, cursorX - 1);
            i++;
            continue;
          }
          if (ch === '\t') {
            var tabStop = (Math.floor(cursorX / 8) + 1) * 8;
            var l = ensureLine(activeBuffer(), cursorY);
            while (cursorX < tabStop && cursorX < cols) {
              l[cursorX] = { ch: ' ', width: 1, fg: curFg, bg: curBg, bold: curBold, dim: curDim, italic: curItalic, underline: curUnderline, inverse: curInverse };
              cursorX++;
            }
            i++;
            continue;
          }
          if (ch === '\x07') {
            i++;
            continue;
          }

          // 3. Printable characters & Unicode decoding
          var cp = data.codePointAt(i);
          var codeUnits = cp > 0xffff ? 2 : 1;
          var glyph = data.slice(i, i + codeUnits);
          i += codeUnits;

          var w = wcwidth(cp);
          var buf = activeBuffer();
          var curLine = ensureLine(buf, cursorY);

          if (w === 0) {
            if (cursorX > 0 && curLine[cursorX - 1]) {
              curLine[cursorX - 1].ch = (curLine[cursorX - 1].ch || '') + glyph;
            }
            continue;
          }

          if (cursorX + w > cols) {
            if (autoWrap) {
              cursorX = 0;
              var bot = (cursorY <= scrollBottomMargin) ? scrollBottomMargin : rows - 1;
              if (cursorY >= bot) {
                scrollRegionUp();
              } else {
                cursorY++;
              }
              curLine = ensureLine(buf, cursorY);
            } else {
              cursorX = Math.max(0, cols - w);
            }
          }

          curLine[cursorX] = {
            ch: glyph,
            width: w,
            fg: curFg,
            bg: curBg,
            bold: curBold,
            dim: curDim,
            italic: curItalic,
            underline: curUnderline,
            inverse: curInverse
          };
          if (w === 2) {
            curLine[cursorX + 1] = { ch: '', width: 0, isContinuation: true };
            cursorX += 2;
          } else {
            cursorX += 1;
          }
        }

        scheduleRender();
      }

      function sendShell(txt) {
        // Only the visible terminal takes input (never a hidden one).
        if (activeTab !== id && 'sz-' + activeTab !== id) return;
        call('intelligence_shell_write', { id: id, data: txt }).catch(function () {});
      }

      hiddenInput.addEventListener('keydown', function (e) {
        if ((e.metaKey || e.ctrlKey) && (e.key === '0' || e.code === 'Digit0' || e.keyCode === 48)) {
          e.preventDefault();
          e.stopPropagation();
          if (typeof window.toggleWarp === 'function') window.toggleWarp();
          return;
        }
        if (e.metaKey) {
          if (e.key === 'c' || e.key === 'C') {
            if (!String(window.getSelection() || '')) {
              e.preventDefault();
              sendShell('\x03');
            }
            return;
          }
          if (e.key === 'k' || e.key === 'K') {
            e.preventDefault();
            if (!isAlt) {
              scrollback = [];
              for (var r = 0; r < rows; r++) screenGrid[r] = [];
              cursorX = 0; cursorY = 0;
              renderBuffer();
            }
            sendShell('\x0c');
            return;
          }
          if (e.key === 'v' || e.key === 'V') {
            return;
          }
          return;
        }

        if (e.ctrlKey && !e.metaKey && !e.altKey && e.key.length === 1) {
          var code = e.key.toLowerCase().charCodeAt(0);
          if (code >= 97 && code <= 122) {
            e.preventDefault();
            sendShell(String.fromCharCode(code - 96));
            return;
          }
          if (e.key === '@' || e.key === ' ') {
            e.preventDefault();
            sendShell('\x00');
            return;
          }
          if (e.key === '[') {
            e.preventDefault();
            sendShell('\x1b');
            return;
          }
        }

        if (e.altKey && !e.ctrlKey && !e.metaKey && e.key.length === 1) {
          e.preventDefault();
          sendShell('\x1b' + e.key);
          return;
        }

        var nav = {
          'Enter': '\r', 'Backspace': '\x7f', 'Tab': e.shiftKey ? '\x1b[Z' : '\t', 'Escape': '\x1b',
          'ArrowUp': '\x1b[A', 'ArrowDown': '\x1b[B', 'ArrowRight': '\x1b[C', 'ArrowLeft': '\x1b[D',
          'Home': '\x1b[H', 'End': '\x1b[F', 'Delete': '\x1b[3~', 'PageUp': '\x1b[5~', 'PageDown': '\x1b[6~'
        };
        if (nav[e.key]) {
          e.preventDefault();
          sendShell(nav[e.key]);
          return;
        }

        if (e.key.length === 1 && !e.isComposing) {
          e.preventDefault();
          sendShell(e.key);
          return;
        }
      });

      hiddenInput.addEventListener('input', function () {
        if (hiddenInput.value) {
          // Inserted text (IME, dictation, accessibility): a line break is
          // the Enter key, which a terminal sends as CR - LF only inserted a
          // new line in full-screen programs (Claude, Antigravity).
          var v = hiddenInput.value;
          var enter = /\n$/.test(v);
          var body = enter ? v.slice(0, -1) : v;
          if (body.indexOf('\n') >= 0 && bracketedPaste) sendShell('\x1b[200~' + body + '\x1b[201~' + (enter ? '\r' : ''));
          else sendShell(body.replace(/\n/g, '\r') + (enter ? '\r' : ''));
          hiddenInput.value = '';
        }
      });

      hiddenInput.addEventListener('paste', function (e) {
        e.preventDefault();
        var txt = (e.clipboardData || window.clipboardData).getData('text');
        if (txt) {
          if (bracketedPaste) {
            sendShell('\x1b[200~' + txt + '\x1b[201~');
          } else {
            sendShell(txt);
          }
        }
      });

      screenEl.addEventListener('click', function () {
        if (!String(window.getSelection() || '')) {
          hiddenInput.focus();
        }
      });
      screenEl.addEventListener('scroll', function () {
        var maxScroll = screenEl.scrollHeight - screenEl.clientHeight;
        if (maxScroll <= 0) {
          autoFollow = true;
          userScrolledUp = false;
          return;
        }
        var distFromBottom = maxScroll - screenEl.scrollTop;
        if (distFromBottom <= 28) {
          autoFollow = true;
          userScrolledUp = false;
          pendingScrollbackLines = 0;
        } else {
          autoFollow = false;
          userScrolledUp = true;
        }
        if (!isAlt) {
          savedMainScrollTop = screenEl.scrollTop;
        }
      });
      hiddenInput.addEventListener('focus', function () {
        paneEl.classList.remove('blur'); paneEl.classList.add('focus');
      });
      hiddenInput.addEventListener('blur', function () {
        paneEl.classList.remove('focus'); paneEl.classList.add('blur');
      });

      function fit() {
        var size = measureCharSize(screenEl);
        charW = size.w || 7.55;
        charH = size.h || 18;
        var w = screenEl.clientWidth - 24;
        var h = screenEl.clientHeight - 16;
        if (w <= 0 || h <= 0) return;
        var newCols = Math.max(20, Math.floor(w / charW));
        var newRows = Math.max(5, Math.floor(h / charH));
        if (newCols !== cols || newRows !== rows) {
          var oldRows = rows;
          cols = newCols;
          rows = newRows;
          scrollTopMargin = 0;
          scrollBottomMargin = rows - 1;

          if (newRows > oldRows) {
            while (screenGrid.length < newRows) {
              if (!isAlt && scrollback.length > 0) {
                screenGrid.unshift(scrollback.pop());
                cursorY = Math.min(newRows - 1, cursorY + 1);
              } else {
                screenGrid.push([]);
              }
            }
            while (altGrid.length < newRows) altGrid.push([]);
          } else if (newRows < oldRows) {
            while (screenGrid.length > newRows) {
              if (!isAlt) {
                scrollback.push(screenGrid.shift());
                cursorY = Math.max(0, cursorY - 1);
              } else {
                screenGrid.pop();
              }
            }
            while (altGrid.length > newRows) altGrid.pop();
          }
          cursorY = Math.max(0, Math.min(rows - 1, cursorY));
          cursorX = Math.max(0, Math.min(cols - 1, cursorX));

          call('intelligence_shell_resize', { id: id, cols: cols, rows: rows }).catch(function () {});
          scheduleRender();
        }
      }

      var ro = new ResizeObserver(function () {
        if (!paneEl.hidden && screenEl.clientWidth > 0) {
          fit();
        }
      });
      ro.observe(screenEl);

      return {
        id: id,
        title: title,
        cwd: cwd,
        paneEl: paneEl,
        screenEl: screenEl,
        feed: feed,
        fit: fit,
        saveScroll: function () {
          if (!isAlt) savedMainScrollTop = screenEl.scrollTop;
        },
        restoreScroll: function () {
          if (autoFollow) {
            screenEl.scrollTop = screenEl.scrollHeight;
          } else {
            screenEl.scrollTop = savedMainScrollTop;
          }
        },
        focus: function () { setTimeout(function () { hiddenInput.focus(); }, 10); },
        // The size the emulator REALLY has (the PTY must start with it).
        groesse: function () { if (!paneEl.hidden && screenEl.clientWidth > 0) fit(); return { cols: cols, rows: rows }; },
        destroy: function () {
          ro.disconnect();
          paneEl.remove();
        }
      };
    }

    // Handle PTY shell data events from Tauri
    if (window.__TAURI__ && window.__TAURI__.event) {
      window.__TAURI__.event.listen('noki-shell-data', function (e) {
        var p = e && e.payload;
        if (p && p.id) {
          var target = shellTabs.find(function (s) { return s.id === p.id; });
          if (!target && p.id.indexOf('sz-') === 0) { var owner = sitzung(p.id.slice(3)); target = owner && owner.shell; }
          if (target) target.feed(p.data || '');
        }
      });
    }

    // ── log & events ──────────────────────────────────────────────────
    function atBottom() { return screen.scrollHeight - screen.scrollTop - screen.clientHeight < 28; }
    function add(node) {
      var follow = stick;
      log.appendChild(node);
      while (log.childNodes.length > 1500) log.removeChild(log.firstChild);
      if (follow) screen.scrollTop = screen.scrollHeight; else jump.hidden = false;
    }
    screen.addEventListener('scroll', function () { stick = atBottom(); if (stick) jump.hidden = true; });
    jump.onclick = function () { stick = true; screen.scrollTop = screen.scrollHeight; jump.hidden = true; input.focus(); };
    function banner() {
      var b = el('div', 'nc-banner');
      var pre = el('pre', 'nc-ascii-banner', ASCII_WORDMARK);
      var sub = el('div', 'nc-banner-sub', 'Powered by JackOD 9B Coder');
      b.appendChild(pre);
      b.appendChild(sub);
      add(b);
    }
    function daySep(ts) {
      var k = dayKey(ts);
      if (k === lastDay) return;
      lastDay = k;
      add(el('div', 'nc-day', dayLabel(ts)));
    }
    function eventRow(e, old, origin) {
      daySep(e.timestamp);
      return eventRowEl(e, old, origin);
    }
    function eventRowEl(e, old, origin) {
      if (e.kind === 'User') {
        var u = el('div', 'nc-user' + (old ? ' old' : ''));
        u.appendChild(el('span', 'nc-u-caret', '›'));
        u.appendChild(el('span', 'nc-u-text', e.summary));
        var info = el('span', 'nc-time', (origin === 'terminal' ? 'Terminal · ' : '') + clock(e.timestamp));
        u.appendChild(info);
        return u;
      }
      var k = KINDS[e.kind] || ['·', e.kind, 'look'];
      var tone = k[2], glyph = k[0], label = k[1], text = e.summary || '';
      if (e.kind === 'DONE' && (!text || text === 'Done')) {
        text = 'Änderung erfolgreich geprüft.';
      }
      if (e.kind === 'TEST') {
        var m = /^(.*?)\s·\s(PASS|FAIL)$/.exec(text);
        if (m) { text = m[1]; }
        if (e.ok === true) { glyph = '✓'; label = 'Passed'; tone = 'ok'; }
        else if (e.ok === false) { glyph = '✗'; label = 'Failed'; tone = 'err'; }
      }
      text = text.replace(/^\$\s/, '');
      var row = el('div', 'nc-ev' + (old ? ' old' : ''));
      row.dataset.k = tone;
      row.appendChild(el('span', 'nc-g', glyph));
      row.appendChild(el('span', 'nc-l', label));
      var body = el('div', 'nc-b');
      var lines = text.split('\n');
      body.appendChild(el('div', 'nc-b1', lines[0]));
      if (lines.length > 1) body.appendChild(el('div', 'nc-more', lines.slice(1).join('\n')));
      (e.affected_files || []).forEach(function (f) {
        var fm = /^(.*)\s(\+\d+)\s(−\d+)$/.exec(f), line = el('div', 'nc-file');
        if (fm) { line.appendChild(el('span', null, fm[1])); line.appendChild(el('span', 'nc-add', fm[2])); line.appendChild(el('span', 'nc-del', fm[3])); }
        else line.textContent = f;
        body.appendChild(line);
      });
      row.appendChild(body);
      row.appendChild(el('span', 'nc-time', clock(e.timestamp)));
      return row;
    }
    function note(text, level) { add(el('div', 'nc-note' + (level ? ' ' + level : ''), text)); }
    function infoBlock(lines) { add(el('pre', 'nc-info', (lines || []).join('\n'))); }
    function clearView(silent) {
      log.textContent = ''; lastDay = '';
      if (!silent) note('Ansicht geleert – Verlauf bleibt gespeichert.');
    }

    // ── confirmation (host prompts and local wipe) ─────────────────────
    function showConfirm(opt) {
      pending = opt;
      q('.nc-c-head').textContent = opt.head;
      q('.nc-c-lead').textContent = opt.lead;
      var ul = box.querySelector('ul'); ul.textContent = '';
      opt.items.forEach(function (i) { ul.appendChild(el('li', null, i)); });
      q('.nc-c-q').textContent = opt.question;
      box.querySelector('[data-c="yes"]').textContent = opt.yes;
      box.dataset.k = opt.danger ? 'danger' : '';
      box.hidden = false; box.focus();
      if (stick) screen.scrollTop = screen.scrollHeight;
    }
    function answer(yes) {
      if (!pending) return;
      var p = pending; pending = null; box.hidden = true;
      p.done(yes);
      input.focus();
    }
    box.addEventListener('click', function (e) { var b = e.target.closest('[data-c]'); if (b) answer(b.dataset.c === 'yes'); });
    box.addEventListener('keydown', function (e) {
      if (e.key === 'y' || e.key === 'Y' || e.key === 'j') { e.preventDefault(); answer(true); }
      else if (e.key === 'n' || e.key === 'N' || e.key === 'Escape' || e.key === 'Enter') { e.preventDefault(); answer(false); }
    });
    function askWipe() {
      showConfirm({ head: 'Verlauf löschen', lead: 'Dauerhaft entfernen:', items: ['Gespeicherter Code-Verlauf dieses Projekts'], question: 'Code-Verlauf wirklich löschen? [y/N]', yes: 'Löschen', danger: true,
        done: function (yes) { if (yes) send({ t: 'clear_history' }); else note('Code-Verlauf bleibt erhalten.'); } });
    }

    // ── CODE SPACE · Projekt ───────────────────────────────────────────
    var proj = null, projTimer = 0, projSection = q('.nc-proj');
    function home(p) { return String(p || '').replace(/^\/Users\/[^/]+/, '~'); }
    // Local time, e.g. "01.10.2026 · 14:32" (seconds or ms).
    function datumZeit(t) {
      var d = new Date(t > 1e12 ? t : t * 1000), z = function (n) { return (n < 10 ? '0' : '') + n; };
      return z(d.getDate()) + '.' + z(d.getMonth() + 1) + '.' + d.getFullYear() + ' · ' + z(d.getHours()) + ':' + z(d.getMinutes());
    }
    function laneText(m) {
      if (!m || !m.display_name) return '';
      var lane = { LOCAL: 'Local', FREE_CLOUD: 'Free Cloud', PAID_CLOUD: 'Paid Cloud', DIREKT: 'direkter Coding-Lauf' }[m.execution_lane] || '';
      var name = String(m.display_name).replace(/\s+Free(\s+\(Mistral\))?/i, '').replace(/^qwen3\.5:9b$/i, 'Qwen 3.5 9B');
      return name + (lane ? ' · ' + lane : '');
    }
    // Header while Code Space shows a project: what really works on it.
    function kopfSetzen() {
      if (headerEl && activeTab === NOKI_TERM && (!projSection || projSection.hidden) && !term.hidden) {
        var nz = sitzung(NOKI_TERM);
        var hb = headerEl.querySelector('.ni-brand-title'); if (hb) hb.textContent = 'Noki Terminal';
        var hm = headerEl.querySelector('.ni-model-indicator');
        if (hm) hm.textContent = nz && nz.modell ? laneText(nz.modell) : '';
        var ht = headerEl.querySelector('.ni-runtime-text');
        if (ht) ht.textContent = nz && nz.projekt ? nz.projekt.name : 'Noki Code';
        return;
      }
      var aktivT = shellTabs.find(function (x) { return x.id === activeTab; });
      if (headerEl && aktivT && (!projSection || projSection.hidden) && !term.hidden) {
        var nc = nokiCode[activeTab];
        var b = headerEl.querySelector('.ni-brand-title'); if (b) b.textContent = aktivT.title;
        var m = headerEl.querySelector('.ni-model-indicator');
        if (m) m.textContent = nc && nc.modell ? laneText(nc.modell) : '';
        var tx = headerEl.querySelector('.ni-runtime-text');
        if (tx) tx.textContent = nc ? 'Noki Code' + (nc.pfad ? ' · ' + home(nc.pfad).split('/').pop() : '') : 'Terminal';
        return;
      }
      if (!headerEl || !projSection || projSection.hidden) return;
      var brand = headerEl.querySelector('.ni-brand-title');
      if (brand) brand.textContent = 'Code Space';
      var mi = headerEl.querySelector('.ni-model-indicator');
      if (mi) mi.textContent = proj && proj.modell ? laneText(proj.modell) : '';
      var txt = headerEl.querySelector('.ni-runtime-text');
      if (txt) txt.textContent = proj ? (proj.laeuft ? 'Noki arbeitet · ' : '') + (proj.name || '') : 'Code-Projekte';
    }
    function projZeigen() {
      over.hidden = true; term.hidden = true; projSection.hidden = false; menu.hidden = true;
      q('.nc-p-list').hidden = true; q('.nc-p-main').hidden = false;
      projRender();
    }
    function projRender() {
      if (!proj) return;
      q('.nc-p-name').textContent = proj.name || '';
      var st = proj.laeuft ? ['', 'Noki arbeitet', 'busy'] : proj.status === 'fertig' ? ['✓', 'Fertig', 'ok']
        : proj.status === 'blockiert' ? ['×', 'Blockiert', 'err'] : proj.status === 'mit Fehlern' ? ['!', 'Mit Fehlern', 'warn'] : ['○', 'Projekt', 'idle'];
      var stEl = q('.nc-p-state');
      var sek = proj.laeuft && proj.start ? ' · ' + Math.round((Date.now() - proj.start) / 1000) + ' s' : (proj.sekunden ? ' · ' + proj.sekunden + ' s' : '');
      stEl.textContent = st[0] + ' ' + st[1] + sek; stEl.dataset.k = st[2];
      q('.nc-p-model').textContent = proj.modell ? 'CODE · ' + laneText(proj.modell) : (proj.laeuft ? 'CODE · Modell wird gewählt …' : '');
      q('.nc-p-path').textContent = home(proj.pfad);
      q('.nc-p-task').textContent = proj.aufgabe ? 'Aufgabe: ' + proj.aufgabe : '';
      var ol = q('.nc-p-steps'); ol.textContent = '';
      (proj.schritte || []).forEach(function (x) {
        var li = el('li', 'nc-p-step'); li.dataset.k = x.ok ? 'ok' : 'err';
        li.appendChild(el('span', 'nc-g', x.ok ? '✓' : '×'));
        var b = el('span', 'nc-p-sl', x.label);
        li.appendChild(b);
        if (!x.ok && x.detail) li.appendChild(el('div', 'nc-p-sd', x.detail));
        else if (/Vorschau/.test(x.label) && x.detail) li.appendChild(el('div', 'nc-p-sd', x.detail));
        ol.appendChild(li);
      });
      if (proj.laeuft) {
        var cur = el('li', 'nc-p-step'); cur.dataset.k = 'busy';
        cur.appendChild(el('span', 'nc-p-sl', proj.modell ? laneText(proj.modell) + ' schreibt den nächsten Schritt …' : 'Plant den ersten Schritt …'));
        ol.appendChild(cur);
      }
      var ul = q('.nc-p-files'); ul.textContent = '';
      (proj.dateien || []).forEach(function (d) {
        var li = el('li', 'nc-p-file', d); li.dataset.datei = d; ul.appendChild(li);
      });
      if (!(proj.dateien || []).length) ul.appendChild(el('li', 'nc-p-none', proj.laeuft ? 'Noch keine Datei geschrieben.' : '—'));
      var res = q('.nc-p-result'); res.textContent = proj.laeuft ? '' : (proj.ergebnis || '');
      q('.nc-p-rh').hidden = proj.laeuft || !proj.ergebnis;
      q('[data-act="p-vorschau"]').disabled = !proj.vorschau;
      q('.nc-p-folge textarea').disabled = !!proj.laeuft;
      kopfSetzen();
      clearTimeout(projTimer);
      if (proj.laeuft && !projSection.hidden) projTimer = setTimeout(projRender, 1000);
    }
    function ausMeta(m) {
      var laeufe = m.laeufe || [], letzter = laeufe[laeufe.length - 1] || {};
      return { pfad: m.pfad, name: m.name, dateien: m.dateien || [], vorschau: !!m.vorschau, laeuft: !!m.laeuft,
        status: m.projekt_status || m.status || letzter.status || '', aufgabe: letzter.aufgabe || '', schritte: letzter.schritte || [],
        ergebnis: letzter.ergebnis || '', modell: letzter.modell || m.modell || null, sekunden: letzter.sekunden || 0,
        start: letzter.start ? letzter.start * 1000 : 0 };
    }
    function projLaden(pfad) {
      projFehler('');
      return call('code_projekt_info', { pfad: pfad || null }).then(function (m) {
        if (!(proj && proj.laeuft && proj.pfad === m.pfad)) proj = ausMeta(m);  // live state wins
        try { projZeigen(); } catch (err) { log('render ' + err); }
      }, function (e) {
        log('info ' + e);
        projListe(); listeHinweis(String(e));
      });
    }
    function log(t) { call('noki_log', { msg: '[CODE-SPACE] ' + t }).catch(function () {}); }
    function listeHinweis(t) { var h = q('.nc-p-list-h'); h.textContent = 'Code-Projekte · ~/Documents/Noki/Code' + (t ? ' — ' + t : ''); }
    // Open a project in Code Space: the terminal that owns it, else a new
    // terminal bound to it (never re-points a busy or different session).
    function oeffneProjekt(pfad) {
      if (projSection) projSection.hidden = true;
      return sLaden().then(function () {
        if (!pfad) { showTerm(); switchTab('noki'); return; }
        var sz = sitzungen.find(function (x) { return x.pfad === pfad; });
        if (sz) { showTerm(); switchTab(sz.id); return; }
        return sNeu(pfad);
      }).catch(function (e) { showTerm(); note(String(e), 'err'); });
    }
    function projFehler(t) { var e = q('.nc-p-err'); e.textContent = t || ''; }
    function projListe(markiert) {
      over.hidden = true; term.hidden = true; projSection.hidden = false;
      q('.nc-p-main').hidden = true; q('.nc-p-list').hidden = false;
      q('.nc-p-name').textContent = ''; var stEl = q('.nc-p-state'); stEl.textContent = ''; listeHinweis('');
      proj = proj && proj.laeuft ? proj : null; kopfSetzen();
      var ul = q('.nc-p-items'); ul.textContent = '';
      call('code_projekte').then(function (liste) {
        if (!liste || !liste.length) { ul.appendChild(el('li', 'nc-p-none', 'Noch keine Projekte. Beschreibe im Chat, was Noki bauen soll.')); return; }
        liste.forEach(function (m) {
          // Compact row (the approved look): name, mode, status · model,
          // actions, ×. Everything else lives in the detail on click.
          var li = el('li', 'nc-p-item' + (markiert && m.pfad === markiert ? ' on' : '')); li.dataset.pfad = m.pfad;
          var l = (m.laeufe || []), last = l[l.length - 1] || {}, first = l[0] || {};
          var zeile = el('div', 'nc-p-zeile');
          // Name first (primary), one quiet meta line below - no chips.
          var txt = el('div', 'nc-p-txt');
          var name = el('span', 'nc-p-in', m.name); name.title = 'Details anzeigen'; name.dataset.pact = 'p-det';
          txt.appendChild(name);
          zeile.appendChild(txt);
          var modus = m.modus || last.modus; // the project's mode, not one run's
          // Project state (whole project) - not the outcome of the last run.
          var st = [m.projekt_status || m.status || last.status || '–'];
          if (modus) st.unshift(modusText(modus));
          // Model of the last run that really changed files (a blocked run that
          // touched nothing does not define who built the project).
          var wirksam = l.slice().reverse().find(function (r) { return (r.schritte || []).some(function (x) { return x.art === 'datei' && x.ok; }); }) || last;
          if (wirksam.modell) st.push(String(wirksam.modell.display_name || '').replace(/\s+Free(\s+\(Mistral\))?/i, ''));
          if (m.erstellt) st.push(datumZeit(m.erstellt));
          var is = el('span', 'nc-p-is', st.join(' · ')); is.title = st.join(' · ');
          txt.appendChild(is);
          var akt = el('div', 'nc-p-akt');
          [['p-code', 'Zum Projekt'], ['p-vor', 'Vorschau öffnen'], ['p-fin', 'Im Finder zeigen']].forEach(function (y) {
            var bt = el('button', 'nc-btn', y[1]); bt.type = 'button'; bt.dataset.pact = y[0];
            if (y[0] === 'p-vor' && !m.vorschau) bt.disabled = true;
            akt.appendChild(bt);
          });
          zeile.appendChild(akt);
          var x = el('button', 'nc-p-x'); x.type = 'button'; x.dataset.pact = 'p-del'; x.title = 'Projekt löschen (Papierkorb)'; x.setAttribute('aria-label', 'Projekt ' + m.name + ' löschen');
          zeile.appendChild(x);
          li.appendChild(zeile);
          // Detail (hidden until asked): prompt, files, runs, model history.
          var det = el('div', 'nc-p-detail'); det.hidden = true;
          if (first.aufgabe) det.appendChild(el('div', 'nc-p-prompt', String(first.aufgabe).trim().slice(0, 600)));
          var modelle = [];
          l.forEach(function (r) {
            var wer = r.extern ? (r.modell && r.modell.display_name || r.akteur || 'Extern') + ' · direkt' : (r.modell ? laneText(r.modell) : 'Noki Code');
            var t = wer + ' · ' + (r.status === 'läuft' ? 'unterbrochen' : r.status || '–');
            if (modelle[modelle.length - 1] !== t) modelle.push(t);
          });
          if (m.erstellt) det.appendChild(el('div', 'nc-p-info', 'Erstellt ' + datumZeit(m.erstellt) + (m.aktualisiert ? ' · Zuletzt geändert ' + datumZeit(m.aktualisiert) : '')));
          var info = [(m.dateien || []).length + ' Dateien', l.length + (l.length === 1 ? ' Lauf' : ' Läufe')];
          if (last.sekunden) info.push('letzter Lauf ' + last.sekunden + ' s');
          det.appendChild(el('div', 'nc-p-info', info.join(' · ')));
          if (modelle.length) det.appendChild(el('div', 'nc-p-info', 'Verlauf: ' + modelle.join(' → ')));
          if ((m.dateien || []).length) det.appendChild(el('div', 'nc-p-info', (m.dateien || []).join(' · ')));
          li.appendChild(det);
          ul.appendChild(li);
          if (markiert && m.pfad === markiert) setTimeout(function () { li.scrollIntoView({ block: 'nearest' }); }, 30);
        });
      }).catch(function (e) { ul.appendChild(el('li', 'nc-p-none', String(e))); });
    }
    projSection.addEventListener('click', function (e) {
      if (e.target.closest('.nc-nav [data-style]')) { projSection.hidden = true; clearTimeout(projTimer); showTerm(); return; }
      var pb = e.target.closest('[data-pact]');
      if (pb) {
        var item = pb.closest('.nc-p-item'), pf = item.dataset.pfad;
        if (pb.dataset.pact === 'p-det') { var dt = item.querySelector('.nc-p-detail'); if (dt) { dt.hidden = !dt.hidden; item.classList.toggle('offen', !dt.hidden); } return; }
        if (pb.dataset.pact === 'p-del') {
          // Inline confirmation (no browser dialog): exact project, Trash.
          if (item.querySelector('.nc-p-confirm')) return;
          var c = el('div', 'nc-p-confirm');
          c.appendChild(el('span', 'nc-p-confirm-t', 'Projekt wirklich löschen? Es wird in den Papierkorb verschoben.'));
          var ja = el('button', 'nc-btn nc-danger-btn', 'Löschen'); ja.type = 'button'; ja.dataset.pact = 'p-del-ja';
          var nein = el('button', 'nc-btn', 'Abbrechen'); nein.type = 'button'; nein.dataset.pact = 'p-del-nein';
          c.appendChild(ja); c.appendChild(nein); item.appendChild(c);
          return;
        }
        if (pb.dataset.pact === 'p-del-nein') { var cc = item.querySelector('.nc-p-confirm'); if (cc) cc.remove(); return; }
        if (pb.dataset.pact === 'p-del-ja') {
          call('code_projekt_loeschen', { pfad: pf }).then(function () {
            item.remove(); listeHinweis('Projekt in den Papierkorb verschoben.');
            var nz = sitzung(NOKI_TERM); if (nz && nz.pfad === pf) { nz.pfad = null; nz.projekt = null; sVerlauf(nz); sKontext(nz); }
          }).catch(function (err) { listeHinweis(String(err)); });
          return;
        }
        if (pb.dataset.pact === 'p-code') {
          // Open the project in the Noki Terminal: its full history; the
          // next task there continues THIS project.
          var nzB = sitzung(NOKI_TERM);
          if (nzB && nzB.laeuft) { listeHinweis('Das Noki Terminal arbeitet gerade an einer Aufgabe – „Zum Projekt“ ist danach möglich.'); return; }
          call('code_sitzung_projekt', { id: NOKI_TERM, pfad: pf }).then(function (d) {
            var nz = sitzung(NOKI_TERM);
            if (nz) {
              nz.pfad = d.pfad; nz.projekt = d.projekt; nz.modus = d.modus || nz.modus; nz.letztesModell = '';
              // The context line shows THIS project's state, not the last run elsewhere.
              var lz = d.projekt && d.projekt.laeufe ? d.projekt.laeufe[d.projekt.laeufe.length - 1] : null;
              nz.status = (lz && lz.status) || ''; nz.modell = (lz && lz.modell) || null;
              sVerlauf(nz); sKontext(nz);
            }
            projSection.hidden = true; term.hidden = false; over.hidden = true; switchTab(NOKI_TERM);
          }).catch(function (err) { listeHinweis(String(err)); });
          return;
        }
        if (pb.dataset.pact === 'p-term') {
          // A NORMAL terminal in the project folder; `noki code` there
          // continues this project - the user starts it.
          projSection.hidden = true; term.hidden = false; over.hidden = true;
          spawnShellTab(null, pf);
        } else {
          call(pb.dataset.pact === 'p-vor' ? 'code_vorschau_oeffnen' : 'code_projekt_zeigen', { pfad: pf })
            .catch(function (err) { listeHinweis(String(err)); });
        }
        return;
      }
      var f = e.target.closest('.nc-p-file');
      if (f && proj) {
        var pre = q('.nc-p-code');
        call('code_projekt_dateiinhalt', { pfad: proj.pfad, datei: f.dataset.datei }).then(function (t) {
          pre.textContent = t; pre.hidden = false; pre.scrollTop = 0;
          projSection.querySelectorAll('.nc-p-file').forEach(function (x) { x.classList.toggle('on', x === f); });
        }).catch(function (err) { projFehler(String(err)); });
        return;
      }
      var b = e.target.closest('[data-act]'); if (!b) return;
      var act = b.dataset.act;
      if (act === 'p-liste') projListe();
      else if (act === 'p-terminal') { projSection.hidden = true; clearTimeout(projTimer); showTerm(); }
      else if (act === 'p-vorschau' && proj) call('code_vorschau_oeffnen', { pfad: proj.pfad }).catch(function (err) { projFehler(String(err)); });
      else if (act === 'p-finder' && proj) call('code_projekt_zeigen', { pfad: proj.pfad }).catch(function (err) { projFehler(String(err)); });
    });
    var folge = q('.nc-p-folge textarea');
    folge.addEventListener('keydown', function (e) {
      if (e.key !== 'Enter' || e.shiftKey || e.isComposing) return;
      e.preventDefault();
      var t = folge.value.trim(); if (!t || !proj || proj.laeuft) return;
      folge.value = '';
      if (opts.folgeauftrag) opts.folgeauftrag(t, proj.pfad);
    });
    if (window.__TAURI__ && window.__TAURI__.event) {
      var ev = window.__TAURI__.event;
      ev.listen('intelligence-code-start', function (e) {
        var p = (e && e.payload) || {};
        log('start ' + (p.projekt || ''));
        var alt = proj && proj.pfad === p.pfad ? proj : null;
        proj = { pfad: p.pfad, name: p.projekt, aufgabe: p.aufgabe || '', laeuft: true, start: Date.now(), modell: null,
          schritte: [{ label: p.neu ? 'Projekt erstellt · ' + home(p.pfad) : 'Projekt geöffnet · bestehende Dateien werden geändert', ok: true }],
          dateien: alt ? alt.dateien : [], vorschau: alt ? alt.vorschau : false, status: '' };
        if (!projSection.hidden) projRender();
      });
      ev.listen('intelligence-code', function (e) {
        var p = (e && e.payload) || {};
        if (!proj || !proj.laeuft) return;
        proj.schritte.push({ label: String(p.label || ''), ok: !!p.ok, detail: String(p.detail || '') });
        if (Array.isArray(p.dateien)) proj.dateien = p.dateien;
        if (/Vorschau/.test(p.label || '') && p.ok) proj.vorschau = true;
        if (!projSection.hidden) projRender();
      });
      ev.listen('intelligence-model', function (e) {
        var p = e && e.payload;
        if (!p || !p.display_name || !proj || !proj.laeuft) return;
        proj.modell = p;
        if (!projSection.hidden) projRender();
      });
      ev.listen('intelligence-code-ende', function (e) {
        var m = (e && e.payload) || {};
        if (!proj || proj.pfad !== m.pfad) return;
        var neu = ausMeta(m); neu.laeuft = false;
        proj = neu;
        if (!projSection.hidden) projRender();
      });
    }

    // ── CODE SPACE · Terminal-Sitzungen ───────────────────────────────
    // "Noki Terminal", "Terminal 1", … : each is a real coding session with
    // its own project, mode, model and output. Same terminal component
    // (nc-screen / nc-log / nc-ev / nc-prompt) as the Noki Terminal - one
    // look, one CSS. Events arrive tagged with the session id: no mixing.
    var sitzungen = [], sessionsView = q('.nc-sessions-view'), bench = q('.nc-bench');
    function modusText(m) { return m === 'creative' ? 'Kreativ' : 'Funktional'; }
    function sitzung(id) { return (sitzungen || []).find(function (x) { return x.id === id; }); }
    function sPane(sz) {
      var pane = el('div', 'nc-agent-view nc-sitzung'); pane.hidden = true; pane.dataset.sitzung = sz.id;
      pane.innerHTML =
        // Project line only when the terminal has a project (no title/mode
        // labels: the tab names the terminal, the mode buttons the mode).
        '<div class="nc-kontext" hidden>' +
          '<span class="nc-k-projekt"></span><button type="button" class="nc-k-loesen" title="Projekt lösen – nichts wird gelöscht">lösen</button><span class="nc-k-modell"></span><span class="nc-k-status"></span>' +
        '</div>' +
        '<div class="nc-noki-view">' +
          '<div class="nc-screen"><div class="nc-log" role="log" aria-live="polite"></div></div>' +
          '<button type="button" class="nc-jump" hidden>↓ Neue Ereignisse</button>' +
          '<div class="nc-live" hidden><span class="nc-ring" aria-hidden="true"></span><div class="nc-live-l"><span class="nc-live-m"></span><span class="nc-live-p"></span></div><span class="nc-live-k">⌃C abbrechen</span></div>' +
          '<form class="nc-prompt"><span class="nc-caret" aria-hidden="true">›</span>' +
            '<textarea rows="1" spellcheck="false" autocomplete="off"></textarea>' +
            '<button type="submit" class="nc-btn nc-send">Senden</button></form>' +
          '<div class="nc-foot"><span>↵ senden</span><span>⇧↵ neue Zeile</span><span>↑↓ Verlauf</span><span>⌃C abbrechen</span><span>⌘K leeren</span></div>' +
        '</div>' +
        // The terminal's own real shell (PTY): claude, antigravity, git, …
        '<div class="nc-tab-shell" hidden>' +
          '<div class="nc-ts-bar"><button type="button" class="nc-btn" data-view="noki">Noki</button><span class="nc-ts-t"></span></div>' +
          '<div class="nc-ts-slot"></div>' +
        '</div>';
      var t = pane.querySelector('textarea');
      t.setAttribute('aria-label', 'Coding-Aufgabe an ' + sz.titel);
      t.placeholder = 'Coding-Aufgabe an ' + sz.titel + ' …';
      pane.querySelector('.nc-send').setAttribute('aria-label', 'Senden an ' + sz.titel);
      pane.querySelector('.nc-ts-t').textContent = 'zsh · ' + sz.titel;
      sz.verlauf = []; sz.histPos = -1; sz.ansicht = 'noki';
      pane.addEventListener('click', function (e) {
        var v = e.target.closest('[data-view]'); if (!v) return;
        sAnsicht(sz, v.dataset.view);
      });
      sz.pane = pane; sz.log = pane.querySelector('.nc-log'); sz.screen = pane.querySelector('.nc-screen');
      pane.querySelector('.nc-k-loesen').addEventListener('click', function () { sLoesen(sz); });
      sz.input = t; sz.liveEl = pane.querySelector('.nc-live'); sz.jump = pane.querySelector('.nc-jump'); sz.stick = true;
      sz.screen.addEventListener('scroll', function () { sz.stick = sz.screen.scrollHeight - sz.screen.scrollTop - sz.screen.clientHeight < 28; if (sz.stick) sz.jump.hidden = true; });
      sz.jump.onclick = function () { sz.stick = true; sz.screen.scrollTop = sz.screen.scrollHeight; sz.jump.hidden = true; };
      pane.querySelector('.nc-prompt').addEventListener('submit', function (e) { e.preventDefault(); sSenden(sz); });
      t.addEventListener('keydown', function (e) {
        if (e.isComposing) return;
        if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sSenden(sz); return; }
        if (e.ctrlKey && (e.key === 'c' || e.key === 'C') && sz.laeuft) {
          e.preventDefault(); call('code_sitzung_abbrechen', { id: sz.id }); sNote(sz, '⌃C Abbruch angefordert', 'warn');
          return;
        }
        if (e.metaKey && (e.key === 'k' || e.key === 'K')) { e.preventDefault(); sz.log.textContent = ''; sBanner(sz); return; }
        var ersteZeile = t.value.lastIndexOf('\n', t.selectionStart - 1) < 0;
        if (e.key === 'ArrowUp' && ersteZeile && sz.verlauf.length) {
          e.preventDefault();
          if (sz.histPos < 0) sz.histPos = sz.verlauf.length;
          sz.histPos = Math.max(0, sz.histPos - 1); t.value = sz.verlauf[sz.histPos];
        } else if (e.key === 'ArrowDown' && sz.histPos >= 0) {
          e.preventDefault();
          sz.histPos++;
          if (sz.histPos >= sz.verlauf.length) { sz.histPos = -1; t.value = ''; } else t.value = sz.verlauf[sz.histPos];
        }
      });
      t.addEventListener('input', function () { t.style.height = 'auto'; t.style.height = Math.min(t.scrollHeight, 160) + 'px'; });
      pane.addEventListener('click', function (e) {
        var b = e.target.closest('[data-sact]'); if (!b || !sz.pfad) return;
        var cmd = b.dataset.sact === 'vorschau' ? 'code_vorschau_oeffnen' : 'code_projekt_zeigen';
        call(cmd, { pfad: sz.pfad }).catch(function (err) { sNote(sz, String(err), 'err'); });
      });
      sessionsView.appendChild(pane);
      return pane;
    }
    function sAdd(sz, node) {
      sz.log.appendChild(node);
      while (sz.log.childNodes.length > 1500) sz.log.removeChild(sz.log.firstChild);
      if (sz.stick) sz.screen.scrollTop = sz.screen.scrollHeight; else sz.jump.hidden = false;
    }
    function sNote(sz, text, level) { sAdd(sz, el('div', 'nc-note' + (level ? ' ' + level : ''), text)); }
    function sEv(sz, kind, text, extra) {
      var e = { kind: kind, summary: text, timestamp: (extra && extra.ts) || Date.now() / 1000, ok: extra ? extra.ok : undefined, affected_files: (extra && extra.files) || [] };
      sAdd(sz, eventRowEl(e, extra && extra.old));
    }
    function sUser(sz, text, ts, old) { sAdd(sz, eventRowEl({ kind: 'User', summary: text, timestamp: ts || Date.now() / 1000 }, old)); }
    function schrittArt(label, ok) {
      if (/Vorschau/.test(label)) return 'TEST';
      if (!ok) return 'ERROR';
      if (/^Schreibe/.test(label)) return 'PATCH';
      if (/^(Lese|Analysiere|Liste|Durchsuche)/.test(label)) return 'INSPECT';
      if (/^Anforderungen/.test(label)) return 'PLAN';
      return 'VERIFY';
    }
    // A real diff block (computed from the files on disk): first lines
    // visible, the rest on demand - no full-file dumps in the stream.
    function diffBlock(diff, zu) {
      var zeilen = String(diff || '').split('\n').filter(function (l) { return l.length; });
      var box = el('div', 'nc-diff');
      if (zu) {
        // History: collapsed until asked for (no thousands of nodes).
        var auf = el('button', 'nc-d-mehr', 'Diff anzeigen (' + zeilen.length + ' Zeilen)'); auf.type = 'button';
        auf.onclick = function () { var neu = diffBlock(diff, false); box.replaceWith(neu); };
        box.appendChild(auf);
        return box;
      }
      function zeigen(n) {
        box.textContent = '';
        zeilen.slice(0, n).forEach(function (l) {
          var c = l.charAt(0), cls = c === '+' ? 'add' : c === '-' ? 'del' : c === '@' ? 'hunk' : 'ctx';
          var z = el('div', 'nc-d nc-d-' + cls, l); box.appendChild(z);
        });
        if (zeilen.length > n) {
          var mehr = el('button', 'nc-d-mehr', 'Diff vollständig anzeigen (' + zeilen.length + ' Zeilen)'); mehr.type = 'button';
          mehr.onclick = function () { zeigen(zeilen.length); };
          box.appendChild(mehr);
        }
      }
      zeigen(40);
      return box;
    }
    // Requirement status in words (colour stays): no symbol needed to read it.
    function auditText(k, t) { return (k === 'ok' ? 'erfüllt   ' : k === 'warn' ? 'teilweise ' : 'offen     ') + t; }
    function saetze(t) { return String(t || '').replace(/\. /g, '.\n').replace(/ · /g, '\n'); }
    function sSchritt(sz, x, old, ts) {
      var label = String(x.label || ''), detail = String(x.detail || ''), art = x.art || '';
      if (art === 'modell') { sModell(sz, x.modell || { display_name: label, execution_lane: detail }, old, ts); return; }
      if (art === 'ausgabe') { sEv(sz, 'OUTPUT', label + (detail ? '\n' + detail : ''), { old: old, ts: ts }); return; }
      if (art === 'notiz') { sEv(sz, 'THINK', detail, { old: old, ts: ts }); return; }
      if (art === 'datei' && x.ok) {
        sEv(sz, 'FILE', label + '   +' + (x.plus || 0) + ' −' + (x.minus || 0), { old: old, ts: ts });
        if (x.diff) sAdd(sz, diffBlock(x.diff, !!old));
        return;
      }
      if (art === 'audit') {
        var zeile = eventRowEl({ kind: 'AUDIT', summary: label, timestamp: ts || Date.now() / 1000 }, old);
        var liste = el('div', 'nc-audit');
        detail.split('\n').forEach(function (l) {
          var k = l.charAt(0) === '✓' ? 'ok' : l.charAt(0) === '△' ? 'warn' : 'err';
          var z = el('div', 'nc-a nc-a-' + k, auditText(k, l.replace(/^[✓△✕]\s*/, ''))); liste.appendChild(z);
        });
        zeile.querySelector('.nc-b').appendChild(liste);
        sAdd(sz, zeile);
        return;
      }
      if (art === 'test' || /Vorschau/.test(label)) {
        sEv(sz, 'TEST', label + '\n' + saetze(detail), { ok: !!x.ok, old: old, ts: ts });
        return;
      }
      if (!x.ok) { sEv(sz, 'ERROR', label + (detail ? '\n' + detail : ''), { old: old, ts: ts }); return; }
      if (/^(Lese|Analysiere|Liste|Durchsuche)/.test(label)) { sEv(sz, 'INSPECT', label, { old: old, ts: ts }); return; }
      sEv(sz, 'VERIFY', label, { old: old, ts: ts });
    }
    function sModell(sz, m, old, ts) {
      if (!m || !m.display_name) return;
      var t = laneText(m);
      if (sz.letztesModell === t) return;
      sz.letztesModell = t;
      sEv(sz, 'MODEL', t, { old: old, ts: ts });
    }
    function sErgebnis(sz, status, sek, text, dateien, old, ts, info) {
      info = info || {};
      var st = status === 'fertig' ? 'Fertig' : status === 'abgeschlossen' ? 'Abgeschlossen' : status === 'abgebrochen' ? 'Abgebrochen' : status === 'blockiert' ? 'Blockiert' : status === 'mit Fehlern' ? 'Mit Fehlern' : status === 'offene Punkte' ? 'Läuft · offene Punkte' : status === 'unterbrochen' ? 'Unterbrochen' : (status || 'Beendet');
      // A blocked run states its reason (model text) - nothing else from it.
      if (status === 'blockiert' && text) info.pruefung = info.pruefung || String(text).replace(/\*\*/g, '');
      sEv(sz, status === 'blockiert' ? 'ERROR' : 'DONE', 'Ergebnis · ' + st + (sek ? ' · ' + sek + ' s' : '') +
        (info.schreibschritte != null ? ' · ' + info.schreibschritte + ' Dateischritte · ' + (info.pruefrunden || 0) + ' Prüfrunden' : ''), { old: old, ts: ts });
      var box = el('div', 'nc-ergebnis' + (old ? ' old' : ''));
      if (status === 'blockiert' && info.pruefung) {
        box.appendChild(el('div', 'nc-e-h nc-e-err', 'Blockiert'));
        box.appendChild(el('div', 'nc-e-t', info.pruefung));
      } else if (info.pruefung) {
        var ok = !/FEHLER/.test(info.pruefung);
        box.appendChild(el('div', 'nc-e-h' + (ok ? ' nc-e-ok' : ' nc-e-err'), 'Letzte Vorschau-Prüfung · ' + (ok ? 'bestanden' : 'Fehler')));
        box.appendChild(el('div', 'nc-e-t', saetze(info.pruefung)));
      }
      if (info.audit && info.audit.length) {
        box.appendChild(el('div', 'nc-e-h', 'Anforderungen'));
        var al = el('div', 'nc-audit');
        info.audit.forEach(function (a) {
          var k = a.status === 'PASS' ? 'ok' : a.status === 'PARTIAL' ? 'warn' : 'err';
          al.appendChild(el('div', 'nc-a nc-a-' + k, auditText(k, a.anforderung + (a.befund ? ' – ' + a.befund : ''))));
        });
        box.appendChild(al);
      }
      if (info.stats && info.stats.length) {
        box.appendChild(el('div', 'nc-e-h', 'Dateien'));
        info.stats.forEach(function (f) {
          var z = el('div', 'nc-e-f');
          z.appendChild(el('span', 'nc-e-n', f.datei));
          z.appendChild(el('span', 'nc-add', '+' + f.plus));
          if (!f.neu) z.appendChild(el('span', 'nc-del', '−' + f.minus)); else z.appendChild(el('span', 'nc-e-neu', 'neu'));
          box.appendChild(z);
        });
      }
      if (info.modell && info.modell.display_name) box.appendChild(el('div', 'nc-e-m', 'Modell · ' + laneText(info.modell)));
      // Project actions live under "Projekte" - the stream only says where.
      if (sz.projekt && sz.projekt.name) box.appendChild(el('div', 'nc-e-t', 'Projekt „' + sz.projekt.name + '“ unter Projekte gespeichert.'));
      sAdd(sz, box);
    }
    // Live line: which model works right now and on what. Driven only by
    // real run events (start/modell/denkt/schritt/phase/ende) - the sheen
    // runs only while a run is active, never from a timer.
    function modellKurz(m) {
      return m && m.display_name ? String(m.display_name).replace(/\s+Free(\s+\(Mistral\))?/i, '').replace(/\s+\((Mistral|Groq|Cloudflare)\)$/i, '') : '';
    }
    function liveSetzen(sz) {
      var l = sz.liveEl; if (!l) return;
      var m = l.querySelector('.nc-live-m'), ph = l.querySelector('.nc-live-p'), k = l.querySelector('.nc-live-k');
      var name = modellKurz(sz.modell);
      if (sz.laeuft) {
        // Ring = it runs, model = who, phase line = what (no "arbeitet").
        m.textContent = sz.modell && sz.modell.execution_lane ? laneText(sz.modell) : (name || 'Modell wird gewählt');
        ph.textContent = sz.livePhase || '';
      } else {
        m.textContent = sz.status === 'blockiert' ? (name || 'Noki') + ' · blockiert' : (sz.modell ? laneText(sz.modell) : '');
        ph.textContent = '';
      }
      l.classList.toggle('laeuft', !!sz.laeuft);
      k.hidden = !sz.laeuft;
      l.hidden = !sz.laeuft && !(sz.liveEnde && m.textContent);
    }
    function livePhase(sz, zustand, phase) {
      if (zustand) sz.liveZustand = zustand;
      if (phase != null) sz.livePhase = String(phase).replace(/\s+/g, ' ').trim().slice(0, 140);
      liveSetzen(sz);
    }
    function sKontext(sz) {
      var p = sz.pane; if (!p) return;
      p.querySelector('.nc-kontext').hidden = !sz.pfad;
      p.querySelector('.nc-k-modell').textContent = sz.modell ? laneText(sz.modell) : (sz.laeuft ? 'Modell wird gewählt …' : '');
      var st = p.querySelector('.nc-k-status');
      st.textContent = sz.laeuft ? '' : sz.status === 'fertig' ? '✓ fertig' : sz.status === 'blockiert' ? '× blockiert' : sz.status === 'mit Fehlern' ? '! mit Fehlern' : '';
      st.dataset.k = sz.laeuft ? 'busy' : sz.status === 'fertig' ? 'ok' : sz.status ? 'err' : '';
      p.querySelector('.nc-k-projekt').textContent = sz.pfad ? 'Projekt · ' + String(sz.pfad).split('/').pop() : '';
      p.querySelector('.nc-k-loesen').hidden = !sz.pfad || !!sz.laeuft;
      liveSetzen(sz);
      if (activeTab === sz.id) { kopfSetzen(); renderBar(); }
      renderBench(); renderTabs();
    }
    // Terminal history = the project's persistent record (restores after
    // restarts): every task exactly as submitted, mode, model, steps, result.
    function sBanner(sz) {
      var b = el('div', 'nc-banner');
      b.appendChild(el('pre', 'nc-ascii-banner', ASCII_WORDMARK));
      b.appendChild(el('div', 'nc-banner-sub', 'Noki Code · beschreibe deine Coding-Aufgabe'));
      sAdd(sz, b);
    }
    function akteurText(l) {
      if (l.extern) return (l.modell && l.modell.display_name ? l.modell.display_name : (l.akteur || 'Extern')) + ' · direkter Coding-Lauf';
      return 'Noki Code' + (l.modell ? ' · ' + laneText(l.modell) : '');
    }
    function sVerlauf(sz) {
      sz.log.textContent = ''; sz.letztesModell = '';
      sBanner(sz);
      var pj = sz.projekt;
      if (!pj) { sNote(sz, 'Beschreibe eine Coding-Aufgabe – Noki legt dafür ein eigenes Projekt unter ~/Documents/Noki/Code an. Für eine normale Shell öffne mit + ein Terminal.'); sAdd(sz, el('div', 'nc-rule')); return; }
      (pj.laeufe || []).forEach(function (l) {
        var ts = l.start || 0;
        // Who worked in this run: Noki Code (router model) or an external actor.
        sEv(sz, 'LAUF', akteurText(l), { old: true, ts: ts });
        if (l.importiert) sNote(sz, 'Importierter Verlauf · ' + (l.quelle || 'aus tatsächlichen Dateiänderungen rekonstruiert'));
        // Inputs to external actors are labelled as theirs, never as a Noki Terminal user line.
        if (l.extern) (l.eingaben || []).forEach(function (e) { sEv(sz, 'EINGABE', (l.akteur || 'Extern') + (e.quelle ? ' · ' + e.quelle : '') + '\n' + (e.text || ''), { old: true, ts: e.zeit ? e.zeit / 1000 : ts }); });
        else sUser(sz, l.aufgabe || '', ts, true);
        if (l.status !== 'läuft' && !(l.schritte || []).some(function (x) { return x.art === 'datei' && x.ok; })) sNote(sz, 'Keine Codeänderungen in diesem Lauf.');
        sEv(sz, 'NOKI', 'Projekt ' + (pj.name || '') + ' · Modus ' + modusText(l.modus || sz.modus), { old: true, ts: ts });
        sz.letztesModell = '';
        if (l.extern && l.modell) sModell(sz, l.modell, true, ts);
        (l.schritte || []).forEach(function (x) { sSchritt(sz, x, true, x.zeit ? x.zeit / 1000 : ts); });
        if (!l.extern && l.modell && !(l.schritte || []).some(function (x) { return x.art === 'modell'; })) sModell(sz, l.modell, true, ts);
        if (l.status === 'läuft' && !sz.laeuft) l.status = 'unterbrochen';
        if (l.status && l.status !== 'läuft') sErgebnis(sz, l.status, l.sekunden, l.ergebnis, null, true, ts,
          { stats: l.stats, audit: l.audit, pruefung: l.pruefung, modell: l.modell,
            schreibschritte: (l.schritte || []).filter(function (x) { return x.art === 'datei'; }).length,
            pruefrunden: (l.schritte || []).filter(function (x) { return x.art === 'audit'; }).length });
      });
      sAdd(sz, el('div', 'nc-rule'));
    }
    // ── Verlauf (⋯): the REAL history of what is open here ──────────────
    // Bound Noki Terminal → that project's timeline (all actors). Unbound →
    // the terminal's own conversation. Shell terminal → the project an agent
    // was recorded in from this terminal. Same drawer as the Work library.
    var vDrawer = null;
    function verlaufUmschalten(an) {
      if (!vDrawer) {
        vDrawer = el('div', 'ni-drawer nc-verlauf'); vDrawer.hidden = true;
        (root.parentElement || root).appendChild(vDrawer);
      }
      var offen = typeof an === 'boolean' ? an : vDrawer.hidden;
      vDrawer.hidden = !offen;
      if (offen) verlaufListe();
    }
    function vKopf(titel, hinweis, zurueck) {
      vDrawer.textContent = '';
      var kopf = el('div', 'ni-drawer-kopf'), tw = el('div', 'ni-drawer-titel-wrap');
      tw.appendChild(el('strong', null, titel)); tw.appendChild(el('span', null, hinweis || ''));
      var acts = el('div', 'ni-drawer-actions');
      if (zurueck) { var b = el('button', 'ni-drawer-new-btn', '← Verlauf'); b.type = 'button'; b.onclick = verlaufListe; acts.appendChild(b); }
      var x = el('button', 'ni-drawer-close-btn', '×'); x.type = 'button'; x.title = 'Verlauf schließen'; x.onclick = function () { verlaufUmschalten(false); };
      acts.appendChild(x); kopf.appendChild(tw); kopf.appendChild(acts); vDrawer.appendChild(kopf);
    }
    function vLeer(text) { var d = el('div', 'ni-drawer-empty'); d.appendChild(el('p', null, text)); vDrawer.appendChild(d); }
    // One calm grouped list (Noki Einstellungen language): the task is the
    // primary line, actor · model · time · status secondary; a row opens in
    // place to changes, checks and result. Full texts stay complete - long
    // ones start as a preview with "Ganzen Text zeigen".
    // Confirmation in place (no browser dialog): "Verlauf wirklich löschen?"
    function vBestaetigen(nach, frage, los) {
      if (nach.nextSibling && nach.nextSibling.classList && nach.nextSibling.classList.contains('nc-vl-confirm')) return;
      var c = el('div', 'nc-vl-confirm'); c.appendChild(el('span', null, frage));
      var ja = el('button', 'nc-btn nc-danger-btn', 'Löschen'); ja.type = 'button';
      var nein = el('button', 'nc-btn', 'Abbrechen'); nein.type = 'button';
      nein.onclick = function (e) { e.stopPropagation(); c.remove(); };
      ja.onclick = function (e) { e.stopPropagation(); ja.disabled = true; los(); };
      c.appendChild(ja); c.appendChild(nein); nach.parentNode.insertBefore(c, nach.nextSibling);
    }
    function vZeile(liste, primaer, meta, aufbauen, loeschen) {
      var it = el('div', 'nc-vl-z'), kopf = el('button', 'nc-vl-kopf'); kopf.type = 'button';
      var t = el('div', 'nc-vl-txt');
      t.appendChild(el('div', 'nc-vl-titel', String(primaer || '').trim() || '—'));
      t.appendChild(el('div', 'nc-vl-meta', meta));
      kopf.appendChild(t);
      if (loeschen) {
        var x = el('span', 'nc-vl-x'); x.title = 'Verlauf löschen'; x.setAttribute('role', 'button'); x.setAttribute('aria-label', 'Verlauf löschen');
        x.onclick = function (e) { e.stopPropagation(); vBestaetigen(it, 'Verlauf wirklich löschen?', loeschen); };
        kopf.appendChild(x);
      }
      kopf.appendChild(el('span', 'nc-vl-pfeil'));
      var det = el('div', 'nc-vl-det'); det.hidden = true;
      kopf.onclick = function () {
        if (!det.childNodes.length) aufbauen(det);
        det.hidden = !det.hidden; it.classList.toggle('offen', !det.hidden);
        kopf.setAttribute('aria-expanded', String(!det.hidden));
      };
      it.appendChild(kopf); it.appendChild(det); liste.appendChild(it);
    }
    function vText(box, titel, text) {
      text = String(text || '');
      if (titel) box.appendChild(el('div', 'nc-vl-h', titel));
      var pre = el('pre', 'nc-vl-pre', text || '—'); box.appendChild(pre);
      if (text.length > 320 || text.split('\n').length > 6) {
        pre.classList.add('zu');
        var b = el('button', 'nc-vl-mehr', 'Ganzen Text zeigen'); b.type = 'button';
        b.onclick = function () { var zu = pre.classList.toggle('zu'); b.textContent = zu ? 'Ganzen Text zeigen' : 'Weniger zeigen'; };
        box.appendChild(b);
      }
    }
    function vLaufMeta(l) {
      var s = l.schritte || [], dateien = s.filter(function (x) { return x.art === 'datei'; });
      var plus = dateien.reduce(function (a, x) { return a + (x.plus || 0); }, 0), minus = dateien.reduce(function (a, x) { return a + (x.minus || 0); }, 0);
      var tests = s.filter(function (x) { return x.art === 'test'; }).length;
      var st = l.status === 'läuft' && !(sitzung(NOKI_TERM) || {}).laeuft ? 'unterbrochen' : (l.status || '–');
      var m = [akteurText(l)];
      if (l.start) m.push(datumZeit(l.start));
      m.push(st);
      if (dateien.length) m.push(dateien.length + (dateien.length === 1 ? ' Datei' : ' Dateien') + ' +' + plus + ' −' + minus);
      if (tests) m.push(tests + (tests === 1 ? ' Prüfung' : ' Prüfungen'));
      if (l.importiert) m.push('rekonstruiert');
      return m.join(' · ');
    }
    function vProjektListe(pj) {
      vKopf('Verlauf · ' + (pj.name || 'Projekt'), (pj.laeufe || []).length + ' Läufe · ' + modusText(pj.modus || '') + (pj.erstellt ? ' · erstellt ' + datumZeit(pj.erstellt) : ''));
      var liste = el('div', 'nc-vl-liste');
      (pj.laeufe || []).slice().reverse().forEach(function (l) {
        var eing = l.eingaben && l.eingaben.length ? l.eingaben : null;
        vZeile(liste, (eing ? eing[0].text : l.aufgabe) || l.aufgabe, vLaufMeta(l), function (det) { vLaufDetails(det, l); });
      });
      if ((pj.laeufe || []).length) vDrawer.appendChild(liste); else vLeer('Dieses Projekt hat noch keine Läufe.');
    }
    function vLaufDetails(box, l) {
      if (l.importiert) box.appendChild(el('div', 'nc-vl-hinweis', 'Importierter Verlauf · ' + (l.quelle || 'rekonstruiert')));
      if (l.modell_quelle) box.appendChild(el('div', 'nc-vl-hinweis', 'Modellangabe: ' + l.modell_quelle));
      var eing = l.eingaben && l.eingaben.length ? l.eingaben : [{ quelle: 'Eingabe', text: l.aufgabe || '' }];
      eing.forEach(function (e) {
        vText(box, (l.extern ? (l.akteur || 'Extern') + ' · ' : '') + (e.quelle || 'Eingabe') + (e.zeit ? ' · ' + datumZeit(e.zeit) : ''), e.text);
      });
      var s = l.schritte || [];
      var uhr = function (x) { return x.zeit ? (datumZeit(x.zeit).split(' · ')[1] || '') : ''; };
      var dateien = s.filter(function (x) { return x.art === 'datei'; });
      box.appendChild(el('div', 'nc-vl-h', 'Änderungen'));
      if (!dateien.some(function (x) { return x.ok; })) box.appendChild(el('div', 'nc-vl-leise', 'Keine Codeänderungen in diesem Lauf.'));
      dateien.forEach(function (x) {
        var z = el('button', 'nc-vl-datei'); z.type = 'button';
        z.appendChild(el('span', 'nc-vl-dn', x.label || ''));
        z.appendChild(el('span', 'nc-vl-dz', '+' + (x.plus || 0) + ' −' + (x.minus || 0)));
        box.appendChild(z);
        if (x.diff) {
          var d = diffBlock(x.diff, true); d.hidden = true; box.appendChild(d);
          z.onclick = function () { d.hidden = !d.hidden; z.classList.toggle('offen', !d.hidden); };
        }
      });
      var ablauf = s.filter(function (x) { return x.art !== 'datei'; });
      if (ablauf.length) {
        box.appendChild(el('div', 'nc-vl-h', 'Ablauf & Prüfungen'));
        var ol = el('div', 'nc-vl-ablauf');
        ablauf.forEach(function (x) {
          var r = el('div', 'nc-vl-schritt' + (x.ok === false ? ' err' : x.art === 'test' && x.ok ? ' ok' : ''));
          r.appendChild(el('span', 'nc-vl-zeit', uhr(x)));
          var txt = x.art === 'modell' ? 'Modell · ' + laneText(x.modell || { display_name: x.label, execution_lane: x.detail })
            : (x.art === 'notiz' ? '' : (x.label || '') + (x.detail ? ' · ' : '')) + String(x.detail || '');
          r.appendChild(el('span', 'nc-vl-st', txt));
          ol.appendChild(r);
        });
        box.appendChild(ol);
      }
      if (l.ergebnis) vText(box, 'Resultat', l.ergebnis);
    }
    function verlaufListe() {
      if (!vDrawer) return;
      if (activeTab === NOKI_TERM) {
        var sz = sitzung(NOKI_TERM);
        if (sz && sz.pfad) {
          call('code_projekt_info', { pfad: sz.pfad }).then(vProjektListe).catch(function (e) { vKopf('Verlauf'); vLeer(String(e)); });
          return;
        }
        call('code_terminal_gespraech', { sitzung: NOKI_TERM }).then(function (turns) {
          vKopf('Verlauf · Noki Terminal', (turns || []).length + ' Gespräche · kein Projekt gebunden');
          // Deleting removes CHAT history only - never a project, its files or runs.
          var leerAnsicht = function () { var nz = sitzung(NOKI_TERM); if (nz && !nz.pfad && nz.log) { nz.log.textContent = ''; if (typeof sBanner === 'function') sBanner(nz); } };
          if ((turns || []).length) {
            var alle = el('button', 'ni-drawer-new-btn nc-vl-alle', 'Diesen Verlauf löschen'); alle.type = 'button';
            var acts = vDrawer.querySelector('.ni-drawer-actions'); if (acts) acts.insertBefore(alle, acts.firstChild);
            alle.onclick = function () {
              vBestaetigen(vDrawer.querySelector('.ni-drawer-kopf'), 'Verlauf wirklich löschen?', function () {
                call('code_terminal_gespraech_loeschen', { sitzung: NOKI_TERM, zeit: null }).then(function () { leerAnsicht(); verlaufListe(); }).catch(function (e) { vLeer(String(e)); });
              });
            };
          }
          var liste = el('div', 'nc-vl-liste');
          (turns || []).slice().reverse().forEach(function (t) {
            var wer = t.modell && t.modell.display_name ? 'Noki · ' + (laneText(t.modell) || t.modell.display_name) : 'Noki';
            vZeile(liste, t.frage, [wer, datumZeit(t.zeit)].join(' · '), function (det) {
              vText(det, 'Eingabe', t.frage);
              vText(det, 'Antwort', t.antwort);
            }, function () {
              call('code_terminal_gespraech_loeschen', { sitzung: NOKI_TERM, zeit: t.zeit }).then(verlaufListe).catch(function (e) { vLeer(String(e)); });
            });
          });
          if ((turns || []).length) vDrawer.appendChild(liste);
          if (!(turns || []).length) vLeer('Noch keine Gespräche im Noki Terminal. Coding-Verläufe stehen in den Projekten (Zum Projekt).');
        }).catch(function (e) { vKopf('Verlauf'); vLeer(String(e)); });
        return;
      }
      // Shell terminal: the project in which a coding agent from THIS terminal was recorded.
      call('code_projekte').then(function (alle) {
        var treffer = (alle || []).filter(function (m) { return (m.laeufe || []).some(function (l) { return l.terminal === activeTab; }); })[0];
        if (treffer) { vProjektListe(treffer); return; }
        vKopf('Verlauf · Terminal'); vLeer('In diesem Terminal wurde noch kein Coding-Agent in einem Noki-Projekt aufgezeichnet.');
      }).catch(function (e) { vKopf('Verlauf'); vLeer(String(e)); });
    }

    // Release the project binding (explicit act; nothing is deleted).
    function sLoesen(sz) {
      call('code_sitzung_loesen', { id: sz.id }).then(function () {
        sz.pfad = null; sz.projekt = null; sz.status = ''; sz.modell = null; sz.liveEnde = false;
        sVerlauf(sz); sKontext(sz);
        sNote(sz, 'Projekt gelöst – das Noki Terminal ist wieder ungebunden. Das Projekt bleibt unter „Projekte“ erhalten.');
      }).catch(function (e) { sNote(sz, String(e), 'err'); });
    }
    function sSenden(sz) {
      if (activeTab !== sz.id || sz.pane.hidden) return; // a hidden terminal never sends
      var text = sz.input.value.trim(); if (!text) return;
      sz.input.value = ''; sz.input.style.height = '';
      if (text === '/lösen' || text === '/loesen') { sLoesen(sz); return; }
      sz.stick = true; sz.histPos = -1;
      if (sz.verlauf[sz.verlauf.length - 1] !== text) sz.verlauf.push(text);
      Promise.resolve(false).then(function () {
        if (sz.laeuft) { sNote(sz, 'Noki arbeitet in diesem Terminal noch an einer Aufgabe – ⌃C bricht ab. Befehle laufen weiter in der Shell.', 'warn'); sz.input.value = text; return; }
        call('code_sitzung_senden', { id: sz.id, text: text }).then(function (r) {
          if (r && r.pfad) sz.pfad = r.pfad;
        }).catch(function (e) { sz.input.value = text; sNote(sz, String(e), 'err'); });
      });
    }
    // The terminal's own real shell: spawned on first use, in the project
    // folder when there is one. Interactive tools (claude, antigravity) get
    // the full terminal.
    function sShell(sz) {
      if (sz.shell) return Promise.resolve(sz.shell);
      var slot = sz.pane.querySelector('.nc-ts-slot');
      var id = 'sz-' + sz.id;
      var cols = 100, rows = 30;
      if (slot.clientWidth > 40 && slot.clientHeight > 40) {
        var m = measureCharSize(slot);
        cols = Math.max(20, Math.floor((slot.clientWidth - 20) / (m.w || 7.55)));
        rows = Math.max(5, Math.floor((slot.clientHeight - 8) / (m.h || 18)));
      }
      var cwd = sz.pfad || '~';
      return call('intelligence_shell_spawn', { id: id, title: 'zsh · ' + sz.titel, cwd: cwd, cols: cols, rows: rows }).then(function () {
        sz.shell = createShellPane(id, 'zsh · ' + sz.titel, cwd, cols, rows);
        slot.appendChild(sz.shell.paneEl);
        return sz.shell;
      });
    }
    function sAnsicht(sz, v) {
      sz.ansicht = v;
      var shellAn = v === 'shell';
      sz.pane.querySelector('.nc-noki-view').hidden = shellAn;
      sz.pane.querySelector('.nc-tab-shell').hidden = !shellAn;
      if (shellAn) {
        sShell(sz).then(function (sh) { if (sh.fit) sh.fit(); sh.focus(); })
          .catch(function (e) { sAnsicht(sz, 'noki'); sNote(sz, 'Shell konnte nicht gestartet werden: ' + String(e), 'err'); });
      } else {
        setTimeout(function () { sz.input.focus(); }, 10);
      }
    }
    function sBefehl(sz, befehl) {
      sAdd(sz, eventRowEl({ kind: 'User', summary: '$ ' + befehl, timestamp: Date.now() / 1000 }));
      var neu = !sz.shell;
      sAnsicht(sz, 'shell');
      sShell(sz).then(function () {
        setTimeout(function () { call('intelligence_shell_write', { id: 'sz-' + sz.id, data: befehl + '\r' }).catch(function () {}); }, neu ? 450 : 0);
      }).catch(function () {});
    }
    function sLaden() {
      return call('code_sitzungen').then(function (liste) {
        (liste || []).filter(function (d) { return d.id === 'noki'; }).forEach(function (d) {
          var sz = sitzung(d.id);
          if (!sz) { sz = { id: d.id }; sitzungen.push(sz); }
          sz.titel = d.titel; sz.modus = d.modus; sz.pfad = d.pfad; sz.laeuft = !!d.laeuft; sz.projekt = d.projekt || null;
          var l = sz.projekt && sz.projekt.laeufe ? sz.projekt.laeufe[sz.projekt.laeufe.length - 1] : null;
          sz.status = sz.laeuft ? '' : (l && l.status) || '';
          sz.modell = (l && l.modell) || null;
          if (!sz.pane) { sPane(sz); sVerlauf(sz); }
          sKontext(sz);
        });
        renderTabs();
      });
    }
    function sNeu(pfad) {
      return call('code_sitzung_anlegen').then(function (d) {
        var sz = { id: d.id, titel: d.titel, modus: d.modus, pfad: null, projekt: null };
        sitzungen.push(sz); sPane(sz);
        var weiter = pfad ? call('code_sitzung_projekt', { id: sz.id, pfad: pfad }).then(function (x) { sz.pfad = x.pfad; sz.projekt = x.projekt; }) : Promise.resolve();
        return weiter.then(function () { sVerlauf(sz); sKontext(sz); showTerm(); switchTab(sz.id); return sz; });
      });
    }
    function renderBench() {
      // No dashboard: each terminal carries its own project line.
      var mit = [];
      bench.hidden = true;
      if (bench.hidden) return;
      bench.textContent = '';
      mit.forEach(function (x) {
        var r = el('div', 'nc-b-row' + (activeTab === x.id ? ' on' : '')); r.dataset.tab = x.id;
        r.appendChild(el('span', 'nc-b-mode', modusText(x.modus).toUpperCase()));
        r.lastChild.dataset.m = x.modus;
        r.appendChild(el('span', 'nc-b-t', x.titel));
        r.appendChild(el('span', 'nc-b-p', x.projekt ? x.projekt.name : home(x.pfad)));
        r.appendChild(el('span', 'nc-b-m', x.modell ? laneText(x.modell) : '—'));
        r.appendChild(el('span', 'nc-b-s', x.laeuft ? 'arbeitet' : x.status === 'fertig' ? '✓ fertig' : x.status || '—'));
        var b = el('button', 'nc-btn nc-b-v', modusText(x.modus) + '-Vorschau öffnen'); b.type = 'button';
        b.dataset.sv = x.pfad; b.disabled = !(x.projekt && x.projekt.vorschau);
        r.appendChild(b);
        bench.appendChild(r);
      });
    }
    bench.addEventListener('click', function (e) {
      var b = e.target.closest('[data-sv]');
      if (b) { e.stopPropagation(); call('code_vorschau_oeffnen', { pfad: b.dataset.sv }).catch(function () {}); return; }
    });
    if (window.__TAURI__ && window.__TAURI__.event) {
      // `noki code` started / ended in a terminal (by the user, in it).
      window.__TAURI__.event.listen('code-terminal', function (e) {
        var p = (e && e.payload) || {};
        if (!p.terminal) return;
        if (p.aktiv) nokiCode[p.terminal] = Object.assign(nokiCode[p.terminal] || {}, { sitzung: p.sitzung, modus: p.modus || 'functional', pfad: p.pfad || (nokiCode[p.terminal] || {}).pfad });
        else delete nokiCode[p.terminal];
        if (p.terminal === activeTab) modusSichtbar();
      });
      window.__TAURI__.event.listen('code-sitzung', function (e) {
        var p = (e && e.payload) || {};
        if (String(p.sitzung || '').indexOf('term:') === 0) {
          var tid = String(p.sitzung).slice(5), nc = nokiCode[tid];
          if (nc) {
            if (p.typ === 'modell') nc.modell = p;
            if (p.typ === 'start') { nc.pfad = p.pfad; nc.laeuft = true; }
            if (p.typ === 'ende') nc.laeuft = false;
            if (tid === activeTab) kopfSetzen();
          }
          return;
        }
        var sz = sitzung(p.sitzung);
        if (!sz) return;
        var ts = (p.zeit || Date.now()) / 1000;
        if (p.typ === 'start') {
          sz.laeuft = true; sz.status = ''; sz.pfad = p.pfad; sz.modell = null; sz.letztesModell = ''; sz.stick = true;
          sz.liveEnde = false; sz.reparatur = false; sz.liveZustand = 'arbeitet'; sz.livePhase = 'Projekt wird geladen';
          if (!sz.projekt || sz.projekt.pfad !== p.pfad) sz.projekt = { name: p.projekt, pfad: p.pfad, vorschau: false, laeufe: [] };
          if (p.akteur) sEv(sz, 'LAUF', p.akteur + ' · direkter Coding-Lauf', { ts: ts });
          sUser(sz, p.aufgabe || '', ts);
          sEv(sz, 'NOKI', (p.neu ? 'Projekt erstellt · ' : 'Projekt geöffnet · ') + home(p.pfad), { ts: ts });
        } else if (p.typ === 'noki') {
          sEv(sz, 'NOKI', p.text || '', { ts: ts });
        } else if (p.typ === 'frage') {
          // A conversation turn (or an input typed into a recorded agent).
          sUser(sz, p.text || '', ts);
          if (p.gespraech) { sz.laeuft = true; sz.liveEnde = false; sz.chatModell = sz.modell; sz.modell = { display_name: 'Noki' }; sz.liveZustand = 'antwortet'; sz.livePhase = p.projekt ? 'Gespräch · Projekt ' + p.projekt + ' (nur lesen)' : 'Gespräch'; }
        } else if (p.typ === 'agent_eingabe') {
          // Recorded input of a terminal agent - history of that agent, not a Noki Terminal message.
          sEv(sz, 'EINGABE', (p.akteur || 'Agent') + (p.terminal ? ' · ' + p.terminal : '') + '\n' + (p.text || ''), { ts: ts });
        } else if (p.typ === 'antwort') {
          if (p.modell) sModell(sz, p.modell, false, ts);
          sEv(sz, 'NOKI', p.text || '', { ts: ts });
          sz.laeuft = false; sz.modell = sz.chatModell || null; sz.liveEnde = false;
        } else if (p.typ === 'info') {
          sNote(sz, p.text || '');
        } else if (p.typ === 'wechsel') {
          sEv(sz, 'WARNING', 'Modellwechsel · ' + (p.text || ''), { ts: ts });
          livePhase(sz, null, 'Modellwechsel');
        } else if (p.typ === 'modell') {
          sz.modell = p; sModell(sz, p, false, ts);
        } else if (p.typ === 'denkt') {
          livePhase(sz, 'generiert', sz.reparatur ? 'Überarbeite Laufzeitfehler' : 'Schritt ' + (p.schritt || 1));
        } else if (p.typ === 'phase') {
          livePhase(sz, 'arbeitet', p.text || '');
        } else if (p.typ === 'schritt') {
          sSchritt(sz, p, false, ts);
          var lb = String(p.label || ''), dt = String(p.detail || '');
          if (lb === 'Nachdenken') livePhase(sz, 'arbeitet', dt);
          else if (p.art === 'datei') livePhase(sz, 'arbeitet', p.ok ? 'Schreibe ' + (p.datei || 'Datei') : 'Änderung nicht angewendet');
          else if (p.art === 'test') {
            sz.reparatur = !p.ok && /^Laufzeitfehler/.test(dt);
            livePhase(sz, 'arbeitet', /^Laufzeit OK/.test(dt) ? 'Laufzeit OK · Vorschau geprüft' : /^Laufzeitfehler/.test(dt) ? 'Laufzeitfehler gefunden' : 'Vorschau geprüft');
          } else if (/^Zurück auf/.test(lb)) livePhase(sz, 'arbeitet', lb);
          else if (lb === 'Antwort verworfen') livePhase(sz, 'arbeitet', dt);
          if (/Vorschau/.test(p.label || '') && p.ok && sz.projekt) sz.projekt.vorschau = true;
        } else if (p.typ === 'ende') {
          sz.laeuft = false; sz.status = p.status || ''; sz.projekt = p.projekt || sz.projekt; sz.liveEnde = true;
          sErgebnis(sz, p.status, p.sekunden, p.text, null, false, ts, p);
        }
        sKontext(sz);
      });
    }

    // ── host messages ─────────────────────────────────────────────────
    function send(msg) { return call('intelligence_code_view_send', { msg: msg }).catch(function (e) { note(String(e), 'err'); }); }
    function onHost(msg) {
      if (!msg || !msg.t || !attached && msg.t !== 'detached') return;
      switch (msg.t) {
        case 'snapshot':
          meta = msg.session; clearView(true); banner();
          (msg.history || []).slice(-300).forEach(function (e) { add(eventRow(e, true)); });
          inputHistory = (msg.history || []).filter(function (e) { return e.kind === 'User'; }).map(function (e) { return e.summary; }).slice(-100);
          if (!(msg.history || []).length) note('Noch kein Verlauf. Beschreibe eine Aufgabe – JackOD prüft, plant und testet lokal.');
          add(el('div', 'nc-rule'));
          syncStatus(meta); renderBar();
          stick = true; screen.scrollTop = screen.scrollHeight;
          break;
        case 'session': meta = msg; syncStatus(msg); renderBar(); break;
        case 'event': add(eventRow(msg.event, false, msg.origin)); if (msg.event.kind === 'User' && msg.origin === 'terminal') inputHistory.push(msg.event.summary); break;
        case 'confirm':
          if (pending && pending.id === msg.id) break;
          if (msg.mine) {
            showConfirm({ id: msg.id, head: 'Bestätigung erforderlich', lead: 'Noki möchte:', items: msg.items || [], question: 'Ausführen? [y/N]', yes: 'Ausführen',
              done: function (yes) { send({ t: 'confirm', id: msg.id, yes: yes }); } });
          } else note('☐ Bestätigung wartet im Terminal: ' + (msg.items || []).join(', '), 'warn');
          break;
        case 'confirm_done':
          if (pending && pending.id === msg.id) { pending = null; box.hidden = true; }
          break;
        case 'info': infoBlock(msg.lines); break;
        case 'notice': note(msg.text, msg.level === 'error' ? 'err' : msg.level === 'warn' ? 'warn' : ''); break;
        case 'history_cleared': clearView(true); banner(); note('Code-Verlauf gelöscht.'); inputHistory = []; break;
        case 'detached':
          attached = false;
          if (!term.hidden && reattach < 2) { reattach++; note('Session-Verbindung getrennt – verbinde neu …', 'warn'); setTimeout(attach, 600); }
          break;
      }
    }
    if (window.__TAURI__ && window.__TAURI__.event) window.__TAURI__.event.listen('noki-code', function (e) { onHost(e && e.payload); });

    // ── actions ────────────────────────────────────────────────────────
    function attach() {
      if (attaching) return;
      attaching = true;
      attached = true;
      call('intelligence_code_view_attach').then(function () { reattach = 0; })
        .catch(function (e) { attached = false; showOverview(); overErr(String(e)); })
        .finally(function () { attaching = false; });
    }
    function overErr(t) { var p = q('.nc-o-err'); p.hidden = !t; p.textContent = t || ''; }
    function showTerm() {
      overErr('');
      if (projSection) projSection.hidden = true;
      over.hidden = true; term.hidden = false; remember(true);
      renderTabs();
      switchTab(activeTab);
    }
    function showOverview() { if (projSection) projSection.hidden = true; term.hidden = true; over.hidden = false; menu.hidden = true; renderOverview(); }
    function openExternal() {
      overErr('');
      return call('intelligence_code_terminal_open').then(function (s) { status = s || status; onStatus(status); renderOverview(); })
        .catch(function (e) { if (term.hidden) overErr(String(e)); else note(String(e), 'err'); });
    }

    root.addEventListener('click', function (e) {
      var closeBtn = e.target.closest('[data-close]');
      if (closeBtn) {
        e.stopPropagation();
        closeShellTab(closeBtn.dataset.close);
        return;
      }

      var zu = e.target.closest('[data-close-sitzung]');
      if (zu) {
        e.stopPropagation();
        var zid = zu.dataset.closeSitzung;
        call('code_sitzung_schliessen', { id: zid }).then(function () {
          var sz = sitzung(zid);
          if (sz && sz.shell) { call('intelligence_shell_close', { id: 'sz-' + zid }).catch(function () {}); sz.shell.destroy(); }
          if (sz && sz.pane) sz.pane.remove();
          sitzungen = sitzungen.filter(function (x) { return x.id !== zid; });
          if (activeTab === zid) switchTab('noki'); else renderTabs();
          renderBench();
        }).catch(function (err) { note(String(err), 'err'); });
        return;
      }
      var tabBtn = e.target.closest('[data-tab]');
      if (tabBtn) {
        if (projSection) projSection.hidden = true;
        term.hidden = false; over.hidden = true;
        switchTab(tabBtn.dataset.tab);
        return;
      }

      var b = e.target.closest('[data-act],[data-style]');
      if (!b || !root.contains(b)) return;
      if (b.dataset.act === 'p-terminal') { if (projSection) projSection.hidden = true; clearTimeout(projTimer); showTerm(); return; }
      // Funktional/Kreativ from the projects view: back to the terminal, then the mode applies.
      if (b.dataset.style && projSection && !projSection.hidden) { projSection.hidden = true; clearTimeout(projTimer); showTerm(); }
      if (b.dataset.style) {
        var nc = activeTab === NOKI_TERM ? { sitzung: NOKI_TERM } : nokiCode[activeTab];
        if (nc) {
          var neuM = b.dataset.style;
          if (activeTab === NOKI_TERM) {
            var istM = sitzung(NOKI_TERM);
            if (istM && istM.modus === neuM) { modusSichtbar(); return; }   // e.g. nav from Projekte: no repeat note
            call('code_sitzung_modus', { id: NOKI_TERM, modus: neuM }).then(function () {
              var nz = sitzung(NOKI_TERM); if (nz) { nz.modus = neuM; sNote(nz, modusText(neuM) + ' – gilt für die nächste Aufgabe.'); }
              modusSichtbar();
            }).catch(function () {});
            return;
          }
          call('code_sitzung_modus', { id: nc.sitzung, modus: neuM }).then(function () { nc.modus = neuM; modusSichtbar(); }).catch(function () {});
          return;
        }
        if (activeTab !== 'repo') return;
        if (!b.classList.contains('on')) {
          if (!meta) meta = {};
          meta.style = b.dataset.style;
          status.style = b.dataset.style;
          renderBar();
          send({ t: 'style', style: b.dataset.style });
        }
        return;
      }
      var act = b.dataset.act;
      if (act !== 'menu') menu.hidden = true;
      if (act === 'projekte') { projListe(); return; }
      if (act === 'add-terminal') { if (projSection) projSection.hidden = true; term.hidden = false; over.hidden = true; spawnShellTab(); return; }
      if (act === 'repo') { repoOffen = true; showTerm(); switchTab('repo'); return; }
      if (act === 'here') showTerm();
      else if (act === 'ext') openExternal();
      else if (act === 'close') { remember(false); showOverview(); }
      else if (act === 'add-shell') spawnShellTab();
      else if (act === 'menu') menu.hidden = !menu.hidden;
      else if (act === 'clear') { clearView(); input.focus(); }
      else if (act === 'status') send({ t: 'input', text: '/status' });
      else if (act === 'sessions') send({ t: 'input', text: '/sessions' });
      else if (act === 'wipe') askWipe();
      else if (act === 'settings') { if (opts.openSettings) opts.openSettings(); }
    });
    document.addEventListener('mousedown', function (e) { if (!menu.hidden && !e.target.closest('.nc-menu,[data-act="menu"],.ni-menu-btn')) menu.hidden = true; });
    if (headerEl) {
      headerEl.addEventListener('click', function (e) {
        var b = e.target.closest('[data-style]');
        if (b && !b.classList.contains('on')) {
          if (!meta) meta = {};
          meta.style = b.dataset.style;
          status.style = b.dataset.style;
          renderBar();
          send({ t: 'style', style: b.dataset.style });
          return;
        }
      });
    }

    function submit() {
      var text = input.value.trim();
      if (!text) return;
      input.value = ''; fit(); histPos = -1; draft = '';
      if (inputHistory[inputHistory.length - 1] !== text) inputHistory.push(text);
      if (text === '/clear') { clearView(); return; }
      if (text === '/clear-history') { askWipe(); return; }
      if (text === '/exit' || text === '/quit') { remember(false); showOverview(); return; }
      if (text === '/help') { infoBlock(HELP); return; }
      if (meta && meta.busy && text.charAt(0) !== '/') { note('JackOD arbeitet noch – Eingabe nicht gesendet. ⌃C bricht ab.', 'warn'); input.value = text; fit(); return; }
      stick = true;
      send({ t: 'input', text: text });
    }
    // AUTOHOEHE OHNE FLACKERN.
    //
    //  Frueher stand hier bei JEDEM Tastendruck erst height:'auto'. Damit
    //  faellt das Feld fuer die Dauer der Messung auf eine Zeile zusammen;
    //  Prompt-Zeichen, Text und Platzhalter sprangen sichtbar mit. Genau
    //  das war das "der Caret erscheint und verschwindet mit dem Text".
    //
    //  Gemessen wird deshalb nur noch, wenn es noetig ist: Wachsen laesst
    //  sich ohne Ruecksetzen erkennen (scrollHeight > clientHeight), und
    //  geschrumpft wird erst, wenn wirklich Text verschwunden ist. Beim
    //  reinen Tippen wird die Hoehe hoechstens EINMAL je neuer Zeile
    //  gesetzt — kein Layoutwechsel je Zeichen.
    var fitMax = 120, fitLetzte = 0;
    function fit() {
      var laenge = input.value.length;
      var kuerzer = laenge < fitLetzte;
      fitLetzte = laenge;
      if (!laenge) { fitLetzte = 0; if (input.style.height) input.style.height = ''; return; }
      if (kuerzer) {
        // Nur hier ist ein Ruecksetzen unvermeidlich: gewachsen bleibt die
        // Box sonst stehen, obwohl der Text nicht mehr so hoch ist.
        input.style.height = 'auto';
        var hK = Math.min(input.scrollHeight, fitMax);
        input.style.height = hK + 'px';
        return;
      }
      if (input.scrollHeight > input.clientHeight) {
        var hW = Math.min(input.scrollHeight, fitMax);
        if (hW !== input.clientHeight) input.style.height = hW + 'px';
      }
    }
    input.addEventListener('input', fit);
    q('.nc-prompt').addEventListener('submit', function (e) { e.preventDefault(); submit(); });
    input.addEventListener('keydown', function (e) {
      if ((e.metaKey || e.ctrlKey) && (e.key === '0' || e.code === 'Digit0' || e.keyCode === 48)) {
        e.preventDefault();
        e.stopPropagation();
        if (typeof window.toggleWarp === 'function') window.toggleWarp();
        return;
      }
      if (e.isComposing) return;
      if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); return; }
      if (e.ctrlKey && (e.key === 'c' || e.key === 'C')) {
        e.preventDefault();
        if (meta && meta.busy) { send({ t: 'cancel' }); note('⌃C Abbruch angefordert', 'warn'); }
        else { if (input.value) note('› ' + input.value + '  ^C'); input.value = ''; fit(); }
        return;
      }
      if (e.metaKey && (e.key === 'k' || e.key === 'K')) { e.preventDefault(); clearView(); return; }
      var firstLine = input.value.lastIndexOf('\n', input.selectionStart - 1) < 0;
      var lastLine = input.value.indexOf('\n', input.selectionEnd) < 0;
      if (e.key === 'ArrowUp' && firstLine && inputHistory.length) {
        e.preventDefault();
        if (histPos < 0) { draft = input.value; histPos = inputHistory.length; }
        histPos = Math.max(0, histPos - 1); input.value = inputHistory[histPos]; fit();
      } else if (e.key === 'ArrowDown' && lastLine && histPos >= 0) {
        e.preventDefault();
        histPos++;
        if (histPos >= inputHistory.length) { histPos = -1; input.value = draft; } else input.value = inputHistory[histPos];
        fit();
      }
    });
    term.addEventListener('keydown', function (e) {
      if ((e.metaKey || e.ctrlKey) && (e.key === '0' || e.code === 'Digit0' || e.keyCode === 48)) {
        e.preventDefault();
        e.stopPropagation();
        if (typeof window.toggleWarp === 'function') window.toggleWarp();
        return;
      }
      if (e.key === 'Escape') {
        if (!menu.hidden) { menu.hidden = true; }
      } else if (e.metaKey && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault(); clearView();
      }
    });
    screen.addEventListener('mouseup', function () { if (!String(window.getSelection() || '')) input.focus(); });

    function poll() {
      clearTimeout(pollT);
      if (!entered) return;
      call('intelligence_code_terminal_status').then(function (s) {
        if (!s) return;
        status = s; onStatus(s); renderOverview();
      }).catch(function () {}).finally(function () { pollT = setTimeout(poll, attached ? 15000 : 4000); });
    }

    return {
      enter: function (s) {
        entered = true;
        if (s) status = s;
        // One mode is selected from the first frame on, before any session
        // message arrives – the picker is never shown with nothing marked.
        renderBar();
        laufzeitMenue();
        // Code Space opens on its terminals. Nothing is auto-started.
        showTerm();
        poll();
        call('intelligence_shell_list').then(function (running) {
          (Array.isArray(running) ? running : []).forEach(function (info) {
            if (!info || !info.running || shellTabs.some(function (st) { return st.id === info.id; })) return;
            if (String(info.id).indexOf('sz-') === 0 || info.id === 'noki-terminal') return;
            var shell = createShellPane(info.id, info.title, info.cwd);
            shellTabs.push(shell);
            shellsView.appendChild(shell.paneEl);
          });
        }).catch(function () {}).then(sLaden).catch(function () {}).then(function () { switchTab(activeTab); });
      },
      leave: function () {
        entered = false; clearTimeout(pollT);
        if (attached) { attached = false; call('intelligence_code_view_detach').catch(function () {}); }
        pending = null; box.hidden = true;
        if (menu) menu.hidden = true;
      },
      status: function (s) { if (s) { status = s; renderOverview(); renderBar(); } },
      setStyle: function (s) { send({ t: 'style', style: s }); },
      toggleMenu: function () { menu.hidden = !menu.hidden; },
      // "⋯" in Code: the Verlauf of the current terminal / project.
      toggleVerlauf: function () { verlaufUmschalten(); },
      // Code Space on one project (chat card "Code öffnen", live build).
      projekt: function (pfad) { entered = true; projListe(pfad || null); },
      projektSichtbar: function () { return !!((projSection && !projSection.hidden) || (!term.hidden && activeTab !== 'repo')); },
      projekte: function () { projListe(); },
      createShellPane: createShellPane
    };
  };
})();
