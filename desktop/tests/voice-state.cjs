// Recording state of Ask Noki, driven exactly like the native helper drives it.
// No browser needed: the reducer (committedSegments + currentInterimSegment) is tested directly.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

const handlers = {};
const calls = [];
const el = () => ({ style: {}, dataset: {}, classList: { toggle() {}, add() {}, remove() {}, contains: () => false },
  children: [], appendChild() {}, replaceChildren() {}, querySelector: () => null, setAttribute() {}, textContent: '', hidden: false });
global.window = { __TAURI__: { event: { listen: (n, f) => { handlers[n] = f; } } }, innerWidth: 1280, innerHeight: 850, screen: {} };
global.document = { addEventListener() {}, createElement: el, body: { appendChild() {} }, querySelector: () => null };
global.setInterval = () => 0;
global.requestAnimationFrame = () => 0;

require('node:vm').runInThisContext(fs.readFileSync(path.join(__dirname, '..', 'intelligence.js'), 'utf8'));
const ask = window.NokiIntelligence.create({
  invoke: (name, args) => { calls.push({ name, args }); return Promise.resolve(null); },
  settingsChanged() {}, openSettings() {}, closePanels() {}, thinking() {}, phase() {}, fertig() {}, explain() {},
  otherPanel: () => false, hidden: () => false, changed() {}, hold() {}, front() {}, back() {}, pause() {},
  anchor: () => ({ x: 100, y: 100, height: 150 }), context: () => ({}), tool() {},
});
const voice = handlers['intelligence-voice'];
assert(voice, 'voice listener registered');
const send = p => voice({ payload: p });

// ---- Test A/B: long recording with pauses and recognizer restarts -----------
assert.equal(ask.mic(), 'idle', 'starts idle');
send({ state: 'listening' });
assert.equal(ask.mic(), 'listening');

const S1 = 'Ich möchte wissen wie viel';
const S12 = 'Ich möchte wissen wie viel der Schuh kostet';
send({ gen: 1, seq: 1, committed: '', interim: S1, recording: true });
assert.equal(ask.voice().text, S1, 'interim is visible immediately');
send({ gen: 1, seq: 2, committed: S1, interim: '', recording: true });          // segment final
assert.equal(ask.voice().committed, S1, 'final appends into committedSegments');

// 2 s pause, then a NEW recognizer generation that overlaps at the segment boundary (§7)
send({ gen: 2, seq: 3, committed: S1, interim: 'wie viel der Schuh kostet', recording: true });
assert.equal(ask.voice().text, S12, 'overlap is deduplicated, nothing is deleted');
send({ gen: 2, seq: 4, committed: S12, interim: '', recording: true });
assert.equal(ask.voice().committed, S12);

// 15 s pause: the helper restarts and reports ONLY the new segment (worst case, §5).
send({ gen: 3, seq: 5, committed: '', interim: 'und wann kommt er an', recording: true });
assert.equal(ask.voice().text, S12 + ' und wann kommt er an', 'restart never resets the transcript');
send({ gen: 3, seq: 6, committed: 'und wann kommt er an', interim: '', recording: true });
const voll = S12 + ' und wann kommt er an';
assert.equal(ask.voice().committed, voll);

// A late result of an older generation must not overwrite newer text (§6).
send({ gen: 2, seq: 4, committed: S12, interim: '', recording: true });
assert.equal(ask.voice().committed, voll, 'stale generation ignored');
assert.equal(ask.mic(), 'listening', 'recording never ends by itself');
assert.equal(calls.filter(c => c.name === 'intelligence_voice_stop').length, 0, 'no automatic stop');

// ---- A recognizer final in the middle of a session must NOT end it (§K–§O) ---
send({ gen: 4, seq: 7, committed: voll, interim: '', final: true, recording: true });
assert.equal(ask.mic(), 'listening', 'a recognizer final never ends the UserRecordingSession');
assert.equal(ask.voice().committed, voll, 'final appends, it never replaces');
send({ gen: 5, seq: 8, committed: '', interim: 'und noch etwas', recording: true });
assert.equal(ask.voice().text, voll + ' und noch etwas', 'recording continues after the final');
send({ gen: 5, seq: 9, committed: 'und noch etwas', interim: '', recording: true });
const ganz = voll + ' und noch etwas';
assert.equal(ask.voice().committed, ganz);
assert.equal(calls.filter(c => c.name === 'intelligence_chat').length, 0, 'no auto-send');

const tick = () => new Promise(r => setImmediate(r));
(async () => {
  // ---- Senden ----------------------------------------------------------------
  ask.voiceSenden();
  await tick();
  assert(calls.some(c => c.name === 'intelligence_voice_stop' && c.args.cancel === false));
  send({ state: 'processing' });
  send({ text: ganz, committed: ganz, interim: '', final: true });
  assert.equal(ask.mic(), 'idle');
  assert.equal(ask.voice().text, '', 'state is cleaned up after the recording');
  // §S: committed_len must never shrink without Senden/Abbrechen.
  const dbg = ask.voiceDebug().filter(d => d.committed_len_after !== undefined);
  for (const d of dbg) assert(d.committed_len_after >= d.committed_len_before, 'committed_len shrank in ' + d.e);
  assert(ask.voiceDebug().some(d => d.e === 'recognizer_restarted'), 'restart was logged');
  assert(ask.voiceDebug().some(d => d.e === 'final_appended'), 'final append was logged');
  assert(ask.voiceDebug().some(d => d.e === 'send_pressed'));
  // §1/§15: STT never writes into the normal composer during a recording – exactly one
  // transfer, and only after Senden.
  const writes = ask.voiceDebug().filter(d => d.e === 'composer_write');
  assert.equal(writes.length, 1, 'exactly one composer transfer per VoiceDraft');
  assert.equal(writes[0].gewollt, true, 'the transfer only happens on a user stop');
  assert.equal(ask.voiceDebug().filter(d => d.e === 'display_regression').length, 0, 'visible committed text never shrank');
  // §27: what the user saw must be what got sent.
  const deck = ask.voiceDeckung();
  assert(deck && deck.coverage >= 0.999, 'visible_vs_sent_coverage = ' + JSON.stringify(deck));

  // ---- Duplication check (§7) ------------------------------------------------
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: 'Ich möchte wissen wie viel', interim: '', recording: true });
  send({ gen: 2, seq: 2, committed: '', interim: 'wie viel der Schuh kostet', recording: true });
  assert.equal(ask.voice().text, S12, 'no duplicated words, no deleted words');
  send({ state: 'idle' });


  // ---- §15: cumulative Apple snapshots must not appear twice --------------------
  send({ state: 'listening' });
  const kum = ['Ich möchte', 'Ich möchte wissen', 'Ich möchte wissen wie viel',
               'Ich möchte wissen wie viel die Air Max', 'Ich möchte wissen wie viel die Air Max 95'];
  kum.forEach((t, i) => send({ gen: 1, seq: i + 1, committed: '', interim: t, recording: true }));
  assert.equal(ask.voice().text, kum[kum.length - 1], 'cumulative snapshots replace, they never stack');
  send({ gen: 1, seq: 9, committed: kum[kum.length - 1], interim: '', recording: true });   // commit
  // New cycle whose head repeats already committed audio.
  send({ gen: 2, seq: 10, committed: '', interim: 'die Air Max 95 aktuell', recording: true });
  send({ gen: 2, seq: 11, committed: '', interim: 'die Air Max 95 aktuell kosten', recording: true });
  const ziel = 'Ich möchte wissen wie viel die Air Max 95 aktuell kosten';
  assert.equal(ask.voice().text, ziel, 'overlap is resolved exactly once');
  const woerter = ask.voice().text.split(' ');
  for (let i = 0; i + 3 < woerter.length; i++) {
    const drei = woerter.slice(i, i + 3).join(' ');
    assert.equal(ask.voice().text.split(drei).length - 1, 1, 'no word group appears twice: ' + drei);
  }
  ask.voiceAbbrechen();
  await tick();

  // ---- Stop / continue: pausing keeps the full VoiceDraft ---------------------
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: 'Erster Satz bleibt sichtbar', interim: '', recording: true });
  ask.voiceStop();
  assert.equal(ask.mic(), 'processing', 'Stop waits for the native final transcript');
  assert(calls.some(c => c.name === 'intelligence_voice_stop' && c.args.cancel === false));
  send({ gen: 1, seq: 2, committed: 'Erster Satz bleibt sichtbar', interim: '', final: true });
  send({ state: 'idle' });
  assert.equal(ask.mic(), 'paused', 'Stop pauses instead of sending or discarding');
  assert.equal(ask.voice().text, 'Erster Satz bleibt sichtbar', 'paused transcript remains complete');
  const chatsVorFortsetzen = calls.filter(c => c.name === 'intelligence_chat').length;
  ask.voiceStart();
  await tick();
  assert.equal(ask.mic(), 'processing', 'continue starts a fresh local recorder');
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: '', interim: 'Zweiter Satz kommt dazu', recording: true });
  assert.equal(ask.voice().text, 'Erster Satz bleibt sichtbar Zweiter Satz kommt dazu', 'continued speech appends to the retained transcript');
  assert.equal(calls.filter(c => c.name === 'intelligence_chat').length, chatsVorFortsetzen, 'Stop/continue never sends automatically');
  ask.voiceAbbrechen();
  await tick();
  assert.equal(ask.mic(), 'idle', 'Cancel leaves the resumed audio session');
  assert.equal(ask.voice().text, '', 'Cancel clears only the audio draft');

  // ---- §16: a real user repetition must survive -------------------------------
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: 'Das ist sehr', interim: '', recording: true });
  send({ gen: 2, seq: 2, committed: '', interim: 'sehr wichtig', recording: true });
  assert.equal(ask.voice().text, 'Das ist sehr sehr wichtig', 'a single shared word is never deduplicated');
  ask.voiceAbbrechen();
  await tick();

  // ---- §13: the helper's composed view wins over any local stitching -----------
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: 'Wie viel kosten die Schuhe', interim: 'aktuell',
         display: 'Wie viel kosten die Schuhe aktuell', recording: true });
  assert.equal(ask.voice().text, 'Wie viel kosten die Schuhe aktuell');
  ask.voiceAbbrechen();
  await tick();

  // ---- Render source: display must never fall behind the ledger ---------------
  // Reproduces the asymmetry that was found: the helper's composed view lagged the ledger,
  // and the UI rendered the helper's view while Senden used the ledger.
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: 'Das ist der erste Teil', interim: '', display: 'Das ist der erste Teil', recording: true });
  send({ gen: 2, seq: 2, committed: '', interim: 'jetzt der zweite Teil', display: 'jetzt der zweite Teil', recording: true });
  const r = ask.voiceRender();
  assert.equal(r.state_full, 'Das ist der erste Teil jetzt der zweite Teil', 'ledger holds A+B');
  assert.equal(r.display_full, r.state_full, 'the rendered source is the ledger, never the shorter helper view');
  assert.equal(ask.voice().text, 'Das ist der erste Teil jetzt der zweite Teil');
  assert.equal(r.dom_regressions, 0);
  ask.voiceAbbrechen();
  await tick();

  // ---- Pause must never blank the visible transcript (§2/§3/§6/§9) -------------
  send({ state: 'listening' });
  send({ gen: 1, seq: 1, committed: '', interim: 'Das ist Abschnitt A', recording: true });
  send({ gen: 1, seq: 2, committed: 'Das ist Abschnitt A', interim: '', recording: true });   // cycle end
  const nachA = ask.voiceRender();
  assert.equal(nachA.snapshot, 'Das ist Abschnitt A');
  // PAUSE: a new empty cycle must not take A off screen.
  send({ gen: 2, seq: 3, committed: '', interim: '', recording: true });
  assert.equal(ask.voiceRender().snapshot, 'Das ist Abschnitt A', 'pause keeps the visible text');
  send({ gen: 2, seq: 4, committed: '', interim: 'jetzt Abschnitt B', recording: true });
  send({ gen: 2, seq: 5, committed: 'jetzt Abschnitt B', interim: '', recording: true });
  assert.equal(ask.voiceRender().snapshot, 'Das ist Abschnitt A jetzt Abschnitt B');
  send({ gen: 3, seq: 6, committed: '', interim: '', recording: true });                      // longer pause
  send({ gen: 3, seq: 7, committed: 'und Abschnitt C', interim: '', recording: true });
  const r3 = ask.voiceRender();
  assert.equal(r3.snapshot, 'Das ist Abschnitt A jetzt Abschnitt B und Abschnitt C');
  assert.equal(r3.blank_frames, 0, 'no blank frame during recording');
  assert.equal(r3.word_regressions, 0, 'the word counter never falls back');
  assert.equal(r3.dom_regressions, 0);
  ask.voiceAbbrechen();
  await tick();

  // 90-second natural dictation: six speech units, including 5/10/20-second pauses.
  // D arrives first in one callback; its audio interval still places it fourth.
  send({ state: 'listening' });
  const speech = [
    'Hallo Noki ich habe eine Frage und muss ein bisschen ausholen',
    'Es geht um Red Bull Purple und ich habe es früher öfter gesehen',
    'Nach einer Pause schaue ich nun bei Kaufland in das Sortiment',
    'Das ist irgendwie aus der Hölle sage ich mal aber nur nebenbei',
    'Warum ist Red Bull Purple dort gerade nicht verfügbar',
    'Kannst du mir erklären was dazu tatsächlich bekannt ist'
  ];
  const starts = [0, 10, 25, 50, 75, 87];
  const segments = speech.map((text, i) => ({ id: 'unit-' + i, generation: i + 1,
    audio_start: starts[i], audio_end: starts[i] + 4, text, final: true }));
  for (let i = 0; i < segments.length; i++) {
    const snapshot = segments.slice(0, i + 1);
    if (i === 3) snapshot.unshift({ ...segments[3] }); // duplicate callback for one interval
    send({ gen: i + 1, seq: i * 2 + 1, timeline: snapshot, recording: true });
    assert.equal(ask.voice().text, speech.slice(0, i + 1).join(' '), 'visible timeline after pause ' + i);
    send({ gen: i + 1, seq: i * 2 + 2, timeline: snapshot, recording: true });
  }
  const expected = speech.join(' ');
  ask.voiceSenden(); await tick();
  send({ gen: 7, seq: 20, timeline: [segments[5], ...segments.slice(0, 5), segments[5]], final: true });
  const longWrite = ask.voiceDebug().filter(d => d.e === 'composer_write').slice(-1)[0];
  assert.equal(longWrite.gesendet, expected, 'last speech unit cannot prepend or duplicate');
  assert(ask.voiceDeckung().coverage >= 0.99, 'visible and sent transcript agree');

  // ---- §9/§10: the Ask Noki panel always stays fully inside the work area -----
  const flaeche = { x: 12, y: 37, w: 1256, h: 750 };           // menu bar + Dock insets
  for (const k of [{ x: 40, y: 40 }, { x: 640, y: 20 }, { x: 1240, y: 60 }, { x: 60, y: 820 }, { x: 1250, y: 840 }, { x: 640, y: 430 }])
    for (const groesse of [{ w: 420, h: 300 }, { w: 520, h: 600 }, { w: 620, h: 750 }])
      for (const hoehe of [110, 150, 220]) {
        const p = ask.platzTest({ x: k.x, y: k.y, height: hoehe }, groesse.w, groesse.h, flaeche);
        assert(p.x >= flaeche.x && p.y >= flaeche.y, `panel above/left of the work area for ${JSON.stringify(k)}`);
        assert(p.x + groesse.w <= flaeche.x + flaeche.w && p.y + groesse.h <= flaeche.y + flaeche.h,
          `panel outside the work area for ${JSON.stringify(k)} / ${JSON.stringify(groesse)}`);
      }

  console.log('voice-state: OK');

})().catch(e => { console.error(e); process.exit(1); });
