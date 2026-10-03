// Noki Companion - lets the LOCAL Noki app type into exactly this VS Code
// window (active editor or active integrated terminal) while the window sits
// on a Desktop the user is not looking at. macOS/Electron drops key events
// posted to an inactive VS Code; the VS Code API does not need focus.
//
// Transport: one Unix domain socket per window (extension host) in a
// user-only directory (0700, socket 0600). Every request carries a random
// per-session token, published only in a 0600 file next to the socket.
// No TCP, no network, no telemetry; nothing typed is logged. Deactivation
// removes socket and token file. No VS Code setting is changed.
'use strict';
const vscode = require('vscode');
const net = require('net');
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

const DIR = path.join(os.homedir(), 'Library', 'Application Support', 'com.noki.desktop', 'vsc');
let server = null, sockPath = '', infoPath = '', version = '';
let letzteArt = '';            // undo grouping: consecutive text inserts form one undo step

// Undo/redo of bridge edits. VS Code's own `undo` command needs an editor
// with keyboard focus, which a window on another Desktop never has. Each
// bridge edit is recorded with its exact inverse; a step = consecutive
// edits of one kind. Valid only while the document is exactly as the bridge
// left it (version check) - any other change clears the history.
const verlauf = new Map();     // document uri -> { undo: [steps], redo: [steps], version }
function verlaufVon(doc) {
  const k = doc.uri.toString();
  let v = verlauf.get(k);
  if (!v || v.version !== doc.version) { v = { undo: [], redo: [], version: doc.version }; verlauf.set(k, v); }
  return v;
}
async function anwenden(ed, teile) {      // teile: [{s, e, t}] offsets of the CURRENT text
  const doc = ed.document;
  const r = teile.map(p => ({ r: new vscode.Range(doc.positionAt(p.s), doc.positionAt(p.e)), t: p.t }));
  return ed.edit(eb => { for (const p of r) eb.replace(p.r, p.t); }, { undoStopBefore: true, undoStopAfter: true });
}
function auswahlSetzen(ed, offs) {
  const doc = ed.document;
  ed.selections = offs.map(([a, b]) => new vscode.Selection(doc.positionAt(a), doc.positionAt(b)));
}
async function rueckgaengig(ed, wieder) {
  const doc = ed.document, v = verlaufVon(doc);
  const schritt = (wieder ? v.redo : v.undo).pop();
  if (!schritt) return false;
  const ops = wieder ? schritt : [...schritt].reverse();
  for (const op of ops) { if (!await anwenden(ed, wieder ? op.vor : op.zurueck)) { v.undo = []; v.redo = []; return false; } }
  auswahlSetzen(ed, wieder ? schritt[schritt.length - 1].nachher : schritt[0].vorher);
  (wieder ? v.undo : v.redo).push(schritt);
  v.version = doc.version;
  letzteArt = '';
  sichtbar(ed);
  return true;
}

function activate(context) {
  version = (context.extension && context.extension.packageJSON && context.extension.packageJSON.version) || '';
  try {
    fs.mkdirSync(DIR, { recursive: true, mode: 0o700 });
    fs.chmodSync(DIR, 0o700);
  } catch (_) { return; }
  const id = `${process.pid}-${crypto.randomBytes(4).toString('hex')}`;
  sockPath = path.join(DIR, `${id}.sock`);
  infoPath = path.join(DIR, `${id}.json`);
  const token = crypto.randomBytes(24).toString('hex');

  server = net.createServer(conn => {
    let puffer = '';
    conn.setEncoding('utf8');
    conn.on('data', async chunk => {
      puffer += chunk;
      if (puffer.length > 1 << 20) { conn.destroy(); return; }
      let i;
      while ((i = puffer.indexOf('\n')) >= 0) {
        const zeile = puffer.slice(0, i); puffer = puffer.slice(i + 1);
        let m;
        try { m = JSON.parse(zeile); } catch (_) { conn.destroy(); return; }
        if (!m || m.token !== token) { conn.destroy(); return; }
        let antwort;
        try { antwort = await bearbeiten(m); } catch (e) { antwort = { ok: false, grund: String(e && e.message || e) }; }
        conn.write(JSON.stringify(antwort) + '\n');
      }
    });
    conn.on('error', () => {});
  });
  server.on('error', () => {});
  server.listen(sockPath, () => {
    try { fs.chmodSync(sockPath, 0o600); } catch (_) {}
    const info = JSON.stringify({ sock: sockPath, token, pid: process.pid, v: 1, version });
    fs.writeFileSync(infoPath, info, { mode: 0o600 });
  });
  context.subscriptions.push({ dispose: aufraeumen });
}

function aufraeumen() {
  try { if (server) server.close(); } catch (_) {}
  server = null;
  for (const p of [infoPath, sockPath]) { try { if (p) fs.unlinkSync(p); } catch (_) {} }
}

function deactivate() { aufraeumen(); }

// ---------------------------------------------------------------- requests
async function bearbeiten(m) {
  switch (m.op) {
    case 'ident': return ident();
    case 'editor': return editorTaste(m);
    case 'terminal': return terminalTaste(m);
    case 'zustand': return zustand();
    case 'status': return status();
    default: return { ok: false, grund: 'unbekannt' };
  }
}

function ident() {
  const ed = vscode.window.activeTextEditor;
  const tab = vscode.window.tabGroups && vscode.window.tabGroups.activeTabGroup.activeTab;
  return {
    ok: true,
    version,
    workspace: vscode.workspace.name || '',
    datei: ed ? path.basename(ed.document.fileName) : '',
    tab: tab ? tab.label : '',
    terminal: vscode.window.activeTerminal ? vscode.window.activeTerminal.name : '',
    terminals: vscode.window.terminals.length,
    fokus: vscode.window.state.focused,
  };
}

// For Noki's lossless-restart check: number of unsaved documents and the
// shell PIDs of this window's integrated terminals (no content).
async function status() {
  const dirty = vscode.workspace.textDocuments.filter(d => d.isDirty).length;
  const shells = [];
  for (const t of vscode.window.terminals) { try { const p = await t.processId; if (p) shells.push(p); } catch (_) {} }
  return { ok: true, dirty, shells };
}

// Verification state for Noki's acceptance tests: cursor position, dirty
// flag and ONLY the cursor line (never the document).
function zustand() {
  const ed = vscode.window.activeTextEditor;
  if (!ed) return { ok: true, editor: false };
  const s = ed.selection, d = ed.document;
  return { ok: true, editor: true, datei: path.basename(d.fileName), zeile: s.active.line, spalte: s.active.character,
           auswahl: !s.isEmpty, dirty: d.isDirty, version: d.version, zeilentext: d.lineAt(s.active.line).text };
}

// ---------------------------------------------------------------- editor
function eolText(doc, t) { return t.replace(/\r?\n/g, doc.eol === vscode.EndOfLine.CRLF ? '\r\n' : '\n'); }

// Replace ranges[i] by texts[i] in one edit and put one cursor right after
// each inserted text - exactly at the user's real cursor/selection.
async function ersetzen(ed, ranges, texts, gruppe) {
  const doc = ed.document;
  const v = verlaufVon(doc);
  const vorher = ed.selections.map(s => [doc.offsetAt(s.anchor), doc.offsetAt(s.active)]);
  const teile = ranges.map((r, i) => ({ s: doc.offsetAt(r.start), e: doc.offsetAt(r.end), r, t: eolText(doc, texts[i]), alt: doc.getText(r) }))
    .sort((a, b) => a.s - b.s);
  const zusammen = gruppe && letzteArt === gruppe;
  const ok = await ed.edit(eb => { for (const p of teile) eb.replace(p.r, p.t); },
    { undoStopBefore: !zusammen, undoStopAfter: false });
  if (!ok) return false;
  let verschub = 0; const neu = [], zurueck = [];
  for (const p of teile) {
    const start = p.s + verschub;
    zurueck.push({ s: start, e: start + p.t.length, t: p.alt });
    const off = start + p.t.length;
    neu.push(new vscode.Selection(doc.positionAt(off), doc.positionAt(off)));
    verschub += p.t.length - (p.e - p.s);
  }
  ed.selections = neu;
  const op = { vor: teile.map(p => ({ s: p.s, e: p.e, t: p.t })), zurueck, vorher,
               nachher: neu.map(s => [doc.offsetAt(s.anchor), doc.offsetAt(s.active)]) };
  if (zusammen && v.undo.length) v.undo[v.undo.length - 1].push(op); else v.undo.push([op]);
  if (v.undo.length > 200) v.undo.shift();
  v.redo = [];
  v.version = doc.version;
  letzteArt = gruppe || '';
  sichtbar(ed);
  return true;
}

function sichtbar(ed) { ed.revealRange(ed.selection, vscode.TextEditorRevealType.InCenterIfOutsideViewport); }

function zeichenLinks(doc, pos) {
  if (pos.character > 0) {
    const t = doc.lineAt(pos.line).text;
    const n = pos.character >= 2 && /[\uDC00-\uDFFF]/.test(t[pos.character - 1]) ? 2 : 1;
    return new vscode.Range(pos.translate(0, -n), pos);
  }
  if (pos.line === 0) return null;
  return new vscode.Range(doc.lineAt(pos.line - 1).range.end, pos);
}
function zeichenRechts(doc, pos) {
  const z = doc.lineAt(pos.line);
  if (pos.character < z.text.length) {
    const n = /[\uD800-\uDBFF]/.test(z.text[pos.character]) ? 2 : 1;
    return new vscode.Range(pos, pos.translate(0, n));
  }
  if (pos.line >= doc.lineCount - 1) return null;
  return new vscode.Range(pos, new vscode.Position(pos.line + 1, 0));
}

function bewegen(ed, key, shift) {
  const doc = ed.document;
  ed.selections = ed.selections.map(s => {
    let p = s.active;
    if (!shift && !s.isEmpty && (key === 'ArrowLeft' || key === 'ArrowRight')) {
      const z = key === 'ArrowLeft' ? s.start : s.end;
      return new vscode.Selection(z, z);
    }
    if (key === 'ArrowLeft') { const r = zeichenLinks(doc, p); if (r) p = r.start; }
    else if (key === 'ArrowRight') { const r = zeichenRechts(doc, p); if (r) p = r.end; }
    else if (key === 'ArrowUp') p = p.line > 0 ? doc.validatePosition(new vscode.Position(p.line - 1, p.character)) : new vscode.Position(0, 0);
    else if (key === 'ArrowDown') p = p.line < doc.lineCount - 1 ? doc.validatePosition(new vscode.Position(p.line + 1, p.character)) : doc.lineAt(p.line).range.end;
    else if (key === 'Home') p = new vscode.Position(p.line, doc.lineAt(p.line).firstNonWhitespaceCharacterIndex);
    else if (key === 'End') p = doc.lineAt(p.line).range.end;
    return shift ? new vscode.Selection(s.anchor, p) : new vscode.Selection(p, p);
  });
  letzteArt = '';
  sichtbar(ed);
  return true;
}

async function editorTaste(m) {
  const ed = vscode.window.activeTextEditor;
  if (!ed) return { ok: false, grund: 'kein_editor' };
  const doc = ed.document, key = m.key, shift = !!m.shift;
  let ok = true;
  if (key === 'text') {
    ok = await ersetzen(ed, ed.selections.map(s => s), ed.selections.map(() => String(m.text || '')), 'text');
  } else if (key === 'Enter') {
    ok = await ersetzen(ed, ed.selections.map(s => s), ed.selections.map(s => {
      const z = doc.lineAt(s.start.line).text; return '\n' + z.slice(0, doc.lineAt(s.start.line).firstNonWhitespaceCharacterIndex);
    }), 'text');
  } else if (key === 'Tab') {
    const o = ed.options;
    ok = await ersetzen(ed, ed.selections.map(s => s), ed.selections.map(s => o.insertSpaces
      ? ' '.repeat(Number(o.tabSize) - (s.start.character % Number(o.tabSize))) : '\t'), 'text');
  } else if (key === 'Backspace' || key === 'Delete') {
    const r = ed.selections.map(s => s.isEmpty ? (key === 'Backspace' ? zeichenLinks(doc, s.active) : zeichenRechts(doc, s.active)) : s);
    const echt = r.filter(Boolean);
    if (echt.length) ok = await ersetzen(ed, echt, echt.map(() => ''), 'loeschen');
  } else if (/^Arrow|^Home$|^End$/.test(key)) {
    ok = bewegen(ed, key, shift);
  } else if (key === 'Escape') {
    ed.selections = [new vscode.Selection(ed.selection.active, ed.selection.active)];
    letzteArt = '';
  } else if (key === 'selectAll') {
    const ende = doc.lineAt(doc.lineCount - 1).range.end;
    ed.selections = [new vscode.Selection(new vscode.Position(0, 0), ende)];
    letzteArt = '';
  } else if (key === 'copy' || key === 'cut') {
    const leer = ed.selections.every(s => s.isEmpty);
    // Like VS Code: no selection = the whole cursor line.
    const bereiche = leer
      ? ed.selections.map(s => doc.lineAt(s.active.line).rangeIncludingLineBreak)
      : ed.selections.filter(s => !s.isEmpty);
    await vscode.env.clipboard.writeText(bereiche.map(r => doc.getText(r)).join(doc.eol === vscode.EndOfLine.CRLF ? '\r\n' : '\n'));
    if (key === 'cut') ok = await ersetzen(ed, bereiche, bereiche.map(() => ''), '');
  } else if (key === 'paste') {
    const t = await vscode.env.clipboard.readText();
    if (t) ok = await ersetzen(ed, ed.selections.map(s => s), ed.selections.map(() => t), '');
  } else if (key === 'save') {
    ok = await doc.save();
    letzteArt = '';
  } else if (key === 'undo' || key === 'redo') {
    ok = await rueckgaengig(ed, key === 'redo');
  } else {
    return { ok: false, grund: 'taste' };
  }
  return { ok: !!ok };
}

// ---------------------------------------------------------------- terminal
const TERMINAL = {
  Enter: '\r', Backspace: '\x7f', Delete: '\x1b[3~', Tab: '\t', Escape: '\x1b',
  ArrowUp: '\x1b[A', ArrowDown: '\x1b[B', ArrowRight: '\x1b[C', ArrowLeft: '\x1b[D', Home: '\x1b[H', End: '\x1b[F',
};

async function terminalTaste(m) {
  const t = vscode.window.activeTerminal;
  if (!t) return { ok: false, grund: 'kein_terminal' };
  let daten = '';
  if (m.key === 'text') daten = String(m.text || '');
  else if (m.key === 'ctrl') {
    const c = String(m.text || '').toLowerCase();
    if (!/^[a-z]$/.test(c)) return { ok: false, grund: 'taste' };
    daten = String.fromCharCode(c.charCodeAt(0) - 96);
  } else if (m.key === 'paste') daten = await vscode.env.clipboard.readText();
  else if (m.key === 'copy') { await vscode.commands.executeCommand('workbench.action.terminal.copySelection'); return { ok: true }; }
  else if (Object.prototype.hasOwnProperty.call(TERMINAL, m.key)) daten = TERMINAL[m.key];
  else return { ok: false, grund: 'taste' };
  if (daten) t.sendText(daten, false);
  return { ok: true };
}

module.exports = { activate, deactivate };
