/* Noki Intelligence UI. All model text is rendered with textContent. */
(function () {
  'use strict';
  window.NokiIntelligence = { create: function (host) {
    var settings = { level: 'off', active_app: true, window_title: false, screen: false, ocr: false, shelf: true, patterns: false,
    ask: true, web: true, mcp: true, active_page: true, unload_min: 10, memory: false, memory_auto: false, noki_folder: true, selected_text: true, mode: 'normal',
      notify: true, notify_preview: true };
    var memOpen = false, memList = [], confirmClear = false, modelActionPending = null;
    var KIND = { user_preferences: 'Präferenz', project_context: 'Projekt', workflows: 'Ablauf', interaction_feedback: 'Feedback', facts: 'Fakt' };
    var state = { opened: false, rect: null, busy: false, settings: settings, status: null, activeEngine: 'local' };
    // Conversation: the active chat (session context) and the grouped history. Never long-term memory.
    var chat = null, chats = [], chatSeq = 0, assistantMode = 'work', codeStyle = 'functional', codeTerminalStatus = null, codeView = null, modeRequest = 0, modeSwitch = Promise.resolve(), verlaufOffen = false, verlaufLeerT = 0, schritte = [], vorgang = [], drawer, mic, modusEl, modusCurrent, micZustand = 'idle';
    // ONE central recording state. UI, native events and the recognizer never keep a competing copy;
    // everything shown is join(committedSegments) + currentInterimSegment.
    // UserRecordingSession: the ONLY lifetime of a recording. A RecognizerCycle (final, timeout,
    // restart, error) may never end it, never clear committedSegments and never send anything.
    var rec = { id: 0, active: false, committed: [], interim: '', basis: '', gen: -1, seq: -1, startedAt: 0, sending: false, stopRequested: false, audio: null,
      // §3: rein visueller, MONOTONER Snapshot. Er wächst nur und wird ausschliesslich bei
      // Senden/Abbrechen zurückgesetzt – eine Sprechpause kann damit nie Text vom Schirm nehmen.
      sichtSnapshot: '', blankFrames: 0, maxWorte: 0, wortRegress: 0 };
    var VOICE_DEBUG = [];
    function vlog(ereignis, extra) {
      var e = { t: Date.now(), e: ereignis, session: rec.id, gen: rec.gen, committed_len: micCommittedText().length };
      if (extra) for (var k in extra) e[k] = extra[k];
      VOICE_DEBUG.push(e); if (VOICE_DEBUG.length > 400) VOICE_DEBUG.shift();
      // §1/§2/§6: DEV-Mitschnitt laeuft automatisch, solange aufgenommen wird. Der User
      // startet Noki normal und muss nichts ablesen.
      if (rec.active || micZustand !== 'idle') { try { call('intelligence_voice_diag', { kind: 'UI', payload: e }); } catch (x) {} }
    }
    var voiceLive, voiceText, voiceInterim, voiceMeter, voiceSend, voiceCancel, voiceToggle, voiceStatus, voiceLeer, voiceBasis, voiceWorte, neuBtn;
    var unread = false, fertigT = 0, taskState = 'IDLE', indArt = 'zahnrad', ind = null, indModus = '', indRect = null;
    var panel, body, frueher, question, statusEl, modelIndicator, answer, input, send, tools, meta, devEl, feedback, tipKey, tipUntil = 0, generation = 0, dragAt = 0;
    var pendingTool = null, ready = false, saving = false, observing = false, context = null;
    // Does the OS-level Cmd+0 hotkey exist? Then this webview must not toggle too.
    // Assume native ownership until the asynchronous capability check says
    // otherwise. This closes the startup race in which one physical shortcut
    // was handled once by Carbon and once by this WebView.
    var cmd0Nativ = true;
    try { call('noki_cmd0_nativ').then(function (v) { cmd0Nativ = !!v; }).catch(function () {}); } catch (e) {}
    var invoke = host.invoke;
    function call(name, args) { return Promise.resolve().then(function () { return invoke(name, args || {}); }); }
    function refresh() { devZeile(); host.settingsChanged(); }
    // Diagnostic overlays (build number, voice word counters) are for
    // development only - never in the product chat.
    var DEV_DIAG = false;
    // Build number only for diagnostics, never in the product chat.
    function devZeile() { if (devEl) devEl.textContent = ''; }
    // Kurzfassung des Routerzustands fuer den Aenderungsvergleich — keine
    // zweite Logik, nur ein Fingerabdruck dessen, was die Ansicht darstellt.
    function routerSignatur(rr) {
      try {
      if (!rr) return '';
      var d = rr.current_defaults || {};
      return rr.engine_mode + '|' + d.work + '|' + d.coding + '|' +
        (rr.providers || []).map(function (p) {
          var q = p.quota_telemetry || {};
          return p.id + ':' + p.state + ':' + (p.remaining_usage || '') + ':' +
            [q.source, q.scope, q.request_limit, q.request_remaining, q.token_limit,
              q.token_remaining, q.neuron_limit, q.neuron_remaining, q.reset_at,
              q.cooldown_seconds, q.observed_at].join(':');
        }).join(',');
      } catch (e) { return 'unlesbar'; }
    }
    function refreshStatus() {
      return call('intelligence_status').then(function (r) {
        if (!r || !state.status) return;
        if (r.code_style) codeStyle = r.code_style;
        // Auch der Router zaehlt als Aenderung: Modus, Defaults und
        // Providerzustaende sind genau das, was die Engine-Ansicht zeigt.
        // Ohne diesen Vergleich bliebe die Seite nach einem Moduswechsel
        // oder einem 429 auf dem alten Stand stehen.
        var changed = state.status.loaded !== r.loaded || state.status.model !== r.model || state.status.thinking !== r.thinking || state.status.loading !== r.loading || state.status.code_style !== r.code_style
          || routerSignatur(state.status.router) !== routerSignatur(r.router);
        Object.keys(r).forEach(function (k) { state.status[k] = r[k]; });
        if (changed) { modellAnzeige(); refresh(); }
        laufzeitAnzeige(); gesichtSetzen();
        if (assistantMode === 'code') return call('intelligence_code_terminal_status').then(function (terminal) { codeTerminalStatus = terminal; renderCodeTerminal(); modellAnzeige(); });
      }).catch(function () {});
    }
    // ===================================================================
    //  NOKI FACE VIEW
    //
    //  KEINE zweite Character- oder Emotion-Engine. Der Desktop-Character
    //  bleibt die eine Instanz mit Lauf-, Flug- und Physikwerk; hier haengt
    //  nur ein PORTRAET an denselben semantischen Zustaenden, die Ask Noki
    //  ohnehin schon an den Character meldet (host.thinking / host.phase /
    //  host.explain) — plus dem Mikrofonzustand, den dieses Panel selbst
    //  besitzt. Es gibt hier keinen eigenen Zustandsspeicher neben jenem:
    //  gesichtZustand() LIEST nur ab.
    //
    //  Bewegung bleibt sparsam: blinzeln alle paar Sekunden, ein leichtes
    //  Mitschauen, sonst Ruhe. Ein Timer laeuft nur, solange das Panel
    //  offen ist — geschlossen wird er abgeraeumt (kein Dauerverbrauch).
    // ===================================================================
    var GESICHT_SVG = '';
    var gesichtEl = null, gesichtT = null, gesichtIst = '';
    function gesichtZustand() { return 'neutral'; }
    function gesichtSetzen() {}
    function gesichtBlinzeln() {}
    function gesichtPlanen() {}
    function gesichtAn() {}
    function modellAnzeige() {
      var brand = panel ? panel.querySelector('.ni-brand-title') : null;
      var s = state.status || {};
      if (assistantMode === 'code' && codeView && codeView.projektSichtbar && codeView.projektSichtbar()) {
        return;  // Code Space names its project + real model itself
      }
      if (assistantMode === 'code') {
        if (brand) brand.textContent = 'Noki Terminal';
        var codeActive = s.active_code_runtime_model || null;
        if (modelIndicator) {
          // Preserve the intentional startup branding. Only a successful Code
          // execution replaces it with its actual canonical provenance.
          modelIndicator.textContent = codeActive ? provenanceText(codeActive) : '';
          modelIndicator.title = codeActive && codeActive.canonical_model_id ? codeActive.canonical_model_id : 'Noki Terminal';
        }
      } else {
        if (brand) brand.textContent = 'Noki';
        // This badge is execution provenance, not a configured/default-model
        // preview. Until a request actually completes there is no model to name.
        var active = s.active_runtime_model || null;
        var label = modelText(active);
        if (modelIndicator) {
          modelIndicator.textContent = label ? (s.loading ? ('Lade ' + label + ' …') : label) : '';
          modelIndicator.dataset.loading = s.loading ? 'true' : 'false';
          modelIndicator.title = active && active.canonical_model_id ? active.canonical_model_id : (label || 'Noki');
        }
      }
    }
    // Die Laufzeitzeile ist reine ANZEIGE des Backend-Zustands
    // (status.runtime). Hier wird nichts mehr zusammengetextet: steht dort
    // kein Zustand, bleibt die Zeile bei ihrem letzten bekannten Wert,
    // statt eine Laufzeit zu behaupten.
    function laufzeitAnzeige() {
      if (!statusEl) return;
      var r = state.status && state.status.runtime;
      // This line is the execution environment, never the selected responder.
      // Engine details (llama.cpp, Metal) are diagnostics, not user status.
      var label = r && r.ready === false && r.label ? r.label : 'Bereit';
      statusEl.textContent = label;
      var box = statusEl.closest ? statusEl.closest('.ni-runtime-status') : statusEl.parentElement;
      if (box) {
        box.title = label;
        box.dataset.kind = (r && r.active_provider_kind) || 'local';
      }
    }
    // What the user needs to know about an answer - never the internal route,
    // and never "lokal" as a claim: the real lane + model are shown in the
    // provenance line below (Local / Free Cloud · model).
    var ROUTE_META = { LOCAL: 'Ohne Quelle', DETERMINISTIC_MATH: 'Exakt gerechnet', STABLE_KNOWLEDGE: 'Allgemeinwissen · ohne Quelle', FAST_LOCAL: 'Kurze Antwort · ohne Quelle', MEMORY: 'Aus deinem Noki-Memory', DESKTOP_CONTEXT: 'Aus freigegebenem Desktop-Kontext', WEB: 'Aus Webquellen',
      UNKNOWN: 'Nicht sicher · nichts erfunden', TASK: 'Erstellt · bitte prüfen' };
    function loadMem() { return call('intelligence_memory_list').then(function (r) { memList = r || []; if (state.status) state.status.memory_count = memList.length; }).catch(function (e) { state.error = String(e); }).finally(refresh); }
    var sources;
    var STATUS = { bereit: 'Bereit', laedt: 'Modell wird geladen …', prewarm: 'Wird vorbereitet …', denkt: 'Denkt nach …', recherche: 'Recherche läuft …', aus: 'Ask Noki deaktiviert', fehlt: 'Lokales Modell nicht verfügbar' };
    function setStatus(k) {
      if (statusEl) {
        if (k === 'bereit') { laufzeitAnzeige(); }
        else { statusEl.textContent = STATUS[k] || STATUS.bereit; }
        statusEl.dataset.k = k;
      }
    }
    // Canonical assistant-message display.  It retains raw Markdown in chat
    // state; the renderer builds only whitelisted DOM nodes and never trusts
    // model output as HTML. Status/error/user content deliberately stays text.
    function renderText(text) {
      if (window.NokiMarkdown && window.NokiMarkdown.render) {
        return window.NokiMarkdown.render(document, text, function (url) {
          call('intelligence_open_source', { url: url }).catch(function (e) { meta.textContent = String(e); });
        });
      }
      var box = document.createElement('div'); box.className = 'ni-text';
      var p = document.createElement('p'); p.textContent = String(text || ''); box.appendChild(p);
      return box;
    }
    // Content blocks: text | status | image | file. Sources render below via showSources.
    // image/file are only drawn from safe local data (no remote URLs); nothing produces them yet.
    function showBlocks(blocks) {
      answer.replaceChildren();
      (blocks || []).forEach(function (b) {
        var el = null;
        if (b.type === 'text') el = renderText(b.text);
        else if (b.type === 'status') { el = document.createElement('p'); el.className = 'ni-hint'; el.textContent = b.text; }
        else if (b.type === 'image' && /^data:image\/(png|jpeg|webp);base64,/.test(b.src || '')) { el = document.createElement('img'); el.className = 'ni-media'; el.src = b.src; el.alt = b.alt || ''; }
        else if (b.type === 'file' && b.name) { el = document.createElement('div'); el.className = 'ni-file'; el.textContent = b.name; }
        if (el) answer.appendChild(el);
      });
    }
    // Real pipeline phases from the native side (never model reasoning text).
    var PHASEN = { load: ['Noki wird vorbereitet …', 'Modell wird geladen'], analyze: ['Analysiere deine Frage …', 'Analysiert'],
      memory: [function (n) { return n ? 'Memory · ' + n + ' Treffer' : 'Memory · keine Treffer'; }, 'Durchsucht Memory'],
      route: ['Prüfe, ob aktuelle Infos nötig sind …', 'Analysiert'], desktop: ['Prüfe Desktop-Kontext …', 'Prüft Kontext'],
      search: ['Recherchiere im Web …', 'Recherchiert'], read: [function (n) { return 'Prüfe ' + (n || 1) + (n === 1 ? ' Quelle …' : ' Quellen …'); }, 'Prüft Quellen'],
      compose: ['Formuliere Antwort …', 'Formuliert'], verify: ['Überprüfe Aussagen …', 'Prüft Aussagen'] };
    // Echte Arbeitsschritte (aus den Phasen der nativen Seite, nie aus
    // Modelltext): je Phase ein "jetzt"-Text (laufend) und ein "fertig"-Text
    // (fuer "Vorgehen" unter der Antwort). Keine Zeitschaltung, kein Raten.
    var VORGANG = {
      analyze: [function () { return 'Noki analysiert die Anfrage …'; }, function () { return 'Anfrage analysiert'; }],
      memory: [function () { return 'Noki prüft das Gedächtnis …'; }, function (p) { return p.n ? 'Gedächtnis · ' + p.n + ' Treffer' : 'Gedächtnis geprüft'; }],
      route: [function () { return 'Noki plant …'; }, function () { return 'Vorgehen geplant'; }],
      desktop: [function () { return 'Noki prüft den Kontext …'; }, function () { return 'Kontext geprüft'; }],
      search: [function () { return 'Noki recherchiert …'; }, function () { return 'Im Web gesucht'; }],
      read: [function (p) { return 'Noki prüft ' + (p.n || 1) + (p.n === 1 ? ' Quelle …' : ' Quellen …'); }, function (p) { return (p.n || 1) + (p.n === 1 ? ' Quelle geprüft' : ' Quellen geprüft'); }],
      wechsel: [function (p) { return 'Noki lädt ' + (p.modell || 'das passende Modell') + ' …'; }, function (p) { return 'Modell · ' + (p.modell || 'gewechselt'); }],
      think: [function (p) { return 'Noki denkt …' + (p.n ? ' (≈ ' + p.n + ' Tokens)' : ''); }, function (p) { return 'Nachgedacht' + (p.n ? ' · ≈ ' + p.n + ' Tokens' : ''); }],
      verify: [function () { return 'Noki prüft die Aussagen …'; }, function () { return 'Aussagen geprüft'; }],
      compose: [function () { return 'Noki formuliert …'; }, function () { return 'Antwort formuliert'; }]
    };
    function vorgangSchritt(phase, p) {
      var v = VORGANG[phase]; if (!v) return false;
      var letzter = vorgang[vorgang.length - 1];
      // Dieselbe Phase aktualisiert ihren Schritt (z. B. Denk-Tokens, Modell geladen).
      if (letzter && letzter.phase === phase) { letzter.p = p; }
      else vorgang.push({ phase: phase, p: p });
      if (vorgang.length > 12) vorgang.shift();
      return true;
    }
    function vorgangZusammenfassung() {
      return vorgang.map(function (s) { return VORGANG[s.phase][1](s.p || {}); }).slice(-8);
    }
    function showSteps() {
      if (codeLauf && state.busy) { codeKarteZeigen(); return; }
      answer.replaceChildren(); var ul = document.createElement('ul'); ul.className = 'ni-steps ni-vorgang';
      if (!vorgang.length) {
        var li0 = document.createElement('li'); li0.className = 'aktiv';
        li0.textContent = schritte.length ? schritte[schritte.length - 1] : 'Noki denkt …';
        ul.appendChild(li0);
      }
      vorgang.forEach(function (st, i) {
        var li = document.createElement('li'), jetzt = i === vorgang.length - 1;
        li.className = jetzt ? 'aktiv' : 'fertig';
        li.textContent = VORGANG[st.phase][jetzt ? 0 : 1](st.p || {});
        ul.appendChild(li);
      });
      answer.appendChild(ul);
      liveModellZeigen();
    }
    // "Vorgehen · N Schritte" - eingeklappt ueber der Antwort, getrennt von ihr.
    function vorgehenZeigen(box, schritteListe) {
      if (!schritteListe || schritteListe.length < 2) return;
      var d = document.createElement('details'); d.className = 'ni-vorgehen';
      var s = document.createElement('summary'); s.textContent = 'Vorgehen · ' + schritteListe.length + ' Schritte'; d.appendChild(s);
      var ul = document.createElement('ul');
      schritteListe.forEach(function (t) { var li = document.createElement('li'); li.textContent = t; ul.appendChild(li); });
      d.appendChild(ul);
      box.insertBefore(d, box.firstChild);
    }
    function metaText(t) {
      if (t.tool) return !t.tool.available ? 'Blockiert · nichts geändert' : t.tool.confirm ? 'Nur mit deiner Bestätigung' : 'Ausführen erst mit deinem Klick';
      if (t.plan && t.plan.intent === 'conversation') return '';
      var basis = ROUTE_META[t.route] || ROUTE_META.UNKNOWN;
      if (t.route === 'WEB') { var n = (t.sources || []).length; if (n) basis = 'Aus ' + n + (n === 1 ? ' Webquelle' : ' Webquellen'); }
      // Only a LOW confidence is worth telling; "mittel"/"hoch" is noise.
      if (t.route !== 'TASK' && t.conf === 'niedrig') basis += ' · unsicher';
      if (t.code_actions && t.code_actions.length) basis += ' · ' + t.code_actions.filter(function(a){return a.ok}).length + '/' + t.code_actions.length + ' Tools erfolgreich';
      return basis;
    }
    function compactQuantization(value) {
      var match = String(value || '').match(/\b(Q\d+)\b/i);
      return match ? match[1].toUpperCase() : '';
    }
    function modelText(provenance) {
      if (!provenance || !provenance.display_name) return '';
      var quantization = compactQuantization(provenance.quantization);
      var name = String(provenance.display_name)
        .replace(/Qwen3\.5/g, 'Qwen 3.5')
        .replace(/^qwen3\.5:9b$/i, 'Qwen 3.5 9B')
        .replace(/^qwen3\.5:4b$/i, 'Qwen 3.5 4B');
      // The lane already says "Free Cloud": "Ministral 8B Free (Mistral)" -> "Ministral 8B (Mistral)".
      name = name.replace(/\s+Free\b/i, '').replace(/\s{2,}/g, ' ').trim();
      return name + (quantization ? ' · ' + quantization : '');
    }
    function provenanceText(provenance) {
      if (!provenance || !provenance.display_name) return '';
      var lane = { LOCAL: 'Local', FREE_CLOUD: 'Free Cloud', PAID_CLOUD: 'Paid Cloud' }[provenance.execution_lane] || '';
      var model = modelText(provenance);
      return lane ? lane + ' · ' + model : model;
    }
    // ---- Code task: real steps while Noki builds, result with files + preview.
    var codeLauf = null;
    function dateiName(d) { return String(d || '').replace(/\s+\(\d+ Zeilen\)$/, ''); }
    function formatDiffLines(diff) {
      if (!diff) return '';
      var lines = diff.split('\n');
      var out = [];
      lines.forEach(function (l) {
        var esc = l.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
        if (l.startsWith('+') && !l.startsWith('+++')) {
          out.push('<span class="ni-diff-add">' + esc + '</span>');
        } else if (l.startsWith('-') && !l.startsWith('---')) {
          out.push('<span class="ni-diff-del">' + esc + '</span>');
        } else if (l.startsWith('@@')) {
          out.push('<span class="ni-diff-hunk">' + esc + '</span>');
        } else {
          out.push('<span class="ni-diff-ctx">' + esc + '</span>');
        }
      });
      return out.join('\n');
    }

    function renderCodeProtokoll(box, schritte, modell, projektName, wechsel, isBusy) {
      box.replaceChildren();

      var kopf = document.createElement('div');
      kopf.className = 'ni-code-lauf-head';
      var kopfTitel = document.createElement('span');
      kopfTitel.className = 'ni-code-lauf-titel';
      kopfTitel.textContent = 'Noki Code · ' + (projektName || 'Projekt');
      kopf.appendChild(kopfTitel);

      var modText = modell ? provenanceText(modell) : '';
      if (modText) {
        var kopfModell = document.createElement('span');
        kopfModell.className = 'ni-code-lauf-modell';
        kopfModell.textContent = modText;
        kopf.appendChild(kopfModell);
      }
      box.appendChild(kopf);

      if (wechsel) {
        var wb = document.createElement('div');
        wb.className = 'ni-code-wechsel-box';
        wb.innerHTML = wechsel.split('\n').map(function (w) {
          return '<div class="ni-code-wechsel-zeile">' + w.replace(/&/g, '&amp;').replace(/</g, '&lt;') + '</div>';
        }).join('');
        box.appendChild(wb);
      }

      var sl = document.createElement('div');
      sl.className = 'ni-code-lauf-schritte';

      (schritte || []).forEach(function (st) {
        var el = document.createElement('div');
        el.className = 'ni-code-step-item' + (st.ok ? ' ok' : ' warn');

        var isNotiz = (st.art === 'notiz') || (st.label === 'Nachdenken') || (st.label === 'Projekt wird aufgebaut') || (st.label === 'Bestehendes Projekt geladen');
        var isDatei = (st.art === 'datei') || Boolean(st.datei) || Boolean(st.diff);
        var isTest = (st.art === 'test') || (st.label && st.label.indexOf('Vorschau') >= 0);
        var isAudit = (st.art === 'audit') || (st.label && st.label.indexOf('Anforderungsprüfung') >= 0);

        if (isNotiz) {
          el.classList.add('ni-step-notiz');
          var h = document.createElement('div');
          h.className = 'ni-code-step-h';
          h.innerHTML = '<span class="ni-code-glyph">◇</span> <strong>' + (st.label === 'Nachdenken' ? 'Analysiere / Denken' : st.label) + '</strong>';
          el.appendChild(h);
          if (st.detail) {
            var b = document.createElement('div');
            b.className = 'ni-code-step-notiz-body';
            b.textContent = st.detail;
            el.appendChild(b);
          }
        } else if (isDatei) {
          el.classList.add('ni-step-datei');
          var h = document.createElement('div');
          h.className = 'ni-code-step-h ni-datei-head';
          var nameSpan = document.createElement('span');
          var lbl = st.label || (st.datei ? 'Datei · ' + st.datei : 'Datei');
          nameSpan.textContent = lbl;
          h.appendChild(nameSpan);

          if (st.plus > 0 || st.minus > 0) {
            var badge = document.createElement('span');
            badge.className = 'ni-diff-badge';
            var html = '';
            if (st.plus > 0) html += '<span class="ni-diff-plus">+' + st.plus + '</span> ';
            if (st.minus > 0) html += '<span class="ni-diff-minus">-' + st.minus + '</span>';
            badge.innerHTML = html;
            h.appendChild(badge);
          }
          el.appendChild(h);

          if (st.diff) {
            var pre = document.createElement('pre');
            pre.className = 'ni-code-diff-block';
            pre.innerHTML = '<code>' + formatDiffLines(st.diff) + '</code>';
            el.appendChild(pre);
          }
        } else if (isTest) {
          el.classList.add('ni-step-test');
          var h = document.createElement('div');
          h.className = 'ni-code-step-h';
          h.innerHTML = '<span class="ni-code-glyph">◇</span> <strong>' + (st.label || 'Prüfe Vorschau') + '</strong>';
          el.appendChild(h);
          if (st.detail) {
            var tb = document.createElement('div');
            tb.className = 'ni-code-step-test-body';
            st.detail.split('\n').forEach(function (l) {
              if (!l.trim()) return;
              var ldiv = document.createElement('div');
              ldiv.className = 'ni-test-line' + (l.indexOf('FEHLER') >= 0 ? ' err' : '');
              ldiv.textContent = l;
              tb.appendChild(ldiv);
            });
            el.appendChild(tb);
          }
        } else if (isAudit) {
          el.classList.add('ni-step-audit');
          var h = document.createElement('div');
          h.className = 'ni-code-step-h';
          h.innerHTML = '<span class="ni-code-glyph">◇</span> <strong>' + st.label + '</strong>';
          el.appendChild(h);
          if (st.detail) {
            var ab = document.createElement('div');
            ab.className = 'ni-code-step-audit-body';
            st.detail.split('\n').forEach(function (l) {
              if (!l.trim()) return;
              var adiv = document.createElement('div');
              adiv.className = 'ni-audit-line';
              adiv.textContent = l;
              ab.appendChild(adiv);
            });
            el.appendChild(ab);
          }
        } else {
          var h = document.createElement('div');
          h.className = 'ni-code-step-h';
          h.innerHTML = '<span class="ni-code-glyph">' + (st.ok ? '✓' : '△') + '</span> ' + st.label;
          el.appendChild(h);
          if (st.detail) {
            var db = document.createElement('div');
            db.className = 'ni-code-step-detail';
            db.textContent = st.detail;
            el.appendChild(db);
          }
        }
        sl.appendChild(el);
      });
      box.appendChild(sl);

      if (isBusy) {
        var busy = document.createElement('div');
        busy.className = 'ni-code-lauf-busy';
        var busyText = document.createElement('span');
        busyText.className = 'ni-code-lauf-statustext';
        var mName = (modell && (modell.display_name || modell.canonical_model_id)) ? modelText(modell) : 'Noki';
        busyText.textContent = (schritte && schritte.length) ? (mName + ' arbeitet …') : 'Analysiert Anforderung und initialisiert Projekt …';
        busy.appendChild(busyText);
        box.appendChild(busy);
      }
    }

    function codeKarteZeigen() {
      if (!state.busy || !answer || !codeLauf) return;
      var box = answer.querySelector('.ni-code-lauf');
      if (!box) {
        answer.replaceChildren();
        box = document.createElement('div');
        box.className = 'ni-code-lauf';
        answer.appendChild(box);
      }
      renderCodeProtokoll(box, codeLauf.schritte, liveModell, codeLauf.projekt, codeLauf.wechsel, true);
      liveModellZeigen();
      if (autoFollow && body) {
        body.scrollTop = body.scrollHeight;
      }
    }

    // Clean textual completion summary (no action buttons in chat, as requested: P6-P9)
    function codeErgebnisZeigen(parent, cp) {
      if (!cp || !cp.pfad) return;
      var box = document.createElement('div');
      box.className = 'ni-code-abschluss-box';
      var hinweis = document.createElement('div');
      hinweis.className = 'ni-code-abschluss-hinweis';
      hinweis.innerHTML = '<span class="ni-code-abschluss-icon">✓</span> <span>Projekt gespeichert in <strong>Projekte</strong> (Projekt öffnen & steuern über den Reiter „Projekte“)</span>';
      box.appendChild(hinweis);

      var meta = document.createElement('div');
      meta.className = 'ni-code-abschluss-meta';
      var d = cp.dateien || [];
      var parts = ['Projekt: ' + (cp.name || 'Code')];
      if (d.length) parts.push(d.length + ' Dateien (' + d.slice(0, 4).map(dateiName).join(', ') + (d.length > 4 ? ' …' : '') + ')');
      meta.textContent = parts.join(' · ');
      box.appendChild(meta);
      parent.appendChild(box);
    }
    // CODE SPACE: Ask is the conversation entry, the work happens (and is
    // shown) in Code. Switching views never cancels the running build.
    function codeSpaceOeffnen(pfad) {
      if (!codeView) return;
      if (assistantMode !== 'code') switchAssistantMode('code');
      // After the view's own start handler has set the live build state.
      setTimeout(function () { codeView.projekt(pfad || null); }, 0);
    }
    // The model that is producing the running answer, reported by the native
    // side the moment an attempt starts (fallbacks update it).
    var liveModell = null;
    function liveModellZeigen() {
      if (!state.busy || !answer || !liveModell) return;
      var text = provenanceText(liveModell);
      if (!text) return;
      var el = answer.querySelector('.ni-model-live');
      if (!el) { el = document.createElement('div'); el.className = 'ni-model-provenance ni-model-live'; }
      el.textContent = text;
      answer.appendChild(el);   // always last, below steps / streamed text
    }
    function appendProvenance(parent, provenance) {
      var text = provenanceText(provenance);
      if (!text) return;
      var el = document.createElement('div');
      el.className = 'ni-model-provenance';
      el.textContent = text;
      parent.appendChild(el);
    }
    function showTurnMeta(t) {
      meta.replaceChildren();
      if (t.fehler) return;
      var route = metaText(t);
      if (route) meta.appendChild(document.createTextNode(route));
    }
    // ---- Conversation (session context) ----------------------------------------------------------
    var CHATS_STORAGE_KEY = 'noki.warp.chats.v1';
    function chatCreated(c) { return Number(c && (c.created_at || c.zeit)) || Date.now(); }
    function chatUpdated(c) { return Number(c && (c.updated_at || c.updated || c.created_at || c.zeit)) || Date.now(); }
    function sortiereChats() { chats.sort(function (a, b) { return chatUpdated(b) - chatUpdated(a); }); }
    function ladeChats() {
      try {
        var raw = localStorage.getItem(CHATS_STORAGE_KEY);
        if (raw) {
          var parsed = JSON.parse(raw);
          if (Array.isArray(parsed) && parsed.length) {
            chats = parsed.map(function (c) {
              var created = Number(c.created_at || c.zeit) || Date.now();
              var updated = Number(c.updated_at || c.updated) || created;
              return {
                id: c.id,
                titel: c.title || c.titel || 'Neuer Chat',
                turns: Array.isArray(c.messages) ? c.messages : (Array.isArray(c.turns) ? c.turns : []),
                created_at: created,
                updated_at: updated,
                preview: c.preview || ''
              };
            }).filter(function (c) { return !!c.id; });
            sortiereChats();
            chat = chats[0];
            return;
          }
        }
      } catch (x) {}
      chats = [];
      chat = null;
    }
    function speichereChats() {
      try {
        sortiereChats();
        var saveList = chats.slice(0, 60).map(function (c) {
          return {
            id: c.id,
            title: c.titel,
            messages: (c.turns || []).slice(-50),
            created_at: chatCreated(c),
            updated_at: chatUpdated(c),
            preview: c.preview || (c.turns && c.turns.length ? (c.turns[c.turns.length - 1].text || '').slice(0, 100) : ''),
          };
        });
        localStorage.setItem(CHATS_STORAGE_KEY, JSON.stringify(saveList));
      } catch (x) {}
    }
    function titelAus(q) { var t = String(q).replace(/\s+/g, ' ').trim().replace(/[?!.]+$/, ''); return t.length > 42 ? t.slice(0, 40).trim() + ' …' : t; }
    function neuerChat(q) {
      var id = 'chat_' + Date.now().toString(36) + '_' + Math.random().toString(36).slice(2, 6);
      var now = Date.now();
      chat = { id: id, titel: titelAus(q), turns: [], created_at: now, updated_at: now, preview: '' };
      chats.unshift(chat);
      if (chats.length > 60) chats.length = 60;
      speichereChats();
    }
    function starteNeuenChat() {
      if (state.busy) call('intelligence_cancel').catch(function () {});
      generation++;
      state.busy = false;
      pendingTool = null;
      schritte = [];
      unread = false;
      var id = 'chat_' + Date.now().toString(36) + '_' + Math.random().toString(36).slice(2, 6);
      var now = Date.now();
      chat = { id: id, titel: 'Neuer Chat', turns: [], created_at: now, updated_at: now, preview: '' };
      chats.unshift(chat);
      if (chats.length > 60) chats.length = 60;
      speichereChats();
      zeigeChat();
      toggleVerlauf(false);
      requestAnimationFrame(function () { zurNeuesten(true); });
      if (meta) meta.textContent = '';
      if (send) send.disabled = false;
      if (input) {
        input.disabled = false;
        input.value = '';
        feldHoehe();
        setTimeout(function () { input.focus(); }, 40);
      }
      setStatus('bereit');
      host.thinking(false);
      indikatorAus();
    }
    function formatZeit(ts) {
      if (!ts) return '';
      var d = new Date(ts);
      var now = new Date();
      var diffDays = Math.floor((now - d) / 86400000);
      var timeStr = d.toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' });
      if (diffDays === 0 && d.getDate() === now.getDate()) return 'Heute, ' + timeStr;
      if (diffDays <= 1) return 'Gestern, ' + timeStr;
      return d.toLocaleDateString('de-DE', { day: '2-digit', month: '2-digit', year: '2-digit' }) + ' ' + timeStr;
    }
    // Model context: recent turns incl. short source/tool notes; the native side compacts older turns itself.
    function modellVerlauf() {
      var h = [];
      (chat ? chat.turns : []).slice(-6).forEach(function (t) {
        var a = t.text + (t.sources && t.sources.length ? '\nQuellen: ' + t.sources.map(function (s) { return s.title || s.url; }).slice(0, 4).join('; ') : '') + (t.tool ? '\nAktion vorgeschlagen: ' + t.tool.label : '');
        h.push({ role: 'user', text: t.q }, { role: 'assistant', text: a });
      });
      return h;
    }
    function turnAlt(t) {
      var box = document.createElement('div'), q = document.createElement('div'), a = renderText(t.text);
      box.className = 'ni-turn'; q.className = 'ni-turn-q'; q.textContent = t.q; a.className = 'ni-turn-a ni-text ni-markdown';
      box.appendChild(q);
      if (t.code_actions && t.code_actions.length) {
        var prot = document.createElement('div');
        prot.className = 'ni-code-lauf ni-code-lauf-fertig';
        renderCodeProtokoll(prot, t.code_actions, t.runtime_model, t.code_projekt ? t.code_projekt.name : '', null, false);
        box.appendChild(prot);
      }
      box.appendChild(a);
      if (!t.fehler) codeErgebnisZeigen(box, t.code_projekt);
      appendProvenance(box, t.runtime_model);
      if (t.sources && t.sources.length) { var s = document.createElement('div'); s.className = 'ni-turn-s'; s.textContent = 'Quellen · ' + t.sources.length; box.appendChild(s); }
      return box;
    }
    function chatId() { return (chat && chat.id) ? String(chat.id) : ''; }
    var anhaengeZeigen = function () {};
    function zeigeFrueher(n) { frueher.replaceChildren(); (chat ? chat.turns.slice(0, n) : []).forEach(function (t) { frueher.appendChild(turnAlt(t)); }); }
    function zeigeChat() {
      tools.replaceChildren(); pendingTool = null;
      anhaengeZeigen();
      if (!chat || !chat.turns.length) { frueher.replaceChildren(); question.textContent = ''; answer.replaceChildren(); sources.replaceChildren(); if (meta) meta.textContent = ''; return; }
      var last = chat.turns[chat.turns.length - 1];
      zeigeFrueher(chat.turns.length - 1);
      question.textContent = last.q; showBlocks([{ type: last.fehler ? 'status' : 'text', text: last.text }]);
      if (!last.fehler && !(last.code_actions && last.code_actions.length)) vorgehenZeigen(answer, last.vorgehen);
      if (last.code_actions && last.code_actions.length) {
        var prot = document.createElement('div');
        prot.className = 'ni-code-lauf ni-code-lauf-fertig';
        renderCodeProtokoll(prot, last.code_actions, last.runtime_model, last.code_projekt ? last.code_projekt.name : '', null, false);
        answer.insertBefore(prot, answer.firstChild);
      }
      // Who answered belongs to THIS answer, right under it - not into the
      // footer, where it only moved to the right place with the next question.
      if (!last.fehler) { codeErgebnisZeigen(answer, last.code_projekt); appendProvenance(answer, last.runtime_model); }
      showSources(last.sources);
      showTurnMeta(last);
    }
    var autoFollow = true;
    var streamingText = '';
    var streamEl = null;
    var streamRenderTimer = null;
    function renderStreaming(final) {
      if (streamRenderTimer) { clearTimeout(streamRenderTimer); streamRenderTimer = null; }
      if (!streamEl) return;
      streamEl.replaceChildren(renderText(streamingText));
      if (autoFollow && body) body.scrollTop = body.scrollHeight;
    }
    function scheduleStreamingRender() {
      if (streamRenderTimer) return;
      streamRenderTimer = setTimeout(function () { streamRenderTimer = null; renderStreaming(false); }, 40);
    }
    // One queued message at most: a second Enter replaces it rather than
    // stacking work the user cannot see.
    var wartendeNachricht = null;

    // An exception thrown mid-render leaves a half-cleared DOM and is invisible
    // in a webview. These two handlers make that case nameable.
    try {
      window.addEventListener('error', function (e) {
        workDiag('js-exception', { wo: String((e && e.filename) || '').slice(-40), zeile: (e && e.lineno) || 0, art: String((e && e.message) || '').slice(0, 120) });
        workInvariante('nach-exception');
      });
      window.addEventListener('unhandledrejection', function (e) {
        workDiag('promise-rejected', { art: String((e && e.reason && e.reason.message) || (e && e.reason) || '').slice(0, 120) });
        workInvariante('nach-rejection');
      });
    } catch (e) {}

    // ---- Blank-screen diagnostics -------------------------------------------------
    // Purpose: when Work goes empty, the log must say WHICH of these it was -
    // view hidden, renderer emptied, conversation switched, or an exception.
    // No message text, no file contents, no secrets - only state shape.
    function workState() {
      var cs = panel ? getComputedStyle(panel) : null;
      return {
        conv: (chat && chat.id) ? String(chat.id) : null,
        gen: generation,
        mode: assistantMode,
        opened: !!state.opened,
        busy: !!state.busy,
        queued: !!wartendeNachricht,
        panel_display: cs ? cs.display : null,
        panel_vis: cs ? cs.visibility : null,
        panel_inline: panel ? (panel.style.display || '') : null,
        body_kinder: body ? body.childElementCount : -1,
        frueher_kinder: frueher ? frueher.childElementCount : -1,
        antwort_kinder: answer ? answer.childElementCount : -1,
        frage_len: question ? (question.textContent || '').length : -1,
        turns: chat ? chat.turns.length : -1,
        verlauf: !!verlaufOffen,
        mic: micZustand,
        recording: !!(panel && panel.classList.contains('ni-recording'))
      };
    }
    function workDiag(ereignis, extra) {
      try {
        var d = workState();
        if (extra) { for (var k in extra) { if (Object.prototype.hasOwnProperty.call(extra, k)) d[k] = extra[k]; } }
        call('noki_log', { msg: '[WORK] ' + ereignis + ' ' + JSON.stringify(d) }).catch(function () {});
      } catch (e) {}
    }
    // The invariant that "blank" violates: an active Work chat with turns must
    // have something rendered. Report it, never silently repair it - a cosmetic
    // auto-fix would hide the very state we are trying to catch.
    function workInvariante(wo) {
      if (assistantMode !== 'work' || !state.opened) return;
      if (!chat || !chat.turns.length) return;
      var leer = (!question || !(question.textContent || '').length)
        && (!answer || answer.childElementCount === 0)
        && (!frueher || frueher.childElementCount === 0);
      var versteckt = panel && getComputedStyle(panel).display === 'none';
      if (leer || versteckt) {
        workDiag('WORK_RENDER_INVARIANT_BROKEN', { wo: wo, leer: leer, versteckt: !!versteckt });
      }
    }

    function distUnten() {
      if (!body) return 0;
      return body.scrollHeight - body.scrollTop - body.clientHeight;
    }
    // The chat area is its own scroll container. Auto-scroll only when the user is already near the
    // bottom; while reading further up the position is kept and a quiet "Neue Antwort" hint appears.
    function nahUnten() { return !body || distUnten() < 72; }
    function neuHinweis(an) { if (neuBtn) neuBtn.hidden = !an; }
    function zurNeuesten(erzwingen) {
      if (!body) return;
      if (!erzwingen && !question.textContent) return;
      if (erzwingen) autoFollow = true;
      if (!autoFollow && !nahUnten()) { neuHinweis(true); return; }
      body.scrollTop = body.scrollHeight;
      neuHinweis(false);
      requestAnimationFrame(function () {
        if (!body) return;
        if (erzwingen || autoFollow) {
          body.scrollTop = body.scrollHeight;
        }
      });
      setTimeout(function () {
        if (!body) return;
        if (erzwingen || autoFollow) {
          body.scrollTop = body.scrollHeight;
        }
      }, 50);
    }
    // ---- History drawer: full conversation library ------------------------------------------------
    function renderVerlauf() {
      if (!drawer) return;
      drawer.replaceChildren();
      var kopf = document.createElement('div');
      kopf.className = 'ni-drawer-kopf';
      var tWrap = document.createElement('div');
      tWrap.className = 'ni-drawer-titel-wrap';
      var t = document.createElement('strong');
      t.textContent = 'Verlauf';
      var hint = document.createElement('span');
      sortiereChats();
      var count = chats.length;
      hint.textContent = count === 1 ? '1 gespeicherter Chat' : count + ' gespeicherte Chats';
      tWrap.appendChild(t); tWrap.appendChild(hint);

      var kopfActs = document.createElement('div');
      kopfActs.className = 'ni-drawer-actions';
      var newBtn = document.createElement('button');
      newBtn.type = 'button';
      newBtn.className = 'ni-drawer-new-btn';
      newBtn.textContent = '+ Neuer Chat';
      newBtn.onclick = function () { starteNeuenChat(); };   // a NEW conversation; the old ones stay
      var closeBtn = document.createElement('button');
      closeBtn.type = 'button';
      closeBtn.className = 'ni-drawer-close-btn';
      closeBtn.textContent = '×';
      closeBtn.title = 'Bibliothek schließen';
      closeBtn.onclick = function () { toggleVerlauf(false); };
      kopfActs.appendChild(newBtn); kopfActs.appendChild(closeBtn);
      kopf.appendChild(tWrap); kopf.appendChild(kopfActs);
      drawer.appendChild(kopf);

      var liste = chats;
      if (!liste.length) {
        var leer = document.createElement('div');
        leer.className = 'ni-drawer-empty';
        leer.innerHTML = '<p>Noch keine vergangenen Chats gespeichert.</p><span>Starte einen neuen Chat oder stelle Noki eine Frage.</span>';
        drawer.appendChild(leer);
        return;
      }
      var ul = document.createElement('div');
      ul.className = 'ni-bib-liste';
      // Grouped like a chat app: Heute / Gestern / Älter (newest first).
      var heute0 = new Date(); heute0.setHours(0, 0, 0, 0);
      var gruppe = function (ts) { var t = +ts || 0; return t >= heute0.getTime() ? 'Heute' : t >= heute0.getTime() - 86400000 ? 'Gestern' : 'Älter'; };
      var letzteGruppe = '';
      liste.forEach(function (c) {
        var g = gruppe(chatUpdated(c));
        if (g !== letzteGruppe) { letzteGruppe = g; var gh = document.createElement('div'); gh.className = 'ni-bib-gruppe'; gh.textContent = g; ul.appendChild(gh); }
        var item = document.createElement('div');
        item.className = 'ni-bib-item ni-bib-zeile' + (chat && chat.id === c.id ? ' aktiv' : '');
        item.dataset.chat = c.id;
        var title = document.createElement('span'); title.className = 'ni-bib-title'; title.textContent = c.titel || 'Unbenannter Chat';
        var zeit = document.createElement('span'); zeit.className = 'ni-bib-time'; zeit.textContent = formatZeit(chatUpdated(c)).replace(/^(Heute|Gestern), /, '');
        var delBtn = document.createElement('button'); delBtn.type = 'button'; delBtn.className = 'ni-bib-x'; delBtn.title = 'Chat löschen'; delBtn.setAttribute('aria-label', 'Chat löschen');
        delBtn.onclick = function (e) {
          e.stopPropagation();
          if (item.nextSibling && item.nextSibling.classList && item.nextSibling.classList.contains('ni-bib-confirm')) return;
          // Real confirmation, inline (no browser dialog).
          var cf = document.createElement('div'); cf.className = 'ni-bib-confirm';
          var tx = document.createElement('span'); tx.textContent = 'Chat wirklich löschen?';
          var ja = document.createElement('button'); ja.type = 'button'; ja.className = 'ni-bib-ja'; ja.textContent = 'Löschen';
          var nein = document.createElement('button'); nein.type = 'button'; nein.textContent = 'Abbrechen';
          nein.onclick = function (ev) { ev.stopPropagation(); cf.remove(); };
          ja.onclick = function (ev) {
            ev.stopPropagation();
            chats = chats.filter(function (x) { return x.id !== c.id; });
            speichereChats();
            if (chat && chat.id === c.id) { chat = null; zeigeChat(); }
            renderVerlauf();
          };
          cf.appendChild(tx); cf.appendChild(ja); cf.appendChild(nein);
          item.parentNode.insertBefore(cf, item.nextSibling);
        };
        item.appendChild(title); item.appendChild(zeit); item.appendChild(delBtn);
        item.onclick = function () {
          chat = c;
          zeigeChat();
          renderVerlauf();     // highlight follows; the library stays open like a sidebar
          zurNeuesten(true);
        };
        ul.appendChild(item);
      });
      drawer.appendChild(ul);

      var footBar = document.createElement('div'); footBar.className = 'ni-drawer-foot';
      var delAll = document.createElement('button'); delAll.type = 'button'; delAll.className = 'ni-verlauf-leeren';
      delAll.textContent = verlaufLeerT && Date.now() - verlaufLeerT < 3000 ? 'Wirklich alle Chats löschen?' : 'Alle Chats löschen';
      delAll.onclick = function () {
        if (!verlaufLeerT || Date.now() - verlaufLeerT >= 3000) {
          verlaufLeerT = Date.now();
          delAll.textContent = 'Wirklich alle Chats löschen?';
          setTimeout(function () { if (verlaufLeerT && Date.now() - verlaufLeerT >= 3000) { verlaufLeerT = 0; delAll.textContent = 'Alle Chats löschen'; } }, 3200);
          return;
        }
        verlaufLeerT = 0; chats = []; chat = null;
        speichereChats(); zeigeChat(); renderVerlauf();
      };
      footBar.appendChild(delAll);
      drawer.appendChild(footBar);
    }
    function toggleVerlauf(an) {
      if (!drawer) return;
      verlaufOffen = typeof an === 'boolean' ? an : !verlaufOffen; verlaufLeerT = 0; drawer.hidden = !verlaufOffen;
      drawer.style.top = '44px';
      var vb = panel.querySelector('.ni-verlauf-btn');
      if (vb) vb.classList.toggle('aktiv', verlaufOffen);
      if (verlaufOffen) renderVerlauf();
    }
    var MODI = [['fast', 'Schnell'], ['normal', 'Normal'], ['intensive', 'Intensiv']];
    function modusName(m) { var x = MODI.filter(function (o) { return o[0] === m; })[0]; return x ? x[1] : 'Normal'; }
    function renderModus() {
      if (!modusEl) return;
      modusEl.innerHTML = MODI.map(function (o) { var an = settings.mode === o[0]; return '<button type="button" role="radio" aria-checked="' + an + '" class="' + (an ? 'aktiv' : '') + '" data-modus="' + o[0] + '">' + o[1] + '</button>'; }).join('');
      if (modusCurrent) modusCurrent.querySelector('span').textContent = modusName(settings.mode);
      modellAnzeige();
    }
    function modusZu() {
      if (!modusEl || !modusCurrent) return;
      modusEl.hidden = true; modusCurrent.setAttribute('aria-expanded', 'false');
    }
    function modusUmschalten() {
      if (!modusEl || !modusCurrent) return;
      var auf = modusEl.hidden; modusEl.hidden = !auf; modusCurrent.setAttribute('aria-expanded', String(auf));
    }
    // One state for the Ask header and Settings → Intelligence (persisted natively, never cancels an answer).
    function setModus(m) {
      if (!MODI.some(function (o) { return o[0] === m; })) return;
      settings.mode = m; modusZu(); renderModus(); refresh();
      call('intelligence_mode', { mode: m }).then(function (r) { settings.mode = r; renderModus(); refreshStatus(); refresh(); }).catch(function () {});
    }
    function switchAssistantMode(next) {
      if (next === 'chat') next = 'work';
      if (next === assistantMode || !['work','code'].includes(next)) return;
      // Leaving Work also leaves its audio mode. Release the recorder before the
      // Code view is shown so a hidden VoiceDraft can never retain the microphone
      // or leave Work's composer locked when the user comes back.
      if (next === 'code' && micZustand !== 'idle') micAbbrechen();
      if (next === 'code' && panel) { var br2 = panel.querySelector('.ni-browser'); if (br2) br2.hidden = true; }
      var request = ++modeRequest;
      assistantMode = next;
      panel.dataset.assistantMode = next;
      panel.querySelectorAll('.ni-assistant-modes button').forEach(function (b) {
        var active = b.dataset.assistant === next;
        b.classList.toggle('aktiv', active);
        b.setAttribute('aria-selected', String(active));
      });
      toggleVerlauf(false);
      modellAnzeige();
      // The view changes immediately. Model ownership is synchronized below,
      // but a slow/canceling Work request must never hold the navigation UI.
      if (next === 'work') {
        if (codeView) codeView.leave();
        zeigeChat();
        input.disabled = true;
        send.disabled = true;
        setStatus(state.busy ? 'denkt' : 'laedt');
        requestAnimationFrame(function () { zurNeuesten(true); });
      } else if (codeView) {
        codeView.enter(codeTerminalStatus);
      }
      var command = next === 'code' ? 'intelligence_code_overview' : 'intelligence_assistant_mode';
      var args = next === 'code' ? {} : { mode: 'work' };
      // Serialize rapid Work/Code toggles so the native model state ends in
      // the same mode as the last click. The buttons themselves stay usable.
      modeSwitch = modeSwitch.catch(function () {}).then(function () { return call(command, args); });
      modeSwitch.then(function (result) {
        if (next === 'code') codeTerminalStatus = result || codeTerminalStatus;
        if (request !== modeRequest) return;
        renderCodeTerminal(); refreshStatus(); modellAnzeige(); setStatus(state.busy ? 'denkt' : 'bereit');
        if (next === 'work') {
          input.disabled = state.busy;
          send.disabled = state.busy;
          if (!state.busy) input.focus();
        }
      }).catch(function (e) {
        if (request !== modeRequest) return;
        meta.textContent = String(e);
        input.disabled = state.busy;
        send.disabled = state.busy;
        setStatus(state.busy ? 'denkt' : 'bereit');
      });
    }
    function renderCodeTerminal() { if (codeView) codeView.status(codeTerminalStatus); }
    // Local speech input (on-device only). Audio never reaches this page; only text arrives.
    var MIC_FEHLER = { unavailable: 'Lokale Spracherkennung ist hier nicht verfügbar.', denied: 'Spracherkennung ist nicht erlaubt (Systemeinstellungen → Datenschutz & Sicherheit).',
      mic_denied: 'Mikrofonzugriff ist nicht erlaubt (Systemeinstellungen → Datenschutz & Sicherheit).', mic: 'Das Mikrofon konnte nicht gestartet werden.' };
    function setMic(k) {
      micZustand = k; rec.active = k !== 'idle';
      if (!mic) return;
      mic.dataset.k = k; mic.setAttribute('aria-pressed', String(k === 'listening'));
      mic.title = k === 'listening' ? 'Aufnahme läuft' : k === 'processing' ? 'Wird lokal umgewandelt …' : 'Spracheingabe (lokal)';
      if (voiceLive) { voiceLive.hidden = k === 'idle'; voiceLive.dataset.k = k; }
      if (voiceSend) voiceSend.disabled = k === 'processing' || k === 'idle' || !micGesamt().trim();
      if (voiceToggle) {
        voiceToggle.disabled = k === 'processing' || k === 'idle';
        voiceToggle.dataset.k = k;
        voiceToggle.textContent = k === 'paused' ? 'Weiter aufnehmen' : k === 'processing' ? 'Wird gestoppt …' : 'Stop';
      }
      if (k === 'idle') setMeter(0);
      micAnzeigen();
    }
    function setMeter(level) {
      if (!voiceMeter || !mic) return;
      var n = Math.max(0, Math.min(1, Number(level) || 0));
      var formen = [0.62, 0.9, 1, 0.78, 0.55];
      Array.prototype.forEach.call(voiceMeter.children, function (b, i) { b.style.transform = 'scaleY(' + (0.18 + n * formen[i]).toFixed(2) + ')'; });
      var klein = mic.querySelector('.ni-mic-eq');
      if (klein) Array.prototype.forEach.call(klein.children, function (b, i) { b.style.transform = 'scaleY(' + (0.22 + n * [0.7, 1, 0.6][i]).toFixed(2) + ')'; });
    }
    function worte(t) { return String(t || '').trim().split(/\s+/).filter(Boolean); }
    var canonW = function (w) { return w.toLowerCase().replace(/[^\p{L}\p{N}]/gu, ''); };
    // Deduplication happens ONLY at the segment boundary: an exact word overlap of the
    // committed tail and the new head is dropped. Nothing already committed is shortened.
    function anhaengen(basisWorte, zusatz) {
      var b = worte(zusatz); if (!b.length) return basisWorte;
      if (!basisWorte.length) return b.slice();
      // Longest boundary overlap; a single shared word is never enough evidence, so a real
      // user repetition ("sehr sehr") survives.
      var overlap = 0;
      for (var n = Math.min(40, basisWorte.length, b.length); n >= 2; n--)
        if (basisWorte.slice(-n).map(canonW).join('\u0000') === b.slice(0, n).map(canonW).join('\u0000')) { overlap = n; break; }
      return basisWorte.concat(b.slice(overlap));
    }
    // Merge the monotonic snapshot with the canonical timeline text. The timeline is the
    // authority; the snapshot only fills in what a momentarily shorter timeline dropped.
    // Prefix checks first: anhaengen() only looks 40 words back and would otherwise
    // concatenate two copies of a long section instead of recognising the overlap.
    function verschmelzen(snapshot, canonical) {
      if (!snapshot) return canonical || '';
      if (!canonical) return snapshot;
      if (canonical.indexOf(snapshot) === 0) return canonical;   // timeline grew - take it
      if (snapshot.indexOf(canonical) === 0) return snapshot;    // timeline shrank - hold
      return anhaengen(worte(snapshot), canonical).join(' ');    // genuinely new tail
    }
    function normalizeVoiceTimeline(segments) {
      var ordered = [];
      (segments || []).forEach(function (s) {
        if (!s || !String(s.text || '').trim() || !Number.isFinite(s.audio_start) || !Number.isFinite(s.audio_end)) return;
        var i = ordered.findIndex(function (old) {
          var overlap = Math.min(old.audio_end, s.audio_end) - Math.max(old.audio_start, s.audio_start);
          return old.id === s.id || (overlap > 0 && overlap >= Math.min(old.audio_end - old.audio_start, s.audio_end - s.audio_start) * 0.6);
        });
        if (i < 0) ordered.push(s);
        else if ((s.final && !ordered[i].final) || (s.final === ordered[i].final && s.text.length > ordered[i].text.length)) ordered[i] = s;
      });
      return ordered.sort(function (a, b) { return a.audio_start - b.audio_start || a.audio_end - b.audio_end; });
    }
    function composeVoiceText(segments, committedOnly) {
      return (segments || []).filter(function (s) { return !committedOnly || s.final; })
        .slice().sort(function (a, b) { return a.audio_start - b.audio_start || a.audio_end - b.audio_end; })
        .map(function (s) { return String(s.text || '').trim(); }).filter(Boolean).join(' ');
    }
    function micCommittedText() { return rec.timeline ? composeVoiceText(rec.timeline, true) : rec.committed.join(' ').replace(/\s+/g, ' ').trim(); }
    // ONE canonical transcript, always from the local ledger (it is monotonic and never shrinks).
    // The helper's composed view is only used while the ledger is still empty.
    function micGesamt() {
      // §12: send and display come from the SAME canonical source. The raw timeline alone
      // can momentarily be shorter than what the user already saw, so it is merged with the
      // monotonic snapshot — never a second, independent text source.
      if (rec.timeline) return verschmelzen(rec.sichtSnapshot, composeVoiceText(rec.timeline));
      var lokal = anhaengen(worte(micCommittedText()), rec.interim).join(' ');
      return lokal || rec.display || '';
    }
    // Monotonic merge: a snapshot from the helper may only EXTEND what we already hold.
    // If it does not continue the committed text (helper restart), it is appended as a new segment.
    function commitSnapshot(text) {
      var neuW = worte(text); if (!neuW.length) return;
      var altW = worte(micCommittedText());
      if (altW.length && neuW.length >= altW.length) {
        var prefix = true;
        for (var i = 0; i < altW.length; i++) if (canonW(altW[i]) !== canonW(neuW[i])) { prefix = false; break; }
        if (prefix) { rec.committed = [neuW.join(' ')]; return; }
      }
      rec.committed = [anhaengen(altW, text).join(' ')];
    }
    // §3/§4/§5: the status line and the transcript live side by side. Already recognised
    // speech stays on screen; only the unconfirmed tail changes.
    function sichtReset() { rec.sichtSnapshot = ''; rec.letzterCanon = ''; rec.blankFrames = 0; rec.maxWorte = 0; rec.wortRegress = 0; rec.domCommitted = ''; }
    // §3: der sichtbare committed-Snapshot ist ZUSTAND, nicht Rendering. Er wächst nur und
    // überlebt jede Sprechpause und jeden Recognizer-Neustart.
    function sichtAktualisieren() {
      // §9: ONE monotonic rule for both paths. The timeline path used to assign the ledger
      // directly, so a momentarily shorter native timeline could shrink the snapshot — and
      // with it every failsafe built on top of it.
      // GEMESSEN am echten Apple-Stream (voice-live.jsonl): nach einer Sprechpause meldet
      // der On-Device-Recognizer KEIN isFinal und erhoeht die Generation NICHT (gen bleibt 1,
      // committed='', segments=0). Er verwirft den vorherigen Satz einfach aus seinem eigenen
      // Ergebnis: timeline 2 -> 0 -> 5. Damit bleibt micCommittedText() dauerhaft leer und
      // der Snapshot haette nie etwas zu halten -> B ersetzte A im DOM (Event 33).
      // Die LEERE timeline ist die echte Satzgrenze: was zuletzt auf dem Schirm stand, ist
      // fertig und wird festgeschrieben. Innerhalb eines Satzes wird die timeline nie leer,
      // deshalb ersetzen Korrekturen desselben Satzes weiterhin sauber (keine Dopplung).
      var ledger = micCommittedText();
      if (ledger) rec.sichtSnapshot = verschmelzen(rec.sichtSnapshot, ledger);
    }
    // §2/§3/§4: THE only writer of the visible transcript node. Every write is logged with
    // its source, and during recording a write may never shorten what the user already sees
    // while the canonical session text is not shorter itself.
    function schreibeSichtbar(neu, quelle) {
      if (!voiceText) return;
      var alt = voiceText.textContent || '';
      neu = String(neu == null ? '' : neu);
      if (neu === alt) return;
      var canon = rec.timeline ? verschmelzen(rec.sichtSnapshot, composeVoiceText(rec.timeline)) : rec.sichtSnapshot;
      vlog('ui_write_attempt', { quelle: quelle, before_dom: alt.length, proposed_dom: neu.length,
        canonical: canon.length, snapshot: rec.sichtSnapshot.length,
        before_head: alt.slice(0, 24), proposed_head: neu.slice(0, 24), canonical_head: canon.slice(0, 24) });
      if (micZustand !== 'idle' && alt && neu.length < alt.length && canon.length >= alt.length) {
        rec.blockiert = (rec.blockiert || 0) + 1;
        vlog('voice_ui_write_blocked', { quelle: quelle, alt_chars: alt.length, neu_chars: neu.length,
          canon_chars: canon.length, alt_kopf: alt.slice(0, 30), neu_kopf: neu.slice(0, 30) });
        return;
      }
      vlog('voice_ui_write', { quelle: quelle, alt_chars: alt.length, neu_chars: neu.length, canon_chars: canon.length });
      voiceText.textContent = neu;
    }
    function micAnzeigen() {
      sichtAktualisieren();
      if (rec.timeline) rec.interim = composeVoiceText(rec.timeline.filter(function (s) { return !s.final; }));
      var wortZahl = anhaengen(worte(rec.sichtSnapshot), rec.interim).join(' ').trim().split(/\s+/).filter(Boolean).length;
      if (wortZahl < rec.maxWorte) { rec.wortRegress++; vlog('word_count_regression', { von: rec.maxWorte, nach: wortZahl }); }
      else rec.maxWorte = wortZahl;
      if (!voiceText) return;
      var c = rec.sichtSnapshot;
      var voll = rec.timeline ? micGesamt() : anhaengen(worte(c), rec.interim).join(' ');
      var tail = rec.timeline ? '' : (voll.length > c.length && voll.indexOf(c) === 0 ? voll.slice(c.length) : (c ? '' : voll));
      if (voiceBasis) voiceBasis.textContent = rec.basis || '';
      schreibeSichtbar(rec.timeline ? voll : c, 'micAnzeigen');
      if (voiceInterim) voiceInterim.textContent = tail;
      // §6: waehrend der Aufnahme darf nie ein leerer Frame entstehen, sobald einmal Text da war.
      if (micZustand !== 'idle' && rec.sichtSnapshot && !(voiceText.textContent || '').length) {
        rec.blankFrames++; vlog('voice_blank_frame', { snapshot: rec.sichtSnapshot.length });
        schreibeSichtbar(rec.sichtSnapshot, 'blank_frame_failsafe');
      }
      var dom = ((voiceBasis && voiceBasis.textContent) || '') + (voiceText.textContent || '') + ((voiceInterim && voiceInterim.textContent) || '');
      // §15 DEV-only: Session-Zustand vs. echter DOM auf einen Blick. Steht dort
      // 'S 42 · DOM 8', ist der Fehler sofort sichtbar — kein Terminal nötig.
      if (voiceWorte) {
        var basis = rec.maxWorte ? 'Aufgenommen: ' + rec.maxWorte + ' Wörter' : '';
        if (basis && DEV_DIAG) {
          var domW = dom.trim().split(/\s+/).filter(Boolean).length;
          basis += ' · CANON ' + (voll || '').trim().split(/\s+/).filter(Boolean).length + ' · VISIBLE ' + domW + ' · G' + (rec.gen < 0 ? 0 : rec.gen)
            + (rec.clipTop ? ' · OBEN ' + rec.clipTop + 'px' : '')
            + (rec.wortRegress || rec.blankFrames || rec.blockiert ? ' · \u26a0' + rec.wortRegress + '/' + rec.blankFrames + '/' + (rec.blockiert || 0) : '');
        }
        voiceWorte.textContent = basis;
      }
      if (voiceStatus) voiceStatus.textContent = micZustand === 'listening'
        ? (c || tail ? 'Noki hört zu' : 'Noki hört zu – sprich einfach los')
        : micZustand === 'processing' ? 'Transkript wird abgeschlossen …'
        : micZustand === 'paused' ? 'Aufnahme pausiert' : 'Spracheingabe';
      if (voiceLeer) voiceLeer.hidden = !!(voll || rec.basis);
      if (voiceSend) voiceSend.disabled = micZustand === 'processing' || micZustand === 'idle' || !voll.trim();

      if (rec.domCommitted && dom.indexOf(rec.domCommitted) !== 0) vlog('dom_regression', { war: rec.domCommitted.length, jetzt: dom.length });
      rec.domText = dom; rec.domCommitted = ((voiceBasis && voiceBasis.textContent) || '') + (voiceText.textContent || '');
      // §5/§6: Verankerung. Solange alles in den Bereich passt, bleibt der Anfang stehen
      // (scrollTop 0) — der User sieht den GESAMTEN bisherigen Text. Erst wenn der
      // Transkript wirklich hoeher ist als der Platz, wird dem Ende gefolgt, und auch
      // das nur, wenn der User nicht selbst hochgescrollt hat. Ein Recognizer-Neustart
      // ist kein UX-Ereignis und loest nie einen Sprung aus.
      var box = voiceText.parentNode;
      if (box) {
        if (!rec.scrollHook) {
          rec.scrollHook = true;
          box.addEventListener('scroll', function () {
            if (rec.autoScroll) { rec.autoScroll = false; return; }   // eigener Schreibvorgang
            rec.userScrolled = box.scrollTop + box.clientHeight < box.scrollHeight - 12;
          });
        }
        var passt = box.scrollHeight <= box.clientHeight + 2;
        if (passt) { rec.userScrolled = false; if (box.scrollTop !== 0) { rec.autoScroll = true; box.scrollTop = 0; } }
        else if (!rec.userScrolled && box.scrollTop + box.clientHeight < box.scrollHeight - 2) {
          rec.autoScroll = true; box.scrollTop = box.scrollHeight;
        }
        rec.clipTop = 0;
        var pr = voiceText.getBoundingClientRect(), br = box.getBoundingClientRect();
        if (!passt) rec.clipTop = Math.max(0, Math.round(br.top - pr.top));
        vlog('viewport', { st: Math.round(box.scrollTop), sh: box.scrollHeight, ch: box.clientHeight,
          off_top_px: Math.max(0, Math.round(br.top - pr.top)), off_bottom_px: Math.max(0, Math.round(pr.bottom - br.bottom)),
          dom_chars: (voiceText.textContent || '').length, passt: passt, user_scrolled: !!rec.userScrolled });
      }
    }
    function composerSperren(an) {
      if (!panel) return;
      panel.classList.toggle('ni-recording', an);
      if (input) { input.disabled = an || !settings.ask; if (an) input.blur(); }
      if (send) send.disabled = an || state.busy;
    }
    function micStart() {
      if (micZustand === 'paused') {
        // Start a new native recorder, but keep the complete visible ledger. The
        // next recognizer generation is merged after this snapshot.
        var bisher = micGesamt();
        rec.sichtSnapshot = bisher;
        rec.committed = bisher ? [bisher] : [];
        rec.timeline = null; rec.interim = ''; rec.display = ''; rec.gen = -1; rec.seq = -1;
        rec.sending = false; rec.stopRequested = false; rec.stopAction = '';
        setMic('processing');
        if (voiceStatus) voiceStatus.textContent = 'Mikrofon wird gestartet …';
        call('intelligence_voice_start', { owner: rec.owner }).catch(function (e) { micFehler(e); });
        return;
      }
      if (micZustand !== 'idle') return;
      var targetChatId = chat ? chat.id : null;
      var ownerKey = 'chat:' + (targetChatId || ('draft_' + Date.now()));
      rec.owner = ownerKey;
      rec.targetChatId = targetChatId;
      rec.targetChat = chat;
      // §7: manually typed text is parked, never mixed into the speech ledger.
      rec.basis = input.value.trim() ? input.value.trim() + ' ' : '';
      rec.sichtbar = 0; sichtReset();
      rec.id++; rec.committed = []; rec.timeline = null; rec.interim = ''; rec.display = ''; rec.gen = -1; rec.seq = -1; rec.sending = false; rec.stopRequested = false; rec.stopAction = ''; rec.startedAt = Date.now(); vlog('recording_session_started');
      composerSperren(true);
      setMic('processing');
      if (voiceStatus) voiceStatus.textContent = 'Mikrofon wird gestartet …';
      call('intelligence_voice_start', { owner: ownerKey }).catch(function (e) { micFehler(e); });
    }
    function micKlick() { if (micZustand === 'listening') micSenden(); else micStart(); }
    // Stop only pauses the VoiceDraft. Its complete transcript stays visible and
    // can be extended by another recording before the user sends it.
    function micStop() {
      if (micZustand !== 'listening') return;
      rec.sending = false; rec.stopRequested = true; rec.stopAction = 'pause'; vlog('stop_pressed');
      rec.sichtbarText = micGesamt();
      setMic('processing');
      schreibeSichtbar(micGesamt() || 'Transkript wird abgeschlossen …', 'micStop');
      call('intelligence_voice_stop', { cancel: false }).catch(function () { micPauseFertig(); });
    }
    function micToggle() { if (micZustand === 'listening') micStop(); else if (micZustand === 'paused') micStart(); }
    // Audio abschicken: stop if necessary, then transfer the whole VoiceDraft
    // exactly once into the normal submit path.
    function micSenden() {
      if (micZustand !== 'listening' && micZustand !== 'paused') return;
      if (!micGesamt().trim()) return;
      rec.sending = true; rec.stopRequested = true; rec.stopAction = 'send'; vlog('send_pressed');
      rec.sichtbarText = micGesamt();
      if (micZustand === 'paused') { micFertig(); return; }
      setMic('processing');
      schreibeSichtbar(micGesamt() || 'Transkript wird abgeschlossen …', 'micStop');
      call('intelligence_voice_stop', { cancel: false }).catch(function () { micFertig(); });
    }
    // Abbrechen: stop, discard audio and transcript; the previous chat stays untouched.
    function micAbbrechen() {
      // Also unlock a stale recording class. This makes Cancel idempotent and
      // repairs the exact state that previously hid the chat composer forever.
      if (micZustand === 'idle') { composerSperren(false); return; }
      rec.sending = false; rec.stopRequested = true; vlog('cancel_pressed'); sichtReset();
      call('intelligence_voice_stop', { cancel: true }).catch(function () {});
      rec.committed = []; rec.timeline = null; rec.interim = ''; rec.display = ''; rec.stopAction = '';
      rec.owner = ''; rec.targetChat = null; rec.targetChatId = null;
      if (input) { input.value = rec.basis.trim(); feldHoehe(); }
      setMic('idle'); composerSperren(false); if (meta) meta.textContent = '';
      if (input) requestAnimationFrame(function () { if (assistantMode === 'work') input.focus(); });
    }
    function micPauseFertig() {
      if (rec.stopAction !== 'pause') return;
      var text = micGesamt();
      rec.sichtSnapshot = text;
      rec.committed = text ? [text] : [];
      rec.timeline = null; rec.interim = ''; rec.display = ''; rec.stopRequested = false; rec.stopAction = '';
      setMic('paused');
      micAnzeigen();
    }
    function micFehler(e) {
      var text = (rec.basis + micGesamt()).trim();
      rec.sending = false; rec.stopRequested = false; rec.stopAction = '';
      rec.committed = []; rec.timeline = null; rec.interim = ''; rec.display = ''; rec.sichtSnapshot = '';
      rec.owner = ''; rec.targetChat = null; rec.targetChatId = null;
      setMic('idle'); composerSperren(false);
      if (input) { input.value = text; feldHoehe(); input.focus(); }
      if (meta) meta.textContent = typeof e === 'string' ? (MIC_FEHLER[e] || e) : String(e || MIC_FEHLER.unavailable);
    }
    // End of a user recording that was confirmed with "Senden".
    // The VoiceDraft ends. ONLY here does its text ever reach the composer, and only once.
    // An unexpected end (helper gone) keeps the draft visible instead of silently transferring it.
    function micFertig() {
      var text = (rec.basis + micGesamt()).trim();
      var senden = rec.sending, gewollt = rec.stopRequested;
      var targetChatId = rec.targetChatId;
      rec.committed = []; rec.timeline = null; rec.interim = ''; rec.display = ''; rec.sichtbar = 0; rec.sichtSnapshot = ''; rec.sending = false; rec.stopAction = '';
      rec.owner = ''; rec.targetChat = null; rec.targetChatId = null;
      setMic('idle'); composerSperren(false);
      if (meta) meta.textContent = gewollt ? '' : 'Aufnahme unerwartet beendet – Text übernommen.';
      vlog('composer_write', { gewollt: gewollt, laenge: text.length, sichtbar: rec.sichtbarText || '', gesendet: text });
      if (!input) return;
      if (!targetChatId || (chat && chat.id === targetChatId)) {
        input.value = text; feldHoehe();
        if (DEV_DIAG && meta) { meta.textContent = 'Voice-Diagnose gespeichert'; }
        if (senden && text && panel) submit(); else input.focus();
      } else {
        var found = chats.find(function (c) { return c.id === targetChatId; });
        if (found) {
          chatWaehlen(found.id);
        }
        input.value = text; feldHoehe();
        if (DEV_DIAG && meta) { meta.textContent = 'Voice-Diagnose gespeichert'; }
        if (senden && text && panel) submit(); else input.focus();
      }
    }
    function feldHoehe() { if (!input) return; input.style.height = 'auto'; input.style.height = Math.min(input.scrollHeight, 132) + 'px'; }
    try {
      var voiceHandler = function (e) {
        var p = (e && e.payload) || {};
        if (p.owner && rec.owner && p.owner !== rec.owner) return;
        if (p.error) { vlog('error', { error: p.error }); micFehler(p.error); return; }
        if (typeof p.level === 'number') setMeter(p.level);
        if (p.state === 'listening') {
          // A fresh helper process starts its generation/sequence counters over.
          if (micZustand === 'idle') { rec.id++; rec.committed = []; rec.timeline = null; rec.interim = ''; rec.display = ''; sichtReset(); rec.sending = false; rec.stopRequested = false; rec.startedAt = Date.now(); }
          rec.gen = -1; rec.seq = -1;
          vlog('recording_session_started');
          setMic('listening'); if (meta) meta.textContent = 'Aufnahme läuft · nur lokal'; return;
        }
        if (p.state === 'processing') { if (micZustand === 'listening') setMic('processing'); return; }
        // The helper process ended. Only a user stop may hand the text on; anything else keeps it
        // in the input without sending (R: never auto-send).
        if (p.state === 'idle') {
          if (micZustand === 'paused' || micZustand === 'idle') return;
          vlog('helper_idle');
          if (rec.stopAction === 'pause') micPauseFertig(); else micFertig();
          return;
        }
        var hatText = Array.isArray(p.timeline) || typeof p.committed === 'string' || typeof p.interim === 'string' || typeof p.partial === 'string';
        if (!hatText && !p.final) return;
        if (micZustand === 'idle') return;                                  // session already over: ignore late results
        // Generation guard: a late result of an older recognizer cycle never overwrites newer text.
        if (typeof p.seq === 'number') { if (p.seq <= rec.seq) return; rec.seq = p.seq; }
        if (typeof p.gen === 'number') {
          if (p.gen < rec.gen) return;
          if (p.gen > rec.gen && rec.gen >= 0) vlog('recognizer_restarted', { von: rec.gen, nach: p.gen });
          if (p.gen !== rec.gen) { rec.gen = p.gen; vlog('recognizer_generation_started'); }
        }
        var vorher = micCommittedText().length;
        if (Array.isArray(p.timeline)) {
          // Satzgrenze, GEMESSEN am echten Apple-Stream (voice-live.jsonl, Event 16/33):
          // nach einer Pause meldet der On-Device-Recognizer KEIN isFinal und erhoeht die
          // Generation nicht (gen bleibt 1, committed='', segments=0) - er verwirft den
          // vorherigen Satz aus seinem eigenen Ergebnis: timeline 2 -> 0 -> 5. Die leere
          // timeline wird unten absichtlich verworfen (kein Flackern), deshalb MUSS hier
          // festgeschrieben werden, was zuletzt auf dem Schirm stand. Sonst ersetzt der
          // naechste Satz den vorherigen.
          if (!p.timeline.length && rec.timeline && rec.timeline.length) {
            var fertig = composeVoiceText(rec.timeline);
            if (fertig) { rec.sichtSnapshot = verschmelzen(rec.sichtSnapshot, fertig); vlog('utterance_committed', { chars: fertig.length, snapshot: rec.sichtSnapshot.length }); }
          }
          // The helper owns audio identity and ordering. Keep its complete snapshot intact.
          if (p.timeline.length || !rec.timeline || !rec.timeline.length || p.final) rec.timeline = normalizeVoiceTimeline(p.timeline);
        } else {
          if (typeof p.committed === 'string') commitSnapshot(p.committed);
          if (typeof p.interim === 'string') rec.interim = p.interim.trim();
          else if (typeof p.partial === 'string' && typeof p.committed !== 'string') rec.interim = p.partial.trim();
        }
        rec.display = typeof p.display === 'string' ? p.display.trim() : '';
        if (typeof p.overlap_resolutions === 'number') rec.overlaps = p.overlap_resolutions;
        if (typeof p.interim_replacements === 'number') rec.replacements = p.interim_replacements;
        if (p.final) {
          // A recognizer final APPENDS and the recording visibly continues. It is the END of the
          // UserRecordingSession only if the user asked to stop.
          if (!rec.timeline) commitSnapshot(typeof p.text === 'string' && p.text ? p.text : micGesamt());
          rec.interim = ''; rec.display = '';
          vlog('final_appended', { committed_len_before: vorher, committed_len_after: micCommittedText().length,
            reconciled: !!p.reconciled, coverage: p.coverage, audio_chunks: p.audio_chunks, lost_audio_chunks: p.lost_audio_chunks });
          rec.audio = { chunks: p.audio_chunks || 0, lost: p.lost_audio_chunks || 0, reconciled: !!p.reconciled, coverage: p.coverage };
          micAnzeigen();
          if (rec.stopRequested && rec.stopAction === 'send') micFertig();
          return;
        }
        vlog('interim_updated', { committed_len_before: vorher, committed_len_after: micCommittedText().length });
        micAnzeigen();
      };
      window.__TAURI__.event.listen('intelligence-voice', voiceHandler);
      // Debug accessor: drives the REAL handler and the REAL DOM, without a microphone.
      state.voiceEvent = function (p) { voiceHandler({ payload: p }); return state.voiceRender(); };
      // Central task state lives natively; the window may be hidden meanwhile.
      window.__TAURI__.event.listen('intelligence-task', function (e) { taskState = (e && e.payload && e.payload.state) || taskState; });
    } catch (e) {}
    function showSources(list) {
      sources.replaceChildren(); if (!list || !list.length) return;
      var d = document.createElement('details'), s = document.createElement('summary'), ol = document.createElement('ol');
      s.textContent = 'Quellen · ' + list.length; d.appendChild(s);
      list.forEach(function (src) {
        var li = document.createElement('li'), b = document.createElement('button'), h = document.createElement('span');
        b.type = 'button'; b.textContent = src.title || src.url; b.title = src.url;
        b.onclick = function () { call('intelligence_open_source', { url: src.url }).catch(function (e) { meta.textContent = String(e); }); };
        try { h.textContent = ' — ' + new URL(src.url).hostname.replace(/^www\./, ''); } catch (e) {}
        li.appendChild(b); li.appendChild(h); ol.appendChild(li);
      });
      d.appendChild(ol); sources.appendChild(d);
    }
    try {
      window.__TAURI__.event.listen('intelligence-research', function (e) {
        if (!state.busy || !answer) return; var p = (e && e.payload) || {}, phase = p.phase === 'summarize' ? 'compose' : p.phase, ph = PHASEN[phase];
        if (!vorgangSchritt(phase, p)) return;
        if (host.phase && ph) host.phase(phase);
        schritte = [VORGANG[phase][0](p)];
        if (!streamEl) showSteps();
        if (statusEl) { statusEl.textContent = ph ? ph[1] : (phase === 'think' ? 'Denkt nach' : 'Wechselt Modell'); statusEl.dataset.k = 'arbeitet'; }
        if (!ph) return;
        if (p.phase !== 'search' && p.phase !== 'read') return;
        meta.textContent = 'Web-Recherche · nur lesen';
      });
      window.__TAURI__.event.listen('intelligence-token', function (e) {
        if (!state.busy || !answer) return;
        var tok = (e && e.payload && e.payload.token) || '';
        if (!tok) return;
        streamingText += tok;
        if (!streamEl) {
          answer.replaceChildren();
          streamEl = document.createElement('div');
          streamEl.className = 'ni-streaming';
          answer.appendChild(streamEl);
          liveModellZeigen();
        }
        scheduleStreamingRender();
      });
      window.__TAURI__.event.listen('intelligence-code-start', function (e) {
        var p = (e && e.payload) || {};
        if (!state.busy) return;
        codeLauf = {
          projekt: p.projekt || '',
          pfad: p.pfad || '',
          neu: !!p.neu,
          aufgabe: p.aufgabe || '',
          schritte: [],
          dateien: [],
          wechsel: null
        };
        codeKarteZeigen();
      });
      window.__TAURI__.event.listen('intelligence-code', function (e) {
        var p = (e && e.payload) || {};
        if (!state.busy || !codeLauf) return;
        codeLauf.schritte.push({
          label: String(p.label || ''),
          ok: !!p.ok,
          detail: String(p.detail || ''),
          ms: p.ms || 0,
          art: String(p.art || ''),
          datei: String(p.datei || ''),
          plus: Number(p.plus || 0),
          minus: Number(p.minus || 0),
          diff: String(p.diff || '')
        });
        if (Array.isArray(p.dateien)) codeLauf.dateien = p.dateien;
        codeKarteZeigen();
      });
      window.__TAURI__.event.listen('intelligence-code-wechsel', function (e) {
        var p = (e && e.payload) || {};
        if (!codeLauf) return;
        codeLauf.wechsel = String(p.text || '');
        codeKarteZeigen();
      });
      window.__TAURI__.event.listen('intelligence-model', function (e) {
        var p = e && e.payload;
        if (!p || !p.display_name) return;
        liveModell = p;
        if (state.status) state.status.active_runtime_model = p;
        modellAnzeige();
        liveModellZeigen();
      });
    } catch (e) {}
    call('intelligence_settings').then(function (r) {
      state.status = r; settings = r.settings; state.settings = settings; assistantMode = (r.assistant_mode === 'code') ? 'code' : 'work';
      if (r.code_style) codeStyle = r.code_style;
      if (r.engine) state.activeEngine = r.engine;
      ready = true; refresh(); modellAnzeige();
    }).catch(function () { state.status = { installed: false }; refresh(); });
    function ensure() {
      if (panel) return;
      panel = document.createElement('section'); panel.id = 'askNoki'; panel.setAttribute('role', 'dialog'); panel.setAttribute('aria-label', 'Ask Noki');
      panel.innerHTML = '<header>' +
        '<div class="ni-header-left">' +
          '<div class="ni-titel">' +
            '<div class="ni-titel-zeile1"><span class="ni-brand-title">Noki</span><span class="ni-model-indicator" aria-live="polite"></span></div>' +
            '<div class="ni-titel-zeile2"><span class="ni-runtime-status" title="Local Runtime"><span class="ni-runtime-text">Bereit</span></span></div>' +
          '</div>' +
        '</div>' +
        '<div class="ni-header-spacer"></div>' +
        '<div class="ni-header-right">' +
          '<div class="ni-assistant-modes" role="tablist" aria-label="Arbeitsmodus">' +
            '<button type="button" role="tab" data-assistant="work" class="aktiv">Work</button>' +
            '<button type="button" role="tab" data-assistant="code">Code</button>' +
          '</div>' +
          '<button type="button" class="ni-icon ni-settings-btn" data-act="settings" title="Einstellungen (Lokal / Cloud, Allgemein)" aria-label="Einstellungen">' +
            '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' +
              '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>' +
              '<circle cx="12" cy="12" r="3"/>' +
            '</svg>' +
          '</button>' +
          '<button type="button" class="ni-icon ni-menu-btn" data-act="menu" title="Chat-Verwaltung" aria-haspopup="true">⋯</button>' +
        '</div></header>' +
        '<div class="ni-menu" hidden role="menu">' +
          '<button type="button" data-act="neu" role="menuitem"><span>Neuer Chat</span></button>' +
          '<button type="button" data-act="verlauf" role="menuitem"><span>Verlauf</span></button>' +
        '</div>' +
        '<div class="ni-body"><div class="ni-frueher"></div><div class="ni-q"></div><div class="ni-answer" role="status" aria-live="polite"></div><div class="ni-sources"></div><div class="ni-tools"></div>' +
        '<div class="ni-feedback" hidden><button type="button" data-feedback="yes">Hilfreich</button><button type="button" data-feedback="no">Nicht nötig</button></div></div>' +
        '<div class="ni-code"></div>' +
        '<div class="ni-drawer" hidden></div><button type="button" class="ni-neu" hidden>Neue Antwort ↓</button>' +
        '<form><div class="ni-voice-live" hidden data-k="idle">' +
        '<div class="ni-voice-area">' +
        '<div class="ni-voice-head"><span class="ni-voice-state"><span class="ni-voice-status">Noki hört zu</span></span>' +
        '<div class="ni-live-meter" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></div>' +
        '<span class="ni-voice-worte"></span></div>' +
        '<div class="ni-voice-scroll" role="status" aria-live="polite"><span class="ni-voice-basis"></span><p></p><em class="ni-voice-interim"></em><span class="ni-voice-leer">Sprich einfach los – dein Text bleibt hier stehen.</span></div>' +
        '<div class="ni-voice-acts"><button type="button" class="ni-voice-toggle">Stop</button><button type="button" class="ni-voice-cancel">Abbrechen</button></div>' +
        '</div>' +
        '<div class="ni-voice-composer"><span>Audio abschicken</span><button type="button" class="ni-voice-send" aria-label="Audio abschicken"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 13.5V2.5M3 7.5l5-5 5 5"/></svg></button></div></div>' +
        '<div class="ni-anhaenge" hidden></div>' +
        '<div class="ni-browser" hidden><div class="ni-browser-kopf"><button type="button" class="ni-browser-hoch" title="Eine Ebene zurück">← Zurück</button><button type="button" class="ni-browser-home" title="Zur Startansicht">Start</button><span class="ni-browser-pfad"></span></div>' +
        '<div class="ni-browser-liste"></div>' +
        '<div class="ni-browser-preview" hidden><div class="ni-preview-kopf"><button type="button" class="ni-preview-zurueck" title="Zurück zur Liste">← Zurück</button><span class="ni-preview-name"></span><span class="ni-preview-meta"></span></div><div class="ni-preview-body"></div><div class="ni-preview-fuss"><button type="button" class="ni-preview-ab">Schließen</button><button type="button" class="ni-preview-ok">Diese Datei anhängen</button></div></div>' +
        '<div class="ni-browser-fuss"><span class="ni-browser-zahl"></span><div class="ni-browser-akt"><button type="button" class="ni-browser-ab">Abbrechen</button><button type="button" class="ni-browser-ok">Anhängen</button></div></div></div>' +
        '<div class="ni-composer-box">' +
        '<textarea rows="1" aria-label="Noki fragen" placeholder="Noki fragen …" maxlength="4000" autocomplete="off" spellcheck="true"></textarea>' +
        '<div class="ni-composer-actions">' +
        '<button type="button" class="ni-mic ni-clip" aria-label="Datei anhängen" title="Datei anhängen"><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M8 3.5v9M3.5 8h9"/></svg></button>' +
        '<button type="button" class="ni-mic" data-k="idle" aria-label="Spracheingabe" title="Spracheingabe (lokal)"><svg width="15" height="15" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><rect x="4.5" y="1" width="5" height="8" rx="2.5"/><path d="M2.5 6.5a4.5 4.5 0 0 0 9 0M7 11v2"/></svg><span class="ni-mic-eq" aria-hidden="true"><i></i><i></i><i></i></span></button>' +
        '<button class="ni-send" aria-label="Senden" type="submit"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 13.5V2.5M3 7.5l5-5 5 5"/></svg></button>' +
        '</div></div></form>' +
        '<footer><span class="ni-meta"></span><span class="ni-dev"></span></footer>';
      document.body.appendChild(panel);
      panel.dataset.assistantMode = assistantMode;
      panel.querySelectorAll('.ni-assistant-modes button').forEach(function (b) { b.classList.toggle('aktiv', b.dataset.assistant === assistantMode); });
      var hdr = panel.querySelector('header');
      hdr.onclick = function (e) {
        var am = e.target.closest('[data-assistant]');
        if (am) { switchAssistantMode(am.dataset.assistant); return; }
        var sb = e.target.closest('.ni-settings-btn, [data-act="settings"]');
        if (sb) { host.openSettings(); return; }
        var rt = e.target.closest('.ni-runtime-status');
        if (rt) { host.openSettings(); return; }
        var mb = e.target.closest('.ni-menu-btn');
        if (mb) {
          if (assistantMode === 'code') {
            if (codeView && codeView.toggleVerlauf) codeView.toggleVerlauf();
          } else {
            // Work: ⋯ IS the chat library (sidebar in this window).
            toggleVerlauf();
          }
          return;
        }
      };
      var niMenu = panel.querySelector('.ni-menu');
      if (niMenu) {
        niMenu.onclick = function (e) {
          var b = e.target.closest('[data-act]');
          if (!b) return;
          niMenu.hidden = true;
          var act = b.dataset.act;
          if (act === 'verlauf') toggleVerlauf(true);
          else if (act === 'neu') starteNeuenChat();
        };
      }
      // Code = overview + embedded terminal on the same persistent session as `noki code`.
      if (window.NokiCodeView) codeView = window.NokiCodeView(panel.querySelector('.ni-code'), {
        call: call,
        header: hdr,
        onStatus: function (s) { codeTerminalStatus = s; modellAnzeige(); },
        closeWindow: function () { close('btn'); },
        openSettings: function () { host.openSettings(); },
        // Change request typed in Code Space: same chat pipeline (TaskPlan,
        // router, history), bound to THAT project.
        folgeauftrag: function (text, pfad) {
          call('code_projekt_aktivieren', { pfad: pfad }).then(function () {
            input.value = text; submit('code-space');
          }).catch(function (e) { meta.textContent = String(e); });
        }
      });
      if (codeView && assistantMode === 'code') codeView.enter(codeTerminalStatus);
      body = panel.querySelector('.ni-body'); frueher = panel.querySelector('.ni-frueher'); question = panel.querySelector('.ni-q');
      statusEl = panel.querySelector('.ni-runtime-text'); modelIndicator = panel.querySelector('.ni-model-indicator'); gesichtEl = null;
      ladeChats();
      if (chats.length && !chat) chat = chats[0];
      drawer = panel.querySelector('.ni-drawer');
      var vb = panel.querySelector('.ni-verlauf-btn');
      if (vb) vb.onclick = function () { toggleVerlauf(); };
      answer = panel.querySelector('.ni-answer'); input = panel.querySelector(':scope > form textarea'); send = panel.querySelector('.ni-send');
      neuBtn = panel.querySelector('.ni-neu'); neuBtn.onclick = function () { autoFollow = true; zurNeuesten(true); };
      body.addEventListener('scroll', function () {
        var d = distUnten();
        if (d > 110) {
          autoFollow = false;
        } else if (d <= 100) {
          autoFollow = true;
        }
        if (nahUnten()) neuHinweis(false);
      }, { passive: true });
      // Anhaenge: der Nutzer waehlt, Noki durchsucht nichts. Die Dateien haengen
      // an genau diesem Chat und gelten nur fuer ihn.
      state.aktuelleAnhaenge = [];
      // Chips statt "1 Datei angehängt": man muss SEHEN, was an der Nachricht haengt.
      anhaengeZeigen = function () {
        var reihe = panel.querySelector('.ni-anhaenge');
        if (!reihe) return;
        var id = chatId();
        if (!id) {
          state.aktuelleAnhaenge = [];
          reihe.replaceChildren();
          reihe.hidden = true;
          return;
        }
        call('anhang_liste', { conversationId: id }).then(function (list) {
          state.aktuelleAnhaenge = Array.isArray(list) ? list.slice() : [];
          reihe.replaceChildren();
          if (!list || !list.length) { reihe.hidden = true; return; }
          reihe.hidden = false;
          list.forEach(function (a) {
            var chip = document.createElement('span');
            chip.className = 'ni-chip' + (a.is_image ? ' ni-chip-bild' : ' ni-chip-dok');
            var t = document.createElement('span');
            t.className = 'ni-chip-typ';
            t.textContent = String(a.name || '').split('.').pop().toUpperCase().slice(0, 4);
            if (a.is_image) {
              call('datei_thumbnail', { pfad: a.path }).then(function (uri) {
                if (!uri) return;
                var im = document.createElement('img');
                im.className = 'ni-thumb'; im.alt = '';
                im.onload = function () {
                  bildRuntimeBericht('attachment_load', a.path, im, uri);
                };
                im.onerror = function () {
                  bildRuntimeBericht('attachment_error', a.path, im, uri);
                };
                im.src = uri;
                t.replaceChildren(im);
              }).catch(function () {
                bildRuntimeBericht('attachment_call_error', a.path, null, '');
              });
            }
            var n = document.createElement('span');
            n.className = 'ni-chip-name'; n.textContent = a.name; n.title = a.name;
            var x = document.createElement('button');
            x.type = 'button'; x.className = 'ni-chip-x'; x.textContent = '×';
            x.title = 'Anhang entfernen'; x.setAttribute('aria-label', 'Anhang entfernen');
            x.onclick = function (ev) {
              ev.stopPropagation();
              call('anhang_entfernen', { conversationId: id, id: a.id })
                .then(anhaengeZeigen).catch(function () {});
            };
            chip.onclick = function () {
              if (!browser) return;
              browser.hidden = false;
              previewOeffnen(a.path);
            };
            chip.appendChild(t); chip.appendChild(n); chip.appendChild(x);
            reihe.appendChild(chip);
          });
        }).catch(function () {
          state.aktuelleAnhaenge = [];
          reihe.replaceChildren();
          reihe.hidden = true;
        });
      }
      state.anhaenge = anhaengeZeigen;
      // ---- Eingebauter Dateibrowser: ein Zustand DIESES Fensters ----------------
      // Kein zweites Fenster, keine Ampeln, kein Finder-Dialog. Er lebt im Panel
      // und verschwindet mit ihm.
      var browser = panel.querySelector('.ni-browser');
      var browserKopf = panel.querySelector('.ni-browser-kopf');
      var browserListe = panel.querySelector('.ni-browser-liste');
      var browserPreview = panel.querySelector('.ni-browser-preview');
      var previewZurueck = panel.querySelector('.ni-preview-zurueck');
      var previewName = panel.querySelector('.ni-preview-name');
      var previewMeta = panel.querySelector('.ni-preview-meta');
      var previewBody = panel.querySelector('.ni-preview-body');
      var previewAb = panel.querySelector('.ni-preview-ab');
      var previewOk = panel.querySelector('.ni-preview-ok');
      var browserFuss = panel.querySelector('.ni-browser-fuss');
      var browserPfad = panel.querySelector('.ni-browser-pfad');
      var browserZahl = panel.querySelector('.ni-browser-zahl');
      var chipRow = panel.querySelector('.ni-anhaenge');
      var browserOrdner = null, browserOben = null, browserWahl = [];
      var browserLadeVersion = 0, galerieGesamt = 0, galerieOffset = 0;
      var galerieLaedt = false, galerieRendern = null, galerieThumbObserver = null;
      var bHoch = panel.querySelector('.ni-browser-hoch');
      var bHome = panel.querySelector('.ni-browser-home');
      var previewDatei = null;

      function previewZu() {
        if (browserPreview) browserPreview.hidden = true;
        if (browserListe) browserListe.hidden = false;
        if (browserFuss) browserFuss.hidden = false;
        if (browserKopf) browserKopf.hidden = false;
        previewDatei = null;
      }
      // Jede Anhang-Sitzung beginnt sauber: der Ordner wird NICHT gemerkt.
      // Sonst landet man beim naechsten "+" wieder tief in einem alten Pfad.
      function browserZu() {
        if (browser) browser.hidden = true;
        previewZu();
        browserLadeVersion++;
        galerieRendern = null;
        if (galerieThumbObserver) galerieThumbObserver.disconnect();
        browserWahl = []; browserOrdner = null; browserOben = null;
      }
      state.browserOffen = function () { return !!(browser && !browser.hidden); };
      function kurzGroesse(n) {
        if (!n) return '';
        return n < 1024 ? n + ' B' : n < 1048576 ? Math.round(n / 1024) + ' KB' : (n / 1048576).toFixed(1) + ' MB';
      }
      function typKurz(mime, name) {
        var ext = String(name || '').split('.').pop();
        if (ext && ext !== name && ext.length <= 4) return ext.toUpperCase();
        return String(mime || '').split('/').pop().toUpperCase().slice(0, 4);
      }
      function browserHinweis(text) {
        if (!browserListe) return;
        previewZu();
        browserListe.replaceChildren();
        var p = document.createElement('div');
        p.className = 'ni-browser-leer';
        p.textContent = text;
        browserListe.appendChild(p);
      }

      function bildRuntimeBericht(phase, pfad, img, uri) {
        var rect = img ? img.getBoundingClientRect() : null;
        var msg = '[THUMB_RUNTIME] phase=' + phase
          + ' path=' + pfad
          + ' complete=' + (!!(img && img.complete))
          + ' naturalWidth=' + (img ? img.naturalWidth : 0)
          + ' naturalHeight=' + (img ? img.naturalHeight : 0)
          + ' renderWidth=' + (rect ? Math.round(rect.width) : 0)
          + ' renderHeight=' + (rect ? Math.round(rect.height) : 0)
          + ' uri_bytes=' + (uri ? uri.length : 0);
        console.log(msg);
        call('browser_debug_log', { msg: msg }).catch(function () {});
      }

      function previewOeffnen(pfad) {
        if (!browserPreview || !previewBody) return;
        previewDatei = pfad;
        if (browserListe) browserListe.hidden = true;
        if (browserFuss) browserFuss.hidden = true;
        if (browserKopf) browserKopf.hidden = true;
        browserPreview.hidden = false;
        var fName = pfad.startsWith('photos://asset/') ? 'Foto' : (pfad.split('/').pop() || 'Datei');
        if (previewName) previewName.textContent = fName;
        if (previewMeta) previewMeta.textContent = 'Lädt Vorschau …';
        previewBody.replaceChildren();
        var ladeBox = document.createElement('div');
        ladeBox.className = 'ni-browser-leer';
        ladeBox.textContent = 'Vorschau wird geladen …';
        previewBody.appendChild(ladeBox);

        call('datei_vorschau', { pfad: pfad }).then(function (d) {
          if (previewDatei !== pfad) return;
          previewBody.replaceChildren();
          if (previewName) previewName.textContent = d.name || fName;
          var metaArr = [];
          if (d.metadaten) {
            if (d.metadaten.typ) metaArr.push(d.metadaten.typ);
            if (d.metadaten.abmessungen) metaArr.push(d.metadaten.abmessungen);
            if (d.metadaten.seiten) metaArr.push(d.metadaten.seiten);
            if (d.metadaten.zeilen) metaArr.push(d.metadaten.zeilen);
            if (d.metadaten.zeichen) metaArr.push(d.metadaten.zeichen);
            if (d.metadaten.groesse) metaArr.push(d.metadaten.groesse);
          } else if (d.groesse) {
            metaArr.push(kurzGroesse(d.groesse));
          }
          if (previewMeta) previewMeta.textContent = metaArr.join(' · ');

          if (d.art === 'bild' && d.bild_uri) {
            var imWrap = document.createElement('div');
            imWrap.className = 'ni-prev-img-wrap';
            var im = document.createElement('img');
            im.className = 'ni-preview-img';
            im.src = d.bild_uri;
            im.alt = d.name || 'Bild';
            imWrap.appendChild(im);
            previewBody.appendChild(imWrap);
          } else if (d.art === 'pdf') {
            if (d.bild_uri) {
              var pWrap = document.createElement('div');
              pWrap.className = 'ni-prev-img-wrap';
              var pIm = document.createElement('img');
              pIm.className = 'ni-preview-img ni-preview-pdf';
              pIm.src = d.bild_uri;
              pIm.alt = 'Erste Seite';
              pWrap.appendChild(pIm);
              previewBody.appendChild(pWrap);
            }
            if (d.text_inhalt) {
              var pTxt = document.createElement('pre');
              pTxt.className = 'ni-preview-text';
              pTxt.textContent = d.text_inhalt;
              previewBody.appendChild(pTxt);
            }
          } else if (d.art === 'csv' && d.csv_tabelle && d.csv_tabelle.length) {
            var tWrap = document.createElement('div');
            tWrap.className = 'ni-preview-table-wrap';
            var tbl = document.createElement('table');
            tbl.className = 'ni-preview-table';
            var thead = document.createElement('thead');
            var tr0 = document.createElement('tr');
            d.csv_tabelle[0].forEach(function (headCell) {
              var th = document.createElement('th');
              th.textContent = headCell;
              tr0.appendChild(th);
            });
            thead.appendChild(tr0);
            tbl.appendChild(thead);
            var tbody = document.createElement('tbody');
            d.csv_tabelle.slice(1).forEach(function (rowCells) {
              var tr = document.createElement('tr');
              rowCells.forEach(function (c) {
                var td = document.createElement('td');
                td.textContent = c;
                tr.appendChild(td);
              });
              tbody.appendChild(tr);
            });
            tbl.appendChild(tbody);
            tWrap.appendChild(tbl);
            previewBody.appendChild(tWrap);
          } else if (d.text_inhalt) {
            var txt = document.createElement('pre');
            txt.className = 'ni-preview-text';
            txt.textContent = d.text_inhalt;
            previewBody.appendChild(txt);
          } else {
            var noPrev = document.createElement('div');
            noPrev.className = 'ni-browser-leer';
            noPrev.textContent = 'Keine Text- oder Bildvorschau für ' + (d.mime || 'dieses Format') + ' verfügbar.';
            previewBody.appendChild(noPrev);
          }
        }).catch(function (err) {
          if (previewDatei !== pfad) return;
          previewBody.replaceChildren();
          var errBox = document.createElement('div');
          errBox.className = 'ni-browser-leer';
          errBox.textContent = 'Vorschau fehlgeschlagen: ' + (err && err.message ? err.message : String(err));
          previewBody.appendChild(errBox);
        });
      }

      // pfad === null -> Startansicht (Benutzerordner, Desktop, Dokumente, Downloads, Bilder ...)
      function browserLaden(pfad) {
        if (!browserListe) return;
        var ladeVersion = ++browserLadeVersion;
        galerieRendern = null;
        if (galerieThumbObserver) galerieThumbObserver.disconnect();
        galerieGesamt = galerieOffset = 0;
        galerieLaedt = false;
        previewZu();
        var ruf = pfad
          ? call('datei_browser_liste', { pfad: pfad })
          : call('datei_browser_start');
        ruf.then(function (d) {
          if (ladeVersion !== browserLadeVersion) return;
          var start = !!d.start;
          var galerie = !!d.ist_galerie;
          galerieGesamt = galerie ? (d.photos_total || 0) : 0;
          galerieOffset = galerie ? (d.photos_next_offset || 0) : 0;
          if (browser) browser.classList.toggle('ni-browser-galerie-offen', galerie);
          browserOrdner = start ? null : d.pfad;
          browserOben = start ? null : (d.oben || null);
          if (browserPfad) browserPfad.textContent = start ? 'Orte' : String(d.pfad || '').replace(/^\/Users\/[^/]+/, '~');
          if (bHoch) bHoch.disabled = start;
          if (bHome) bHome.disabled = start;
          browserListe.className = 'ni-browser-liste' + (start ? ' ni-browser-start' : (galerie ? ' ni-browser-galerie' : ''));
          browserListe.replaceChildren();
          var galerieAnker = null;
          if (galerie) {
            var observer = new IntersectionObserver(function (records) {
              records.forEach(function (record) {
                if (!record.isIntersecting) return;
                observer.unobserve(record.target);
                if (record.target.nokiThumbLaden) record.target.nokiThumbLaden();
              });
            }, { root: browserListe, rootMargin: '350px 0px' });
            galerieThumbObserver = observer;
            galerieAnker = document.createElement('span');
            galerieAnker.hidden = true;
            browserListe.appendChild(galerieAnker);
          }

          if (d.photos_hinweis) {
            var hint = document.createElement('div');
            hint.className = 'ni-browser-perm-hint';
            hint.textContent = d.photos_hinweis;
            browserListe.appendChild(hint);
          }

          if (!(d.eintraege || []).length) {
            browserHinweis(d.photos_hinweis || 'Keine Dateien');
            return;
          }

          var renderedCards = 0;
          function renderEintrag(e) {
            var row = document.createElement('div');
            var isChosen = browserWahl.indexOf(e.pfad) >= 0;
            row.className = 'ni-browser-zeile' + (e.ordner ? ' ordner' : '') + (e.paket ? ' paket' : '') + (isChosen ? ' gewaehlt' : '');
            row.tabIndex = 0;
            row.setAttribute('role', 'button');
            row.setAttribute('aria-label', e.ordner ? (e.name + ' öffnen') : (galerie && e.bild ? (e.name + ' auswählen') : (e.name + ' auswählen oder öffnen')));
            if (galerie && e.bild) row.setAttribute('aria-pressed', isChosen ? 'true' : 'false');
            row.onkeydown = function (ev) {
              if (ev.key === 'Enter' || ev.key === ' ') {
                ev.preventDefault();
                row.click();
              }
            };

            // Fall 1: Galerie-Karte für Bilder
            if (galerie && e.bild) {
              var thumbWrap = document.createElement('div');
              thumbWrap.className = 'ni-galerie-thumb-wrap';

              var im = document.createElement('img');
              im.className = 'ni-galerie-thumb';
              im.alt = e.name || '';
              im.decoding = 'async';
              var thumbUri = '';
              im.onload = function () {
                bildRuntimeBericht('gallery_load', e.pfad, im, thumbUri);
              };
              im.onerror = function () {
                bildRuntimeBericht('gallery_error', e.pfad, im, thumbUri);
              };
              thumbWrap.appendChild(im);

              im.nokiThumbLaden = function () {
                call('datei_thumbnail', { pfad: e.pfad }).then(function (uri) {
                  thumbUri = uri || '';
                  if (thumbUri) im.src = thumbUri;
                }).catch(function (err) {
                  console.warn('[GALLERY_THUMB_CALL_FAIL] ' + e.pfad, err);
                  bildRuntimeBericht('gallery_call_error', e.pfad, im, '');
                });
              };

              var chk = document.createElement('span');
              chk.className = 'ni-browser-check';
              chk.setAttribute('aria-hidden', 'true');
              chk.textContent = isChosen ? '✓' : '';
              thumbWrap.appendChild(chk);
              row.appendChild(thumbWrap);

              var n = document.createElement('span');
              n.className = 'ni-browser-name';
              n.textContent = e.name;
              n.title = e.name + (e.erstellt ? (' · ' + e.erstellt.slice(0, 10)) : '');
              row.appendChild(n);

              row.onclick = function () {
                var idx = browserWahl.indexOf(e.pfad);
                if (idx >= 0) browserWahl.splice(idx, 1); else browserWahl.push(e.pfad);
                var sel = browserWahl.indexOf(e.pfad) >= 0;
                row.classList.toggle('gewaehlt', sel);
                row.setAttribute('aria-pressed', sel ? 'true' : 'false');
                chk.textContent = sel ? '✓' : '';
                if (browserZahl) browserZahl.textContent = browserWahl.length ? (browserWahl.length + ' ausgewählt') : '';
              };
              row.ondblclick = function (ev) {
                ev.stopPropagation();
                var id = chatId();
                if (!id) { starteNeuenChat(); id = chatId(); }
                if (!id) return;
                call('anhang_hinzufuegen', { conversationId: id, pfade: [e.pfad] })
                  .then(function () { browserZu(); anhaengeZeigen(); })
                  .catch(function (err) { if (meta) meta.textContent = String(err); browserZu(); });
              };

              renderedCards++;
              if (e.quelle === 'photos' && galerieAnker) browserListe.insertBefore(row, galerieAnker);
              else browserListe.appendChild(row);
              if (galerieThumbObserver) galerieThumbObserver.observe(im);
              else im.nokiThumbLaden();
              return;
            }

            // Fall 2: Startansicht oder reguläre Datei-/Ordner-Karte
            // Checkbox für Mehrfachauswahl (nur bei anhaengbaren Dateien)
            if (!e.ordner && !e.paket) {
              var chk = document.createElement('button');
              chk.type = 'button';
              chk.className = 'ni-browser-check';
              chk.title = 'Datei auswählen';
              chk.textContent = isChosen ? '✓' : '';
              chk.onclick = function (ev) {
                ev.stopPropagation();
                var idx = browserWahl.indexOf(e.pfad);
                if (idx >= 0) browserWahl.splice(idx, 1); else browserWahl.push(e.pfad);
                var sel = browserWahl.indexOf(e.pfad) >= 0;
                row.classList.toggle('gewaehlt', sel);
                chk.textContent = sel ? '✓' : '';
                if (browserZahl) browserZahl.textContent = browserWahl.length ? (browserWahl.length + ' ausgewählt') : '';
              };
              row.appendChild(chk);
            }

            var t = document.createElement('span');
            t.className = 'ni-browser-typ' + (e.ordner ? ' ordnersym' : '') + (e.paket ? ' paketsym' : '');
            t.textContent = e.ordner ? '▸' : (e.paket ? 'Paket' : typKurz(e.mime, e.name));
            if (e.bild || e.mime === 'application/pdf') {
              call('datei_thumbnail', { pfad: e.pfad }).then(function (uri) {
                if (!uri) return;
                var im = document.createElement('img');
                im.className = 'ni-thumb'; im.src = uri; im.alt = '';
                t.replaceChildren(im);
              }).catch(function () {});
            }
            // Die Startorte sind selbst die Navigation; dort braucht es keine
            // zusaetzlichen Pfeile auf den Kacheln.
            if (!(start && e.ordner)) row.appendChild(t);

            var n = document.createElement('span');
            n.className = 'ni-browser-name';
            n.textContent = e.name;
            if (e.hinweis) n.title = e.hinweis;
            row.appendChild(n);

            var g = document.createElement('span');
            g.className = 'ni-browser-groesse';
            g.textContent = e.hinweis ? e.hinweis : (e.ordner ? '' : kurzGroesse(e.groesse));
            row.appendChild(g);

            if (!e.ordner && !e.paket) {
              var oeffnenBtn = document.createElement('button');
              oeffnenBtn.type = 'button';
              oeffnenBtn.className = 'ni-browser-prev-btn';
              oeffnenBtn.textContent = 'Öffnen';
              oeffnenBtn.title = 'In Ask Noki ansehen';
              oeffnenBtn.onclick = function (ev) {
                ev.stopPropagation();
                previewOeffnen(e.pfad);
              };
              row.appendChild(oeffnenBtn);
            }

            row.onclick = function () {
              if (e.ordner) {
                browserLaden(e.pfad);
                return;
              }
              previewOeffnen(e.pfad);
            };

            renderedCards++;
            browserListe.appendChild(row);
          }
          (d.eintraege || []).forEach(renderEintrag);
          galerieRendern = galerie ? renderEintrag : null;
          if (galerie) {
            console.log('[GALLERY_DEBUG] backend_assets=' + (d.eintraege || []).length + ' after_filter=' + (d.eintraege || []).length + ' rendered_cards=' + renderedCards);
            setTimeout(function () {
              if (!browserListe.classList.contains('ni-browser-galerie')) return;
              var layoutMsg = '[GALLERY_LAYOUT] clientWidth=' + browserListe.clientWidth
                + ' scrollWidth=' + browserListe.scrollWidth
                + ' clientHeight=' + browserListe.clientHeight
                + ' scrollHeight=' + browserListe.scrollHeight
                + ' horizontal=' + (browserListe.scrollWidth > browserListe.clientWidth + 1)
                + ' vertical=' + (browserListe.scrollHeight > browserListe.clientHeight + 1);
              console.log(layoutMsg);
              call('browser_debug_log', { msg: layoutMsg }).catch(function () {});
              if (browserListe.scrollHeight <= browserListe.clientHeight + 240) galerieWeitereLaden();
            }, 0);
          }
          if (browserZahl) browserZahl.textContent = browserWahl.length ? (browserWahl.length + ' ausgewählt') : '';
        }).catch(function (e) {
          if (ladeVersion !== browserLadeVersion) return;
          browserHinweis(String(e && e.message ? e.message : e) || 'Ordner konnte nicht geladen werden');
        });
      }
      function galerieWeitereLaden() {
        if (!galerieRendern || galerieLaedt || galerieOffset >= galerieGesamt) return;
        galerieLaedt = true;
        var ladeVersion = browserLadeVersion, offset = galerieOffset;
        call('datei_galerie_seite', { offset: offset }).then(function (d) {
          if (ladeVersion !== browserLadeVersion || !galerieRendern) return;
          var neu = d.eintraege || [];
          galerieGesamt = d.photos_total || galerieGesamt;
          galerieOffset = d.photos_next_offset || offset + neu.length;
          neu.forEach(galerieRendern);
          console.log('[GALLERY_PAGE] offset=' + offset + ' loaded=' + neu.length + ' next=' + galerieOffset + ' total=' + galerieGesamt);
          call('browser_debug_log', { msg: '[GALLERY_PAGE] offset=' + offset + ' loaded=' + neu.length + ' next=' + galerieOffset + ' total=' + galerieGesamt }).catch(function () {});
          galerieLaedt = false;
          if (neu.length && browserListe.scrollHeight <= browserListe.clientHeight + 240) galerieWeitereLaden();
        }).catch(function (err) {
          galerieLaedt = false;
          console.warn('[GALLERY_PAGE_ERROR]', err);
        });
      }
      browserListe.addEventListener('scroll', function () {
        if (browserListe.scrollTop + browserListe.clientHeight >= browserListe.scrollHeight - 240) galerieWeitereLaden();
      }, { passive: true });
      var clip = panel.querySelector('.ni-clip');
      if (clip) clip.onclick = function () {
        var id = chatId();
        if (!id) { starteNeuenChat(); id = chatId(); }
        if (!id) { if (meta) meta.textContent = 'Bitte zuerst einen Chat starten.'; return; }
        if (!browser) return;
        if (!browser.hidden) { browserZu(); return; }
        browserWahl = []; browserOrdner = null; browserOben = null;
        browser.hidden = false;
        browserLaden(null);
      };
      if (bHoch) bHoch.onclick = function () { browserLaden(browserOben || null); };
      if (bHome) bHome.onclick = function () { browserWahl = []; browserLaden(null); };
      var bAb = panel.querySelector('.ni-browser-ab');
      if (bAb) bAb.onclick = function () { browserZu(); };
      var bOk = panel.querySelector('.ni-browser-ok');
      if (bOk) bOk.onclick = function () {
        var id = chatId();
        if (!id) { starteNeuenChat(); id = chatId(); }
        if (!id || !browserWahl.length) { browserZu(); return; }
        call('anhang_hinzufuegen', { conversationId: id, pfade: browserWahl.slice() })
          .then(function () { browserZu(); anhaengeZeigen(); })
          .catch(function (e) { if (meta) meta.textContent = String(e); browserZu(); });
      };
      if (previewZurueck) previewZurueck.onclick = previewZu;
      if (previewAb) previewAb.onclick = browserZu;
      if (previewOk) previewOk.onclick = function () {
        if (!previewDatei) return;
        var id = chatId();
        if (!id) { starteNeuenChat(); id = chatId(); }
        if (!id) return;
        call('anhang_hinzufuegen', { conversationId: id, pfade: [previewDatei] })
          .then(function () { browserZu(); anhaengeZeigen(); })
          .catch(function (e) { if (meta) meta.textContent = String(e); browserZu(); });
      };
      mic = panel.querySelector('.ni-mic:not(.ni-clip)'); mic.onclick = micKlick; modusEl = panel.querySelector('.ni-modus'); modusCurrent = panel.querySelector('.ni-mode-current'); if (modusEl) renderModus();
      voiceLive = panel.querySelector('.ni-voice-live'); voiceText = panel.querySelector('.ni-voice-scroll p'); voiceInterim = panel.querySelector('.ni-voice-interim');
      voiceStatus = panel.querySelector('.ni-voice-status'); voiceWorte = panel.querySelector('.ni-voice-worte'); voiceLeer = panel.querySelector('.ni-voice-leer'); voiceBasis = panel.querySelector('.ni-voice-basis');
      voiceMeter = panel.querySelector('.ni-live-meter'); setMeter(0);
      voiceSend = panel.querySelector('.ni-voice-send'); voiceCancel = panel.querySelector('.ni-voice-cancel'); voiceToggle = panel.querySelector('.ni-voice-toggle');
      voiceSend.onclick = micSenden; voiceCancel.onclick = micAbbrechen; voiceToggle.onclick = micToggle;
      if (modusCurrent) modusCurrent.onclick = modusUmschalten;
      if (modusEl) modusEl.onclick = function (e) { var b = e.target.closest('[data-modus]'); if (b) setModus(b.dataset.modus); };
      tools = panel.querySelector('.ni-tools'); sources = panel.querySelector('.ni-sources'); meta = panel.querySelector('.ni-meta'); devEl = panel.querySelector('.ni-dev'); feedback = panel.querySelector('.ni-feedback');
      // Render only after every conversation node exists. Previously
      // zeigeChat() ran while tools/sources/meta were still undefined and
      // aborted the rest of Ask Noki's event-listener setup.
      modellAnzeige(); laufzeitAnzeige(); renderCodeTerminal(); zeigeChat(); zurNeuesten(true);
      ['pointerdown','pointerup','mousedown','mouseup','click','dblclick','wheel','contextmenu'].forEach(function (n) {
        panel.addEventListener(n, function (e) { e.stopPropagation(); });
      });
      var lastPrefixPress = 0;
      var prefixWindowEnd = 0;
      panel.addEventListener('keydown', function (e) {
        // The native Carbon shortcut owns ^/° + 0 for the whole app. A second
        // WebView toggle can turn the user's deliberate hide straight back
        // into show, especially when focus changes as the window disappears.
        if (cmd0Nativ) return;
        var isPrefix = e.code === 'Backquote' || e.code === 'IntlBackslash' || e.key === '^' || e.key === '°' || e.keyCode === 192 || e.keyCode === 220;
        var now = Date.now();
        if (isPrefix && !e.metaKey && !e.ctrlKey && !e.altKey) {
          if (now - lastPrefixPress < 420) {
            e.preventDefault();
            e.stopPropagation();
            lastPrefixPress = 0;
            prefixWindowEnd = 0;
            if (host.toggleSettings) host.toggleSettings();
            else host.openSettings();
            return;
          }
          lastPrefixPress = now;
          prefixWindowEnd = now + 1300;
        } else if (now < prefixWindowEnd && (e.key === '0' || e.code === 'Digit0' || e.keyCode === 48)) {
          e.preventDefault();
          e.stopPropagation();
          prefixWindowEnd = 0;
          toggleWarp();
          return;
        }
        e.stopPropagation();
      });
      input.addEventListener('input', feldHoehe);
      // Enter sends, Shift+Enter makes a new line (code, lists).
      input.addEventListener('keydown', function (e) { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); submit('enter'); } });
      // THE blank-screen cause: .ni-send is type="submit" inside this <form>,
      // and nothing ever cancelled the browser's own submission. A click
      // therefore NAVIGATED the webview - the panel DOM was torn down and Work
      // came back as an empty black surface, while the window itself stayed.
      // Enter never showed it because the textarea handler cancels first.
      var formular = panel.querySelector(':scope > form');
      if (formular) {
        formular.addEventListener('submit', function (e) {
          e.preventDefault();
          e.stopPropagation();
          submit('send-button');
        });
      }
      var cz = panel.querySelector('.ni-chat-zu'); if (cz) cz.onclick = chatSchliessen;
      var setBtn = panel.querySelector('.ni-settings'); if (setBtn) setBtn.onclick = function () { host.openSettings(); };
      feedback.onclick = function (e) {
        var b = e.target.closest('[data-feedback]'); if (!b || !tipKey) return;
        var key = tipKey; feedback.hidden = true; tipKey = null;
        call('intelligence_feedback', { key: key, helpful: b.dataset.feedback === 'yes' }).then(function () {
          meta.textContent = settings.patterns ? 'Feedback lokal gemerkt' : 'Feedback für diese Sitzung';
        }).catch(function (e) { meta.textContent = String(e); });
      };
      // Settings is a separate configuration surface: working there never closes Ask Noki.
      var inSettings = function (t) { return !!(t && t.closest && t.closest('#einstellungen')); };
      document.addEventListener('pointerdown', function (e) {
        if (modusEl && !modusEl.hidden && !e.target.closest('.ni-modepicker')) modusZu();
        var niM = panel ? panel.querySelector('.ni-menu') : null;
        if (niM && !niM.hidden && !e.target.closest('.ni-menu, .ni-menu-btn')) niM.hidden = true;
        if (state.opened && !panel.contains(e.target) && !inSettings(e.target) && !(ind && ind.contains(e.target))) close('aussen:' + ((e.target && (e.target.id || e.target.tagName)) || '?'));
      }, true);
      function escNormal() {
        if (modusEl && !modusEl.hidden) modusZu();
        else if (micZustand !== 'idle') micAbbrechen();
        else close('esc');
      }
      document.addEventListener('keydown', function (e) {
        if (!state.opened || e.key !== 'Escape' || inSettings(e.target)) return;
        e.preventDefault(); e.stopPropagation();
        // ESC IM NATIVEN VOLLBILD BEENDET DAS VOLLBILD, ES VERSTECKT NICHT.
        //
        //  Frueher hing das an einem eigenen state.fullscreen samt
        //  panel.style.width/height — einem Rest der alten CSS-Grossansicht.
        //  Gesetzt wurde dieses Flag nirgends mehr, also lief ESC im echten
        //  macOS-Vollbild in close('esc') und damit in win.hide(): ein
        //  verstecktes Fenster in einem Vollbild-Space. Genau dort ging das
        //  Zurueckstellen auf die vorherige Groesse verloren.
        //
        //  Massgeblich ist jetzt der ECHTE Fensterzustand. Das Zuruecksetzen
        //  von Groesse und Position macht NSWindow selbst; hier wird nur
        //  umgeschaltet. Kein CSS-Vollbild, kein zweites Fenster.
        if (host.fullscreen) {
          e.stopImmediatePropagation();
          host.fullscreen(false, function (warVoll) { if (!warVoll) escNormal(); });
          return;
        }
        escNormal();
      }, true);
    }
    function open(tip) {
      if (!settings.ask) return;
      // Im Desktop-Character-Host lebt Ask Noki in einem eigenen nativen
      // Fenster. Dieselbe Intelligence-UI wird dort erzeugt; im Character-
      // DOM entsteht niemals eine zweite sichtbare Character-/Ask-Instanz.
      if (host.openNative) { host.openNative(tip || null); return; }
      if (!ready) { host.openSettings(); return; }
      host.closePanels(); ensure();
      if (!state.userPos) {
        var f = flaeche();
        var k = host.anchor();
        var w = panel.offsetWidth || 440;
        var h = panel.offsetHeight || 520;
        state.userPos = platzieren(k, w, h, f);
      }
      state.opened = true; panel.hidden = false; panel.classList.toggle('ni-tip', !!tip);
      gesichtAn(true);
      panel.style.display = 'block'; host.hold(true); indikatorAus(); tick();
      if (codeView && assistantMode === 'code') codeView.enter(codeTerminalStatus);
      if (tip) {
        tipKey = tip.key; frueher.replaceChildren(); question.textContent = ''; showBlocks([{ type: 'text', text: tip.text }]); feedback.hidden = false; tipUntil = Date.now() + 14000;
        meta.textContent = 'Timer-Hinweis · lokal';
      } else {
        tipUntil = 0; feedback.hidden = true;
        var off = !settings.ask, fehlt = state.status && state.status.installed === false;
        input.disabled = off || fehlt; send.disabled = off || fehlt || state.busy;
        // Opening the window is UI work only. The Dynamic Ranker decides first;
        // local weights are loaded later, and only if its winner is LOCAL.
        setStatus(off ? 'aus' : fehlt ? 'fehlt' : state.busy ? 'denkt' : 'bereit');
        if (off) showBlocks([{ type: 'status', text: 'Ask Noki ist ausgeschaltet. Einschalten unter Einstellungen → Intelligence.' }]);
        else if (fehlt) showBlocks([{ type: 'status', text: 'Lokales Modell nicht verfügbar. Bitte den llama.cpp-Router starten.' }]);
        else if (state.busy) {
          if (schritte && schritte.length) showSteps();
          requestAnimationFrame(function () { zurNeuesten(true); });
        } else if (unread) {
          unread = false;
          call('intelligence_seen').catch(function () {});
          zeigeChat();
          requestAnimationFrame(function () { zurNeuesten(true); });
        } else if (assistantMode === 'work' && chat && chat.turns && chat.turns.length) {
          if (!question || !question.textContent) zeigeChat();
          requestAnimationFrame(function () { zurNeuesten(true); });
        }
        // Snapshot before Noki becomes frontmost. Only permitted fields are collected.
        var opening = generation;
        call('intelligence_context', { noki: host.context() }).then(function (c) { if (opening === generation) context = c; }).catch(function () {}).finally(function () {
          if (!state.opened) return;
          host.front(); setTimeout(function () { if (state.opened && !input.disabled) input.focus(); }, 80);
        });
      }
    }
    // X / Esc / click beside: hide the window only. A running task, the chat and its context stay.
    function close(grund) {
      if (!state.opened) return;
      state.closeLog = (state.closeLog || []).concat([(typeof grund === 'string' ? grund : 'extern') + '@' + Date.now()]).slice(-10);   // diagnostics only
      state.opened = false; state.rect = null; tipUntil = 0;
      state.sprechend = false; gesichtAn(false);   // geschlossen = keine Animation
      if (panel) {
        if (host.openNative) panel.style.display = 'none';
        toggleVerlauf(false);
        // Temporaere Attachment-UI gehoert zum sichtbaren Fenster - nie daneben.
        var br = panel.querySelector('.ni-browser'); if (br) br.hidden = true;
      }
      if (codeView && assistantMode === 'code') codeView.leave();
      // Hiding the window is not a user decision about the recording: a running
      // UserRecordingSession keeps going and is still there when the panel comes back.
      if (!state.busy) host.thinking(false);
      host.hold(false); host.back(); host.changed();
    }
    // "Chat schließen": end the conversation on purpose – cancel a running task, free the session context.
    function chatSchliessen() {
      if (state.busy) call('intelligence_cancel').catch(function () {});
      generation++; state.busy = false; chat = null; unread = false; pendingTool = null; schritte = []; wartendeNachricht = null;
      if (panel) { zeigeChat(); meta.textContent = ''; send.disabled = false; input.disabled = false; setStatus('bereit'); }
      host.thinking(false); indikatorAus(); close();
    }
    function buildIntelligenceChatArgs(q, aktivChat, verlaufModell, hostContext, attachmentsSnapshot) {
      var baseCtx = hostContext && typeof hostContext === 'object' ? hostContext : {};
      var nokiCtx = {
        conversation_id: (aktivChat && aktivChat.id) ? String(aktivChat.id) : '',
        timer_remaining_s: baseCtx.timer_remaining_s != null ? baseCtx.timer_remaining_s : null,
        timer_session: baseCtx.timer_session != null ? baseCtx.timer_session : null,
        focus: !!baseCtx.focus,
        workspace: baseCtx.workspace ? String(baseCtx.workspace) : null,
        freeze: !!baseCtx.freeze,
        recording: !!baseCtx.recording,
        attachments: Array.isArray(attachmentsSnapshot) ? attachmentsSnapshot : []
      };
      return {
        question: q,
        noki: nokiCtx,
        history: verlaufModell || []
      };
    }
    state.buildIntelligenceChatArgs = buildIntelligenceChatArgs;
    function submit(quelle) {
      var q = input.value.trim(); if (!q || !settings.ask) return;
      // Busy: take the message out of the field and send it when the running
      // one is done. Leaving it behind meant the next thing typed was appended
      // to it - a hidden half-sent state that produced merged messages.
      if (state.busy) {
        wartendeNachricht = q;
        input.value = ''; feldHoehe();
        meta.textContent = 'Wird direkt nach der laufenden Antwort gesendet …';
        return;
      }
      workDiag('submit', { quelle: quelle || 'unbekannt' });
      var request = ++generation; state.busy = true; liveModell = null; codeLauf = null; pendingTool = null; tools.replaceChildren(); sources.replaceChildren(); feedback.hidden = true;
      panel.classList.remove('ni-tip'); tipUntil = 0;
      autoFollow = true; streamingText = ''; streamEl = null;
      if (streamRenderTimer) { clearTimeout(streamRenderTimer); streamRenderTimer = null; }
      if (!chat) neuerChat(q);
      else if (chat.turns.length === 0 && (!chat.titel || chat.titel === 'Neuer Chat')) {
        chat.titel = titelAus(q);
      }
      chat.updated_at = Date.now();
      chat.preview = q.replace(/\s+/g, ' ').slice(0, 100);
      speichereChats();
      var aktiv = chat, verlaufModell = modellVerlauf(), warm = state.status && state.status.loaded, modusFrage = settings.mode;
      indArt = 'zahnrad';
      // The sent question moves up into the conversation; the field is free again at once.
      send.disabled = true; input.value = ''; feldHoehe(); micAbbrechen(); zeigeFrueher(aktiv.turns.length); question.textContent = q; meta.textContent = ''; neuHinweis(false);
      toggleVerlauf(false); setStatus('denkt'); schritte = ['Noki denkt …']; vorgang = []; showSteps(); zurNeuesten(true); host.thinking(true);
      state.sprechend = false; gesichtSetzen();

      // Immutable snapshot of attachments captured before dispatch
      var attachmentsSnapshot = (state.aktuelleAnhaenge || []).slice();

      if (state.status) {
        state.status.active_runtime_model = null;
        modellAnzeige();
        laufzeitAnzeige();
      }

      // Clear composer attachment UI after dispatch so next prompt is clean
      if (attachmentsSnapshot.length > 0) {
        var aId = chatId();
        attachmentsSnapshot.forEach(function (att) {
          if (aId && att.id) {
            call('anhang_entfernen', { conversationId: aId, id: att.id }).catch(function () {});
          }
        });
        state.aktuelleAnhaenge = [];
        var reiheEl = panel.querySelector('.ni-anhaenge');
        if (reiheEl) { reiheEl.replaceChildren(); reiheEl.hidden = true; }
      }

      var chatPayload = buildIntelligenceChatArgs(q, aktiv, verlaufModell, host.context(), attachmentsSnapshot);
      call('intelligence_chat', chatPayload).then(function (r) {
        if (request !== generation) return;
        renderStreaming(true);
        var vorgehen = vorgangZusammenfassung();
        if (r.tool && r.tool.label) vorgehen.push('Werkzeug · ' + r.tool.label);
        var turn = { q: q, text: r.text, vorgehen: vorgehen, sources: r.sources || [], research: r.research || null, route: r.route, plan: r.plan || null, conf: r.confidence && r.confidence.level, tool: r.tool || null, code_actions: r.code_actions || [], code_projekt: r.code_projekt || null, runtime_model: r.runtime_model || null, finish_reason: r.finish_reason || null, output_word_count: r.output_word_count == null ? null : r.output_word_count, modus: modusFrage,
          web: r.route === 'WEB' || (r.sources || []).length > 0, zeit: Date.now(), timings: r.timings || null, abstention: r.abstention_reason || null, attachments: attachmentsSnapshot };
        if (r.timings) { state.lastTimings = r.timings; try { console.debug('noki-latency', r.route, JSON.stringify(r.timings)); } catch (x) {} }
        aktiv.turns.push(turn); if (aktiv.turns.length > 40) aktiv.turns.shift();
        if (typeof state.stimmeHoerer === 'function') {
          try {
            state.stimmeHoerer({
              text: turn.text || '', route: turn.route || '',
              tool: turn.tool || null,
              // Das WIRKLICH benutzte Modell, so wie der Router es meldet.
              runtime_model: turn.runtime_model || null
            });
          } catch (x) {}
        }
        aktiv.updated_at = Date.now();
        aktiv.preview = (turn.text || '').replace(/\s+/g, ' ').slice(0, 100);
        speichereChats();
        if (state.status) {
          // Clear stale provenance as well: an abstention before synthesis did
          // not execute the model from the preceding turn.
          state.status.active_runtime_model = r.runtime_model || null;
          modellAnzeige();
          laufzeitAnzeige();
        }
        if (chat === aktiv) {
          zeigeChat();
          if (autoFollow && body) { body.scrollTop = body.scrollHeight; } else { zurNeuesten(false); }
          workInvariante('nach-render');
        } else {
          workDiag('conversation-gewechselt', { war: aktiv.id ? String(aktiv.id) : null });
        }
        setStatus('bereit'); host.thinking(false); gesichtSetzen();
        if (r.memory_saved && state.status) { state.status.memory_count = (state.status.memory_count || 0) + 1; if (memOpen) loadMem(); }
        if (r.tool && r.tool.available) {
          // Tool request → native gate (nonce, permission, confirmation) → existing Noki action.
          pendingTool = r.tool;
          if (r.tool.auto_execute && !r.tool.confirm && (/^(app|file|directory)\.open$/.test(r.tool.name) || r.tool.name === 'app.open_url' || r.tool.name === 'browser.search' || r.tool.name === 'browser.open')) {
            pendingTool = null;
            meta.textContent = 'Wird geöffnet …';
            call('intelligence_tool_execute', { nonce: r.tool.nonce, confirmed: false }).then(function (ok) {
              if (request !== generation) return;
              Promise.resolve(host.tool(ok)).then(function (erg) {
                if (request !== generation) return;
                // Only the real outcome: a refusal is shown as such.
                meta.textContent = erg && erg.ok === false ? (erg.grund || 'Nicht geöffnet') : (erg && erg.grund ? 'Geöffnet · ' + erg.grund : 'Geöffnet');
              });
            }).catch(function (e) { meta.textContent = String(e); });
            return;
          }
          var b = document.createElement('button'), confirmed = false; b.type = 'button'; b.textContent = r.tool.label;
          b.onclick = function () {
            var tool = pendingTool; if (!tool || b.disabled) return;
            if (tool.confirm && !confirmed) { confirmed = true; b.textContent = 'Wirklich „' + tool.label + '“? Bestätigen'; return; }
            b.disabled = true;
            call('intelligence_tool_execute', { nonce: tool.nonce, confirmed: confirmed }).then(function (ok) { close(); host.tool(ok); })
              .catch(function (e) { b.disabled = false; meta.textContent = String(e); });
          }; tools.appendChild(b);
        }
        if (state.opened) { if (!r.tool) host.explain(r.text); }
        else fertigMelden(r.text);
        // Dieselbe Stelle, die dem Character "sprechen" meldet, meldet es
        // dem Portraet. Danach kurz zufrieden, dann zurueck in Ruhe.
        state.sprechend = true; gesichtSetzen();
        setTimeout(function () {
          state.sprechend = false;
          state.zufrieden = Date.now() + 2600;
          gesichtSetzen();
        }, Math.min(9000, 2200 + String(r.text || '').length * 18));
        if (verlaufOffen) renderVerlauf();
      }).catch(function (e) {
        if (request !== generation) return;
        console.error('[INTELLIGENCE_CHAT_ERROR]', e);
        var rawErr = String(e);
        var m = rawErr.indexOf('invalid args') >= 0
          ? 'Die Anfrage konnte nicht übermittelt werden. Bitte erneut versuchen.'
          : rawErr;
        showBlocks([{ type: 'status', text: m }]); meta.textContent = ''; host.thinking(false); gesichtSetzen();
        aktiv.turns.push({ q: q, text: m, sources: [], route: 'UNKNOWN', fehler: true, modus: modusFrage, zeit: Date.now() });
        aktiv.updated_at = Date.now();
        aktiv.preview = m.replace(/\s+/g, ' ').slice(0, 100);
        speichereChats();
        setStatus(/nicht verfügbar|nicht sicher geladen/.test(m) ? 'fehlt' : /ausgeschaltet/.test(m) ? 'aus' : 'bereit');
        if (!state.opened) { unread = true; fertigT = Date.now() - 5000; }   // show the Ask Noki hint, no celebration for an error
      }).finally(function () {
        refreshStatus();
        if (request === generation) {
          state.busy = false; send.disabled = false; input.disabled = false;
          if (state.opened && assistantMode === 'work') input.focus();
          workInvariante('nach-antwort');
          if (wartendeNachricht) {
            var naechste = wartendeNachricht; wartendeNachricht = null;
            input.value = naechste; feldHoehe();
            setTimeout(function () { submit('queue'); }, 0);
          }
        }
      });
    }
    // Answer finished while the window is hidden: bulb → happy reaction → "Ask Noki" hint (+ native notification).
    function fertigMelden(text) {
      unread = true; fertigT = Date.now();
      if (host.fertig) host.fertig();
      if (settings.notify) {
        var vorschau = String(text || '').replace(/```[\s\S]*?```/g, ' ').replace(/\s+/g, ' ').trim().split(/(?<=[.!?])\s/)[0] || '';
        call('intelligence_notify', { vorschau: vorschau.slice(0, 90) }).catch(function () {});
      }
    }
    // ---- Two restrained, physical gears bound to Noki throughout visible and background thinking. ----
    var ZAHNRAD = '<svg width="42" height="32" viewBox="0 0 42 32" aria-hidden="true" focusable="false">' +
      '<g transform="translate(14 18) rotate(-8) scale(1 .86)"><g class="z1" fill="#c9ced3"><circle r="7.6"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(45)"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(90)"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(135)"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(180)"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(225)"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(270)"/><rect x="-1.5" y="-11" width="3" height="5" rx=".6" transform="rotate(315)"/><circle r="2.7" fill="#686e75"/></g></g>' +
      '<g transform="translate(29 10) rotate(8) scale(1 .86)"><g class="z2" fill="#747a81"><circle r="5.8"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(45)"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(90)"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(135)"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(180)"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(225)"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(270)"/><rect x="-1.25" y="-8.5" width="2.5" height="4" rx=".5" transform="rotate(315)"/><circle r="2" fill="#d7dadd"/></g></g></svg>';
    var BIRNE = '<svg width="22" height="28" viewBox="0 0 22 28"><path d="M11 2a8 8 0 0 0-4.6 14.5c.9.7 1.4 1.6 1.4 2.6V20h6.4v-.9c0-1 .5-1.9 1.4-2.6A8 8 0 0 0 11 2z" fill="#fff8e1" stroke="#e6dab0" stroke-width="1"/>' +
      '<path d="M8.8 12.5l2.2 2.2 2.2-2.2" fill="none" stroke="#d9c98f" stroke-width="1" stroke-linecap="round"/><rect x="7.6" y="21" width="6.8" height="2.2" rx="1" fill="#c9ccd1"/><rect x="8.4" y="23.8" width="5.2" height="2" rx="1" fill="#b3b7bd"/></svg>';
    function indikatorEl() {
      if (ind) return ind;
      ind = document.createElement('div'); ind.id = 'nokiDenkt';
      ind.innerHTML = '<div class="nd-zahn" aria-hidden="true">' + ZAHNRAD + '</div><div class="nd-birne" aria-hidden="true">' + BIRNE + '</div>' +
        '<button type="button" class="nd-label"><i></i>Ask Noki</button>';
      var label = ind.querySelector('.nd-label');
      ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'click'].forEach(function (n) { label.addEventListener(n, function (e) { e.stopPropagation(); }); });
      label.addEventListener('click', function () { open(); });
      document.body.appendChild(ind); return ind;
    }
    function indikatorAus() { if (ind) { ind.style.display = 'none'; indModus = ''; } if (indRect) { indRect = null; if (!state.opened) { state.rect = null; host.changed(); } } }
    function indikatorTick() {
      // The prop belongs to the character. The character canvas is masked
      // while Noki passes behind another macOS window; a body-level prop must
      // obey that same effective occlusion instead of floating above it.
      var verdeckt = typeof host.occluded === 'function' && host.occluded();
      var modus = (host.hidden() || verdeckt) ? '' : state.busy ? 'denken' : (!state.opened && unread ? (Date.now() - fertigT < 1900 ? 'birne' : 'label') : '');
      if (!modus) { if (indModus || indRect) indikatorAus(); return; }
      var el = indikatorEl();
      if (modus !== indModus) { indModus = modus; el.className = 'm-' + modus + ' art-' + indArt; el.style.display = 'block'; }
      // Bound to Noki: sits above the head and follows every move.
      var k = host.anchor(), s = Math.max(0.75, Math.min(1.25, k.height / 150));
      el.style.transform = 'translate(' + Math.round(k.x + k.height * 0.26) + 'px,' + Math.round(k.y - k.height * 0.7) + 'px) translate(-50%,-100%) scale(' + s.toFixed(3) + ')';
      if (modus === 'label') {
        var r = el.querySelector('.nd-label').getBoundingClientRect(), nr = { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) };
        if (!indRect || indRect.x !== nr.x || indRect.y !== nr.y || indRect.w !== nr.w) { indRect = nr; state.rect = nr; host.changed(); }
      } else if (indRect) { indRect = null; if (!state.opened) state.rect = null; host.changed(); }
    }
    // Visible work area: window bounds minus menu bar, Dock and a safety margin on every edge.
    function flaeche() {
      var s = window.screen || {}, iw = window.innerWidth, ih = window.innerHeight, rand = 12;
      var oben = rand, unten = rand, links = rand, rechts = rand;
      // The native side knows the real visibleFrame (no menu bar, no Dock) – prefer it.
      var a = host.workArea && host.workArea();
      if (a && a.w > 120 && a.h > 120) {
        links = Math.max(rand, a.x + 8); oben = Math.max(rand, a.y + 8);
        rechts = Math.max(rand, iw - (a.x + a.w) + 8); unten = Math.max(rand, ih - (a.y + a.h) + 8);
        return { x: links, y: oben, w: Math.max(120, iw - links - rechts), h: Math.max(120, ih - oben - unten) };
      }
      if (s.height && s.availHeight && s.availHeight < s.height) {
        var at = typeof s.availTop === 'number' ? s.availTop : s.height - s.availHeight;
        oben = Math.max(oben, at + 8); unten = Math.max(unten, (s.height - s.availHeight - at) + 8);
      }
      if (s.width && s.availWidth && s.availWidth < s.width) {
        var al = typeof s.availLeft === 'number' ? s.availLeft : 0;
        links = Math.max(links, al + 8); rechts = Math.max(rechts, (s.width - s.availWidth - al) + 8);
      }
      return { x: links, y: oben, w: Math.max(120, iw - links - rechts), h: Math.max(120, ih - oben - unten) };
    }
    // Beside Noki when there is room, otherwise below, otherwise above. Noki itself is never moved.
    function platzieren(k, w, h, f) {
      var gap = Math.max(14, k.height * .55), mitte = k.x + k.height * .3;
      var yNeben = k.y - h * .45, seite = [{ x: k.x + gap, y: yNeben }, { x: k.x - gap - w, y: yNeben }];
      if (k.x >= f.x + f.w / 2) seite.reverse();
      var kandidaten = seite.concat([{ x: mitte - w / 2, y: k.y + k.height * .4 }, { x: mitte - w / 2, y: k.y - k.height * .3 - h }]);
      for (var i = 0; i < kandidaten.length; i++) {
        var c = kandidaten[i];
        if (c.x >= f.x && c.y >= f.y && c.x + w <= f.x + f.w && c.y + h <= f.y + f.h) return c;
      }
      return kandidaten[0];
    }
    function tick() {
      indikatorTick();
      gesichtSetzen();          // nur bei echtem Wechsel; sonst sofort zurueck
      if (!state.opened || !panel) return;
      if (host.hidden() || (tipUntil && Date.now() > tipUntil)) { close(host.hidden() ? 'versteckt' : 'tip'); return; }
      host.pause();
      var f = flaeche();
      // The panel never grows beyond the visible work area (menu bar, Dock, screen edge).
      if (panel.style.maxHeight !== f.h + 'px') panel.style.maxHeight = f.h + 'px';
      if (panel.style.maxWidth !== f.w + 'px') panel.style.maxWidth = f.w + 'px';
      var k = host.anchor(), w = panel.offsetWidth, h = panel.offsetHeight;
      var pos = state.userPos ? state.userPos : platzieren(k, w, h, f), old = state.rect;
      // Sticky against Noki's micro-movement – but the result is clamped again below.
      if (!state.userPos && old && Math.abs(old.x - pos.x) < k.height * .6 && Math.abs(old.y - pos.y) < k.height * .6) pos = { x: old.x, y: old.y };
      var x = Math.round(Math.max(f.x, Math.min(f.x + Math.max(0, f.w - w), pos.x)));
      var y = Math.round(Math.max(f.y, Math.min(f.y + Math.max(0, f.h - h), pos.y)));
      state.rect = { x: x, y: y, w: w, h: h };
      panel.style.transform = 'translate(' + x + 'px,' + y + 'px)';
      if (!old || old.w !== w || old.h !== h || old.x !== x || old.y !== y) host.changed();
      if (Date.now() - dragAt > 250) { dragAt = Date.now(); host.hold(true); }
    }
    function saveSetting(key, value) {
      if (!ready || saving || !Object.prototype.hasOwnProperty.call(settings, key)) return;
      var next = Object.assign({}, settings); next[key] = value;
      saving = true; call('intelligence_save', { settings: next }).then(function (s) {
        settings = s;
        state.settings = settings; context = null;
        if (key === 'ask' && !settings.ask) close('ask-disabled');
      }).catch(function (e) { state.error = String(e); }).finally(function () { saving = false; refresh(); setTimeout(refreshStatus, 300); });
    }
    function action(a, v) {
      if (a === 'ni-engine') {
        state.activeEngine = v;
        call('intelligence_engine_set_mode', { mode: v }).then(function () {
          refreshStatus();
        }).catch(function () {});
        refresh();
      }
      else if (a === 'ni-toggle-provider') {
        var parts = v.split(':');
        call('intelligence_engine_toggle_provider', { id: parts[0], enabled: parts[1] === 'true' }).then(function () {
          refreshStatus();
        }).catch(function () {});
      }
      else if (a === 'ni-toggle-model') {
        var modelParts = v.split(':');
        call('intelligence_engine_toggle_model', { id: modelParts[0], enabled: modelParts[1] === 'true' }).then(function () {
          refreshStatus();
        }).catch(function () {});
      }
      else if (a === 'ni-reapprove-model') {
        call('intelligence_renew_attestation', { modelId: v }).then(function () {
          refreshStatus();
        }).catch(function (e) {
          state.error = String(e);
          refresh();
        });
      }
      else if (a === 'ni-rolle') {
        var t = String(v), i = t.indexOf('=');
        call('noki_modell_rolle_setzen', { rolle: t.slice(0, i), id: t.slice(i + 1) || null }).then(function (r) { state.rollen = r || state.rollen; })
          .catch(function (e) { state.error = String(e); }).finally(refresh);
      }
      else if (a === 'ni-level') saveSetting('level', v);
      else if (a === 'ni-mode') setModus(v);
      else if (a === 'ni-unload-min') saveSetting('unload_min', Number(v));
      else if (a === 'ni-mem-open') { memOpen = !memOpen; confirmClear = false; if (memOpen) loadMem(); else refresh(); }
      else if (a === 'ni-mem-del') call('intelligence_memory_delete', { id: Number(v) }).then(loadMem).catch(function (e) { state.error = String(e); refresh(); });
      else if (a === 'ni-mem-clear') { confirmClear = true; refresh(); }
      else if (a === 'ni-mem-clear-yes') call('intelligence_memory_clear').then(function () { confirmClear = false; memList = []; if (state.status) state.status.memory_count = 0; }).catch(function (e) { state.error = String(e); }).finally(refresh);
      else if (a === 'ni-unload' || a === 'ni-load') {
        if (modelActionPending) return;
        modelActionPending = a === 'ni-load' ? 'load' : 'unload';
        state.error = null;
        refresh();
        call(a === 'ni-load' ? 'intelligence_load' : 'intelligence_unload').then(function (r) {
          if (state.status && r) Object.keys(r).forEach(function (k) { state.status[k] = r[k]; });
        }).catch(function (e) {
          state.error = String(e);
        }).finally(function () {
          modelActionPending = null;
          refreshStatus().finally(refresh);
        });
      }
    }
    function settingsHTML() {
      var esc = host.escape;
      var s = state.status;
      var levels = [['off','Aus'],['reserved','Zurückhaltend'],['normal','Normal'],['active','Aktiv']];
      var off = !ready || saving;
      function sw(key, label, disabled) { return '<label class="e-check e-schalter-zeile"><input type="checkbox" class="e-schalter" data-ni="' + key + '"' + (settings[key] ? ' checked' : '') + (disabled || off ? ' disabled' : '') + '><span>' + label + '</span></label>'; }
      var loaded = !!(s && s.loaded);
      var unload = [[0,'Nie'],[5,'5 Min'],[10,'10 Min'],[20,'20 Min'],[30,'30 Min']];
      var mc = (s && s.memory_count) || 0;
      var engine = state.activeEngine || 'local';
      // The backend supplies an explicit ready local model before the first
      // execution; completed runtime provenance always takes precedence.
      var rt = (s && s.runtime) || null;
      var rtLaufzeit = rt ? (rt.runtime + (rt.accelerator ? ' · ' + rt.accelerator : '')) : 'Wird ermittelt …';
      var activeRuntimeModel = s && s.active_runtime_model;
      var readyRuntimeModel = s && s.ready_runtime_model;
      var rtModell = modelText(activeRuntimeModel) || (modelText(readyRuntimeModel) ? ('Bereit: ' + modelText(readyRuntimeModel)) : 'Noch nicht ausgeführt');
      var cloudDa = !!(s && s.cloud_configured);

      // EINE Wahrheit: der Rust-Router liefert Ketten, Zustaende und Defaults.
      // Hier wird nichts nachgerechnet, nichts sortiert und keine Reihenfolge
      // fest getextet — aendert sich das Routing in Rust, folgt diese Ansicht.
      var rr = (s && s.router) || null;
      var currentMode = (rr && rr.engine_mode) || (settings && settings.engine_mode) || 'local_and_cloud';
      var isFreeCloud = !(rr ? rr.local_only : currentMode === 'only_local');

      // Die Zustandsnamen kommen unveraendert aus dem Router; hier steht nur
      // die Beschriftung, nie eine zusaetzliche Logik.
      var engineSection = '<div class="e-sektion">' +
        '<div class="e-s-titel">Intelligence Engine</div>' +
        sw('ask', 'Ask Noki') +
        '<div class="e-s-unter">Aus beendet Ask Noki sofort und gibt dessen lokale KI-Ressourcen frei. Code bleibt bei laufender Aufgabe unberührt.</div>' +
        '<div class="e-seg ni-engine-modes" role="group" aria-label="Ausführungsmodus">' +
          '<button class="e-seg-btn ni-engine-choice' + (!isFreeCloud ? ' aktiv' : '') + '" data-e="ni-engine" data-v="only_local" aria-pressed="' + (!isFreeCloud ? 'true' : 'false') + '"><span>Only Local</span></button>' +
          '<button class="e-seg-btn ni-engine-choice' + (isFreeCloud ? ' aktiv' : '') + '" data-e="ni-engine" data-v="local_and_cloud" aria-pressed="' + (isFreeCloud ? 'true' : 'false') + '"><span>Free Cloud</span></button>' +
        '</div>';

      // Anzeigename: der Router bleibt Source of Truth. Hier faellt nur der
      // technische Ballast weg, den die Karte weiter unten ohnehin schon sagt
      // (Providerklammer, "Free", Laufzeit-Zusatz). Die IDs bleiben unberuehrt.
      function modellName(n) {
        return String(n === null || n === undefined ? '' : n)
          .replace(/\s*\((?:OpenRouter|Mistral|Google|Groq|Cloudflare(?:\s+Workers\s+AI)?|Ollama|DeepSeek|Anthropic|OpenAI|lokal|local)\)/gi, '')
          .replace(/\s*(\+|via)\s+Noki\s+Native/gi, '')
          .replace(/\bFree\s+(?:Tier|Endpoint)\b/gi, '')
          .replace(/\bFree\b/gi, '')
          .replace(/\$0(?:\.00)?\b/g, '')
          .replace(/\s{2,}/g, ' ')
          .trim();
      }
      var PROVIDER_NAME = { openrouter: 'OpenRouter', mistral: 'Mistral', google: 'Google', groq: 'Groq', cloudflare: 'Cloudflare', cloudflare_workers_ai: 'Cloudflare' };
      function providerName(key) {
        return PROVIDER_NAME[String(key).toLowerCase()] || (String(key).charAt(0).toUpperCase() + String(key).slice(1));
      }
      var REASON = {
        rate_limited: 'Rate limit erreicht', quota_exhausted: 'Kontingent aufgebraucht',
        temporarily_unavailable: 'Vorübergehend nicht verfügbar', credentials_required: 'Nicht verbunden',
        cost_uncertain: 'Freigabe abgelaufen', disabled: 'Deaktiviert',
        model_removed: 'Nicht verfügbar (Free-Endpunkt entfernt)'
      };
      function istVerfuegbar(st) { return st === 'available'; }
      function grundText(st) { return istVerfuegbar(st) ? '' : (REASON[st] || 'Nicht verfügbar'); }
      function kette(list) {
        if (!list || !list.length) return '';
        return list
          .filter(function (n) { return String(n || '').toLowerCase().indexOf('deepseek') === -1; })
          .map(function (n) { return esc(modellName(n)); })
          .join(' → ');
      }
      function routingUebersicht(titel, chains, keys) {
        var seen = {};
        var list = [];
        keys.forEach(function (key) {
          (chains && chains[key] || []).forEach(function (name) {
            var identity = String(name || '');
            if (identity && identity.toLowerCase().indexOf('deepseek') === -1 && !seen[identity]) {
              seen[identity] = true;
              list.push(name);
            }
          });
        });
        return '<div class="ni-block-titel">' + esc(titel) + '</div>' +
          '<div class="ni-engine-box ni-kette-box"><div class="ni-kette-modelle">' + kette(list) + '</div></div>';
      }
      // One quiet geometry for every active cloud model. A full baseline is
      // explicitly a local reset-window observation, never provider evidence.
      function quotaBars(p) {
        var quota = p.quota_telemetry;
        var live = quota && quota.source === 'provider_live';
        var pairs = live && [
          [quota.request_remaining, quota.request_limit],
          [quota.token_remaining, quota.token_limit],
          [quota.neuron_remaining, quota.neuron_limit]
        ];
        var pair = pairs && pairs.find(function (value) {
          return typeof value[0] === 'number' && typeof value[1] === 'number' && value[1] > 0 && value[0] >= 0;
        });
        var percent = pair ? Math.max(0, Math.min(100, (pair[0] / pair[1]) * 100)) : 100;
        function compactReset(value) {
          return String(value || '').trim().replace(/^in\s+/i, '')
            .replace(/(\d+)\s+hours?\b/gi, '$1h')
            .replace(/(\d+)\s+minutes?\b/gi, '$1m')
            .replace(/(\d+)\s+seconds?\b/gi, '$1s');
        }
        // Fallbacks are documented provider windows, not synthetic quota data.
        // A response/header reset above always wins, and the selected primary
        // metric remains requests before tokens (matching the displayed bar).
        function resetFallback(provider) {
          switch (String(provider || '')) {
            case 'cloudflare_workers_ai': return '00:00 UTC';
            case 'google': return '00:00 PT';
            case 'groq': return 'live';
            case 'mistral': return 'Window 1s / 1m';
            case 'openrouter': return 'daily';
            default: return '—';
          }
        }
        var reset = quota && quota.reset_at ? compactReset(quota.reset_at) : '';
        if (!reset && p.reset_hint) reset = compactReset(p.reset_hint);
        if (!reset) reset = resetFallback(p.provider);
        var source = pair ? 'provider_live' : 'locally_observed_baseline';
        return '<div class="ni-quota-row" data-quota-source="' + source + '">' +
          '<span class="ni-quota-percent">' + percent.toFixed(0) + '%</span>' +
          '<div class="ni-quota-bar" role="progressbar" aria-label="Quota ' + percent.toFixed(0) + '%" aria-valuemin="0" aria-valuemax="100" aria-valuenow="' + percent.toFixed(0) + '"><span style="width:' + percent.toFixed(2) + '%"></span></div>' +
          '<span class="ni-quota-reset">' + esc(reset.indexOf('Window ') === 0 ? reset : 'Reset ' + reset) + '</span>' +
        '</div>';
      }

      // Ein unvollstaendiger oder fehlender Routerstatus ist ein normaler
      // Zustand (App gerade gestartet, Backend-Aufruf fehlgeschlagen) und darf
      // hoechstens diesen Abschnitt kosten - nie die Settings-Seite und schon
      // gar nicht die App.
      try {
      if (rr) {
        // 2. Aktuelles Routing — direkt aus dem Router abgeleitet.
        var def = rr.current_defaults || {};
        var workDef = modellName(def.work);
        var codingDef = modellName(def.coding);
        if (workDef.toLowerCase().indexOf('deepseek') !== -1) workDef = 'Qwen 3.5 9B';
        if (codingDef.toLowerCase().indexOf('deepseek') !== -1) codingDef = 'GLM-4.7-Flash';
        engineSection += '<div class="ni-engine-box" style="margin-top:8px;">' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">Work Default</span>' +
            '<span class="ni-engine-val">' + esc(workDef || '–') + '</span></div>' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">Coding Default</span>' +
            '<span class="ni-engine-val">' + esc(codingDef || '–') + '</span></div>' +
        '</div>';

        // Eine kompakte, aktuelle Zusammenfassung statt die internen
        // Task-Klassen mehrfach auszustellen. Die Klasse selbst bleibt im
        // Router aktiv; die Reihenfolge kommt weiterhin ausschließlich dorther.
        engineSection += routingUebersicht('Work Routing', rr.work_chains, ['NORMAL']);
        engineSection += routingUebersicht('Coding Routing', rr.coding_chains, ['NORMAL', 'LOCAL']);

        // 3./4. Provider-Karten mit exakt den Router-Zustaenden.
        var provs = rr.providers || [];
        var lokale = provs.filter(function (p) { return p.local; });
        var cloud = provs.filter(function (p) {
          var role = String(p.status_role || '');
          var isDeepSeek = p.id === 'openrouter_deepseek_v4_flash' ||
            String(p.model || '').toLowerCase().indexOf('deepseek') !== -1 ||
            String(p.display_name || '').toLowerCase().indexOf('deepseek') !== -1;
          return !p.local && role.indexOf('candidate') !== 0 && role !== 'parked' && p.state !== 'model_removed' && !isDeepSeek;
        });

        engineSection += '<div class="ni-block-titel">Lokale Modelle</div>';
        // Rollen der lokalen Modelle (noki-rollen.json, sonst Standard).
        // Nur die fuenf Rollen und die installierten Presets - keine Kandidaten.
        if (!state.rollen && !state.rollenLaedt) {
          state.rollenLaedt = true;
          call('noki_modell_rollen').then(function (r) { state.rollen = r || null; }).catch(function () {}).finally(function () { state.rollenLaedt = false; refresh(); });
        }
        if (state.rollen && state.rollen.rollen) {
          var gb = function (b) { return b ? (b / 1e9).toFixed(1).replace('.', ',') + ' GB' : ''; };
          var presets = state.rollen.presets || [];
          engineSection += '<div class="ni-engine-box ni-modell-box ni-rollen">' + state.rollen.rollen.map(function (r) {
            var zusatz = [r.quant, gb(r.bytes), r.installiert ? '' : 'nicht installiert'].filter(Boolean).join(' · ');
            var wahl = '<select class="ni-rolle-wahl" data-ni-rolle="' + esc(r.rolle) + '" aria-label="Modell für ' + esc(r.titel) + '">'
              + '<option value=""' + (r.standard ? ' selected' : '') + '>Standard</option>'
              + presets.filter(function (p) { return p.installiert; }).map(function (p) {
                  return '<option value="' + esc(p.id) + '"' + (!r.standard && p.id === r.id ? ' selected' : '') + '>' + esc(p.anzeige) + '</option>';
                }).join('') + '</select>';
            return '<div class="ni-modell ni-rolle"><span class="ni-rolle-titel">' + esc(r.titel) + '</span>'
              + '<span class="ni-modell-name">' + esc(r.anzeige) + '</span>' + wahl + '</div>'
              + (zusatz || r.eingetragen_fehlt ? '<div class="ni-modell-zusatz">' + esc(zusatz) + (r.eingetragen_fehlt ? ' · gewähltes Modell fehlt, Standard aktiv' : '') + '</div>' : '');
          }).join('') + '</div>'
            + (state.rollen.quelle === 'benchmark' && state.rollen.stand ? '<div class="e-s-unter">Ausgewählt per lokalem Benchmark vom ' + esc(state.rollen.stand) + '.</div>' : '');
        } else engineSection += '<div class="ni-engine-box ni-modell-box">' + lokale.map(function (p) {
          var grund = grundText(p.state);
          return '<div class="ni-modell">' +
              '<span class="ni-modell-name">' + esc(modellName(p.display_name)) +
                (p.local_quantization ? ' · ' + esc(compactQuantization(p.local_quantization)) : '') + '</span>' +
            '</div>' +
            (grund ? '<div class="ni-modell-zusatz">' + esc(grund) + '</div>' : '');
        }).join('') + '</div>';

        if (isFreeCloud) {
          engineSection += '<div class="ni-block-titel">Cloud-Provider</div>';
          var proProvider = {};
          cloud.forEach(function (p) { (proProvider[p.provider] = proProvider[p.provider] || []).push(p); });

          Object.keys(proProvider).forEach(function (key) {
            var models = proProvider[key];

            engineSection += '<div class="ni-engine-box ni-provider">' +
              '<div class="ni-provider-kopf">' +
                '<span class="ni-provider-name">' + esc(providerName(key)) + '</span>' +
              '</div>';

            models.forEach(function (p) {
              var zusatz = [];
              if (p.state === 'cost_uncertain') {
                zusatz.push(
                  esc(grundText(p.state)) +
                  ' <button class="e-btn-sekundaer ni-reapprove-btn" data-e="ni-reapprove-model" data-v="' + esc(p.id) + '" style="margin-left:6px;padding:2px 8px;font-size:11px;font-weight:600;line-height:1.4;border-radius:4px;cursor:pointer;">Erneut freigeben</button>'
                );
              } else if (!istVerfuegbar(p.state)) {
                zusatz.push(esc(grundText(p.state)));
              }
              var bars = quotaBars(p);
              var deaktiviert = p.state === 'disabled';

              engineSection += '<div class="ni-modell">' +
                  '<span class="ni-modell-name">' + esc(modellName(p.display_name)) +
                    (p.specialist_only ? '<span class="ni-modell-hinweis">Limited quota</span>' : '') + '</span>' +
                  '<span class="ni-modell-rechts"><input class="e-schalter" type="checkbox" role="switch"' + (deaktiviert ? '' : ' checked') + ' aria-checked="' + (deaktiviert ? 'false' : 'true') + '"' +
                    ' aria-label="' + esc(modellName(p.display_name)) + '" title="' + (deaktiviert ? 'Modell aktivieren' : 'Modell deaktivieren') + '"' +
                    ' data-e="ni-toggle-model" data-v="' + esc(p.id) + ':' + (deaktiviert ? 'true' : 'false') + '"></span>' +
                '</div>' +
                (zusatz.length ? '<div class="ni-modell-zusatz">' + zusatz.join(' · ') + '</div>' : '') + bars;
            });
            engineSection += '</div>';
          });

        }
      } else {
        engineSection += '<div class="e-s-unter" style="margin-top:8px;">Routerstatus wird geladen …</div>';
      }
      } catch (e) {
        engineSection += '<div class="e-s-unter" style="margin-top:8px;">Routerstatus derzeit nicht darstellbar.</div>';
      }

      engineSection += '<div class="ni-block-titel">Laufzeit auf diesem Mac</div>' +
        '<div class="ni-engine-box">' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">Standort</span><span class="ni-engine-val">Auf diesem Mac</span></div>' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">Runtime</span><span class="ni-engine-val">' + esc(rtLaufzeit) + '</span></div>' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">Aktives Modell</span><span class="ni-engine-val">' + esc(rtModell) + '</span></div>' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">Status</span><span class="ni-engine-val">' + (loaded ? 'Geladen im VRAM' : 'Bereit (entladen)') + '</span></div>' +
          '<div class="ni-engine-row"><span class="ni-engine-lbl">VRAM / RAM</span><span class="ni-engine-val">' + (s ? (s.ram_gb || '?') + ' GB RAM verfügbar' : 'Wird ermittelt…') + '</span></div>' +
        '</div>' +
        '<div style="margin-top:4px;"><button class="e-btn-sekundaer" data-e="' + (loaded ? 'ni-unload' : 'ni-load') + '"' + (modelActionPending ? ' disabled' : '') + '>' +
          (modelActionPending === 'load' ? 'Wird geladen …' : modelActionPending === 'unload' ? 'Wird entladen …' : loaded ? 'Modell entladen' : 'Modell laden') + '</button></div>' +
        '<div class="e-s-unter" style="margin-top:8px;">Automatisch entladen nach Inaktivität</div>' +
        '<div class="e-seg">' + unload.map(function (u) { return '<button class="e-seg-btn' + (settings.unload_min === u[0] ? ' aktiv' : '') + '" data-e="ni-unload-min" data-v="' + u[0] + '"' + (off ? ' disabled' : '') + '>' + u[1] + '</button>'; }).join('') + '</div>' +
        '<p class="e-s-unter">Modelle werden nur bei Bedarf geladen und nach der Ruhezeit automatisch freigegeben. Work und Code teilen sich nie gleichzeitig VRAM.</p>' +
        '</div>';

      var contextSection = '<div class="e-sektion">' +
        '<div class="e-s-titel">Kontext & Privatsphäre</div>' +
        '<div class="e-s-unter">Freigabe für situative Noki-Hinweise (bleibt nur kurz im RAM, keine Historie):</div>' +
        sw('patterns', 'Arbeitsmuster lokal merken · nur für Feedback') +
        sw('window_title', 'Fenstertitel berücksichtigen · benötigt Bedienungshilfen') +
        '</div>';

      var memSection = '<div class="e-sektion">' +
        '<div class="e-s-titel">Memory</div>' +
        sw('memory', 'Langzeit-Memory aktivieren') +
        sw('memory_auto', 'Arbeitspräferenzen automatisch lernen', !settings.memory) +
        '<div class="e-s-unter ni-mem-count">Gespeichert: ' + mc + (mc === 1 ? ' Erinnerung' : ' Erinnerungen') + '</div>' +
        '<div style="display:flex; gap:8px; margin-top:4px;">' +
          '<button class="e-btn-sekundaer" data-e="ni-mem-open">' + (memOpen ? 'Memory schließen' : 'Memory anzeigen') + '</button>' +
          (mc ? (confirmClear ? '<button class="e-btn-sekundaer" data-e="ni-mem-clear-yes" style="color:#ff8888;">Wirklich alles leeren</button>' : '<button class="e-btn-sekundaer" data-e="ni-mem-clear">Memory leeren</button>') : '') +
        '</div>' +
        (memOpen ? '<ul class="ni-mem">' + (memList.length ? memList.map(function (e) { return '<li><span><b style="font-weight:600;">' + esc(KIND[e.kind] || e.kind) + ':</b> ' + esc(e.text) + '</span><button class="e-btn-sekundaer" data-e="ni-mem-del" data-v="' + e.id + '" style="flex-shrink:0;">Entfernen</button></li>'; }).join('') : '<li class="e-s-unter">Keine Einträge vorhanden.</li>') + '</ul>' : '') +
        '<p class="e-s-unter" style="margin-top:6px;">Noki merkt sich nur, was du ausdrücklich sagst („Merk dir …“). Niemals Passwörter, API-Keys oder sensible Daten.</p>' +
        '</div>';

      var tipSection = '<div class="e-sektion">' +
        '<div class="e-s-titel">Proaktive Hinweise</div>' +
        '<div class="e-s-unter">Wie oft Noki unaufgefordert Hinweise gibt (Timer, Arbeitsrhythmus).</div>' +
        '<div class="e-seg">' + levels.map(function (l) { return '<button class="e-seg-btn' + (settings.level === l[0] ? ' aktiv' : '') + '" data-e="ni-level" data-v="' + l[0] + '"' + (off ? ' disabled' : '') + '>' + l[1] + '</button>'; }).join('') + '</div>' +
        '<p class="e-s-unter">Aus = Stille; Zurückhaltend = max. 1×/h; Normal = max. 2×/h; Aktiv = max. 4×/h. Laufende Arbeiten werden nie unterbrochen.</p>' +
        (state.error ? '<p style="color:#ff8888;">' + esc(state.error) + '</p>' : '') +
        '</div>';

      return engineSection + contextSection + memSection + tipSection;
    }
    setInterval(function () {
      if (ready && state.status && (state.status.loaded || assistantMode === 'code') && !state.busy) refreshStatus(); // reflects auto-unload and terminal state
      // Proactive tips never interrupt a conversation or a waiting answer.
      if (!ready || saving || observing || !settings.ask || settings.level === 'off' || state.opened || state.busy || unread || (chat && chat.turns.length) || host.hidden() || host.otherPanel()) return;
      observing = true;
      call('intelligence_observe', { noki: host.context() }).then(function (d) {
        if (d.kind === 'TIP' && settings.level !== 'off' && !state.opened && !host.otherPanel() && !host.hidden()) open(d);
      }).catch(function () {}).finally(function () { observing = false; });
    }, 15000);
    // ^/° + 0: a real toggle. Hiding never destroys the panel – chat, task and context stay.
    function shortcut() {
      if (!settings.ask) return;
      if (state.opened && panel && !panel.classList.contains('ni-tip')) { close('toggle'); return; }
      if (state.opened) close('tip-toggle');
      open();
    }
    state.open = open; state.close = close; state.shortcut = shortcut; state.toggleWarp = shortcut; state.chatSchliessen = chatSchliessen;
    state.starteNeuenChat = starteNeuenChat;
    state.setThinking = function (an) {
      state.busy = !!an;
      if (an) { indArt = 'zahnrad'; }
      indikatorTick();
    };
    state.setFertig = function () {
      state.busy = false;
      unread = true;
      fertigT = Date.now();
      indikatorTick();
    };
    state.alsGelesen = function () {
      unread = false;
      fertigT = 0;
      indikatorAus();
    };
    window.toggleWarp = shortcut;
    state.verlaufAnzahl = function () { return chats.length; };
    state.chat = function () { return chat ? { titel: chat.titel, turns: chat.turns.length } : null; };
    // Test/debug accessor: route, web usage, source count and stage timings of the last answer.
    state.lastTurn = function () {
      var t = chat && chat.turns.length ? chat.turns[chat.turns.length - 1] : null;
      return t ? { q: t.q || '', text: t.text || '', route: t.route, plan: t.plan || null, web: !!t.web, sources: (t.sources || []).length,
        source_titles: (t.sources || []).map(function (s) { return s.title; }), research: t.research || null,
        timings: t.timings || null, fehler: !!t.fehler,
        runtime_model: t.runtime_model || null, finish_reason: t.finish_reason || null,
        output_word_count: t.output_word_count == null ? null : t.output_word_count,
        tool: t.tool ? { name: t.tool.name, path: t.tool.path || null, confirm: !!t.tool.confirm, auto_execute: !!t.tool.auto_execute } : null } : null;
    };
    state.hinweis = function () { return indModus ? indModus + ':' + indArt : ''; };
    state.task = function () { return taskState; };
    // Test accessor (read only): the clamped panel position for a given Noki anchor.
    state.platzTest = function (k, w, h, f) {
      f = f || flaeche(); var pos = platzieren(k, w, h, f);
      return { x: Math.max(f.x, Math.min(f.x + Math.max(0, f.w - w), pos.x)), y: Math.max(f.y, Math.min(f.y + Math.max(0, f.h - h), pos.y)), f: f };
    };
    state.mic = function () { return micZustand; };
    state.voiceStart = micStart; state.voiceStop = micStop; state.voiceSenden = micSenden; state.voiceAbbrechen = micAbbrechen;
    state.voiceLayout = function () { return { recording: !!(panel && panel.classList.contains('ni-recording')), composerDisabled: !!(input && input.disabled) }; };
    // Reads the three layers separately: VoiceDraft state, computed display, real DOM.
    state.voiceRender = function () {
      return { state_full: (rec.basis || '') + micGesamt(),
               display_full: (rec.basis || '') + micGesamt(),
               dom_full: voiceText ? (((voiceBasis && voiceBasis.textContent) || '') + (voiceText.textContent || '') + ((voiceInterim && voiceInterim.textContent) || '')) : null,
               dom_regressions: VOICE_DEBUG.filter(function (d) { return d.e === 'dom_regression'; }).length,
               blank_frames: rec.blankFrames, word_regressions: rec.wortRegress, snapshot: rec.sichtSnapshot,
               blocked_writes: rec.blockiert || 0 };
    };
    // Metric: does what the user saw match what was sent?
    state.voiceDeckung = function () {
      var w = ask => String(ask || '').toLowerCase().replace(/[^\p{L}\p{N} ]/gu, ' ').split(/\s+/).filter(Boolean);
      var e = VOICE_DEBUG.filter(function (d) { return d.e === 'composer_write'; }).slice(-1)[0];
      if (!e) return null;
      var sicht = w(e.sichtbar), sent = w(e.gesendet), rest = sent.slice(), hit = 0;
      sicht.forEach(function (x) { var i = rest.indexOf(x); if (i >= 0) { rest = rest.slice(i + 1); hit++; } });
      return { sichtbar: sicht.length, gesendet: sent.length, coverage: sicht.length ? hit / sicht.length : 1 };
    };
    state.voiceDebug = function () { return VOICE_DEBUG.slice(); };
    state.voice = function () { return { active: rec.active, committed: micCommittedText(), interim: rec.interim, text: micGesamt(), segmente: rec.timeline ? rec.timeline.length : rec.committed.length, audio: rec.audio || null, overlaps: rec.overlaps || 0, replacements: rec.replacements || 0 }; }; state.tick = tick; state.saveSetting = saveSetting; state.action = action; state.settingsHTML = settingsHTML;
    state.context = function () { return context; };
    // Ein FERTIGER Satz von aussen (Stimme) nimmt exakt denselben Weg wie
    // ein getippter: dasselbe Feld, dasselbe submit, dieselbe Absicht,
    // dieselben Werkzeuge. Kein zweiter Befehlsweg.
    state.frage = function (text, opt) {
      var t = String(text || '').trim();
      if (!t) return false;
      // Die Verarbeitung braucht das Eingabefeld - das aber erst beim
      // ersten Oeffnen gebaut wurde. Genau daran endete der stille Weg der
      // Stimme: ohne `open()` gab es kein `input`, und `frage` kehrte vor
      // `submit` zurueck. Ohne Fehler, ohne Antwort.
      //
      // `ensure()` BAUT nur (das Panel traegt `display: none`); gezeigt
      // wird es allein in `open()`. Aufbau und Auftritt sind damit
      // getrennt: die Stimme bekommt die Maschinerie, Ask Noki die Buehne.
      ensure();
      if (!state.opened && !(opt && opt.still)) open();
      if (!input) return false;
      input.value = t;
      feldHoehe();
      submit((opt && opt.quelle) || 'stimme');
      return true;
    };
    return state;
  } };
})();
