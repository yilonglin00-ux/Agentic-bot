// Die SPRACH-SITZUNG als eigenes Leben.
//
// Zwei Dinge, die der Nutzer gemeldet hat und die sich nur am echten
// Dokument zeigen:
//   * "Noki hoert zu" verschwand, sobald Noki eine Aeusserung verarbeitete
//     oder intern einen Schreibtisch vorbereitete.
//   * Die Sprechblase haengt an Noki - laeuft er beim Lesen weiter, muss man
//     ihr hinterherjagen.
// Beides wird hier an der laufenden Seite geprueft, nicht an einer Kopie
// der Regeln.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright-core');
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

const ok = [];
const check = (bedingung, was) => { assert.ok(bedingung, 'FAIL: ' + was); ok.push(was); };

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.addInitScript(() => {
    window.__calls = [];
    const invoke = async (name, args) => {
      window.__calls.push({ name, args });
      if (name === 'intelligence_settings') return { settings:{ level:'off', ask:true, web:false }, installed:true, loaded:false };
      return null;
    };
    window.__nokiEvents = {};
    const listen = async (name, fn) => { (window.__nokiEvents[name] = window.__nokiEvents[name] || []).push(fn); return () => {}; };
    window.__emit = (name, payload) => (window.__nokiEvents[name] || []).forEach(fn => fn({ payload }));
    window.__TAURI__ = { core:{invoke}, event:{listen,emit:async()=>{}}, window:{getCurrentWindow:()=>({onMoved:async()=>()=>{},onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}})} };
    window.__TAURI_INTERNALS__ = { invoke };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/index.html`);
  await page.waitForFunction(() => window.__nokiReady && window.NokiRaum);

  // --- 1. Die Tafel gehoert zur SITZUNG, nicht zur Aeusserung -------------
  const sichtbarkeit = async zustand => page.evaluate(z => {
    document.documentElement.setAttribute('data-noki-stimme', z);
    return getComputedStyle(document.getElementById('nokiStimme')).display;
  }, zustand);
  check(await sichtbarkeit('0') === 'none', 'ohne Sitzung ist die Tafel weg');
  for (const z of ['1', 'arbeitet', 'fertig', 'leer']) {
    check(await sichtbarkeit(z) === 'block',
      `die Tafel bleibt im Zustand "${z}" stehen`);
  }

  // --- 2. Sie ueberlebt die interne Schreibtisch-Vorbereitung -------------
  // `spaceUnsichtbar()` nimmt der BUEHNE die Deckkraft, damit die Figur beim
  // Wechsel kein Gespenst zuruecklaesst. Die Sitzungs-Tafel darf davon nicht
  // betroffen sein - genau daran verschwand sie mitten in der Aufgabe.
  const drin = await page.evaluate(() =>
    document.getElementById('stage').contains(document.getElementById('nokiStimme')));
  check(drin === false, 'die Tafel liegt nicht in der Buehne');
  const traeger = await page.evaluate(() => {
    const t = document.getElementById('nokiStimme').parentElement;
    const s = getComputedStyle(t);
    return { id: t.id, pos: s.position, w: t.clientWidth, h: t.clientHeight, zeiger: s.pointerEvents };
  });
  check(traeger.id === 'nokiUi' && traeger.pos === 'fixed', 'sie haengt an einem eigenen Traeger');
  check(traeger.w === 1280 && traeger.h === 800, 'deckungsgleich mit der Buehne - gleiche Koordinaten');
  check(traeger.zeiger === 'none', 'der Traeger faengt keine Klicks ab');

  // --- 3. Zeiger in der Blase: Noki bleibt STEHEN ------------------------
  await page.evaluate(() => {
    document.documentElement.setAttribute('data-noki-blase', '1');
    window.NokiBlaseStellen();
  });
  const blase = await page.evaluate(() => {
    const r = document.getElementById('nokiBlase').getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
  });
  check(blase.w > 100 && blase.h > 40, 'die Blase steht sichtbar am Bild');

  await page.mouse.move(blase.x, blase.y);
  check(await page.evaluate(() => window.NokiBlaseHalt()) === true,
    'der Zeiger in der Blase haelt Noki fest');
  const a = await page.evaluate(() => window.NokiRaum.ort());
  await page.waitForTimeout(2500);
  const b = await page.evaluate(() => window.NokiRaum.ort());
  check(Math.hypot(b.x - a.x, b.y - a.y) < 6,
    'Noki bewegt sich waehrend des Lesens nicht vom Fleck');

  // Er ist nicht eingefroren: die Zeit laeuft weiter, die Figur wird gezeichnet.
  const bilder = await page.evaluate(() => new Promise(res => {
    let n = 0; const t = () => { if (++n < 8) requestAnimationFrame(t); else res(n); };
    requestAnimationFrame(t);
  }));
  check(bilder === 8, 'die Animation laeuft weiter - nur die Fortbewegung ruht');

  // --- 4. Kleine Nachfrist am Zipfel -------------------------------------
  await page.mouse.move(blase.x, blase.y);
  await page.mouse.move(10, 10);
  check(await page.evaluate(() => window.NokiBlaseHalt()) === true,
    'ein kurzes Streifen ueber die Kante loest den Halt nicht sofort');
  await page.waitForTimeout(500);
  check(await page.evaluate(() => window.NokiBlaseHalt()) === false,
    'nach echtem Verlassen darf er wieder gehen');

  // --- 5. Das Schliessen-Ziel ist greifbar -------------------------------
  const zu = await page.evaluate(() => {
    const r = document.getElementById('nokiBlaseZu').getBoundingClientRect();
    const bl = document.getElementById('nokiBlase').getBoundingClientRect();
    return { w: r.width, h: r.height, schrift: parseFloat(getComputedStyle(document.getElementById('nokiBlaseZu')).fontSize),
             drin: r.left >= bl.left && r.right <= bl.right && r.top >= bl.top && r.bottom <= bl.bottom };
  });
  check(zu.w >= 30 && zu.h >= 30, `das Schliessen-Ziel misst ${zu.w}x${zu.h} px`);
  check(zu.schrift >= 14 && zu.schrift <= 16, `das Zeichen bleibt mit ${zu.schrift} px bescheiden`);
  check(zu.drin, 'das Ziel liegt ganz in der Blase - auch am Bildrand erreichbar');

  await page.mouse.click(blase.x, blase.y); // in die Blase, nicht auf das ✕
  check(await page.evaluate(() => document.documentElement.getAttribute('data-noki-blase')) === '1',
    'ein Klick in die Blase schliesst sie nicht');
  await page.evaluate(() => document.getElementById('nokiBlaseZu').click());
  check(await page.evaluate(() => document.documentElement.getAttribute('data-noki-blase')) === '0',
    'ein Klick auf das ✕ schliesst sie sofort');
  check(await page.evaluate(() => window.NokiBlaseHalt()) === false,
    'und gibt Noki wieder frei');

  // --- 6. Die Blase folgt Noki im selben Takt ----------------------------
  const quelle = fs.readFileSync(path.join(root, 'index.html'), 'utf8');
  const lein = quelle.indexOf('function leinwandStellen');
  assert.ok(lein > 0, 'leinwandStellen existiert');
  check(quelle.slice(lein, lein + 4000).includes('NokiBlaseStellen'),
    'die Blase wird im selben Takt nachgefuehrt wie Nokis Leinwand');

  // --- 7. Nur der Nutzer beendet den Zuhoer-Modus ------------------------
  const h = quelle.indexOf('an("intelligence-voice"');
  const hoerer = quelle.slice(h, quelle.indexOf('var blaseEl', h));
  check(/if \(stimme\.an\) stimmeWache\(\);/.test(hoerer),
    'jedes Lebenszeichen des Helfers setzt die Notbremse zurueck - Schweigen beendet nichts');
  const final = hoerer.slice(hoerer.indexOf('if (p.final && typeof p.text'));
  // Der ERKLAERENDE Kommentar nennt `noki_stimme_beendet` mit Absicht -
  // gesucht ist der AUFRUF.
  check(!/stimmeRuf\("noki_stimme_beendet"/.test(final.slice(0, final.indexOf('} else if'))),
    'das Ende einer Aeusserung beendet nicht den Modus');

  // --- 8. "fuer mich" ist der Empfaenger, nicht der Ort -----------------
  const ORT_NOKI = await page.evaluate(() => window.NokiOrt.aus('Oeffne Chrome'));
  for (const satz of ['Oeffne Spotify fuer mich', 'Oeffne mir Spotify', 'Oeffne Spotify']) {
    check(await page.evaluate(t => window.NokiOrt.aus(t), satz) === ORT_NOKI,
      `"${satz}" laeuft auf Nokis Arbeitsplatz`);
  }
  for (const satz of ['Oeffne Spotify bei mir', 'Oeffne Spotify hier',
                      'Oeffne Spotify auf meinem Schreibtisch', 'Oeffne Spotify auf meinem Desktop']) {
    check(await page.evaluate(t => window.NokiOrt.aus(t), satz) !== ORT_NOKI,
      `"${satz}" laeuft beim Nutzer`);
  }

  // --- 9. Blase und Noki liegen in DERSELBEN Tiefe ----------------------
  // Laeuft er hinter einem Fenster entlang, wird sein Rechteck aus der
  // Leinwand geschnitten. Die Blase muss denselben Schnitt bekommen, sonst
  // schwebt sie ueber dem Fenster, das ihn gerade verdeckt.
  await page.evaluate(() => {
    document.documentElement.setAttribute('data-noki-blase', '1');
    window.NokiBlaseStellen();
  });
  const ohne = await page.evaluate(() => document.getElementById('nokiBlase').style.clipPath || '');
  check(ohne === '', 'ohne verdeckendes Fenster wird nichts geschnitten');
  const geschnitten = await page.evaluate(() => {
    const b = document.getElementById('nokiBlase').getBoundingClientRect();
    window.NokiBlaseStellen({ x: b.left + 10, y: b.top + 10, w: 60, h: 40 });
    return document.getElementById('nokiBlase').style.clipPath || '';
  });
  check(/^polygon\(evenodd/.test(geschnitten), 'das Fensterrechteck wird aus der Blase geschnitten');
  const frei = await page.evaluate(() => {
    window.NokiBlaseStellen(null);
    return document.getElementById('nokiBlase').style.clipPath || '';
  });
  check(frei === '', 'ist er wieder vorn, ist auch die Blase wieder ganz');

  const quelle2 = quelle;
  const li = quelle2.indexOf('function leinwandStellen');
  const leinwand = quelle2.slice(li, li + 3000);
  check(leinwand.indexOf('canvas.style.clipPath') < leinwand.indexOf('NokiBlaseStellen(mr)'),
    'die Blase bekommt dieselbe Maske wie die Figur, im selben Schritt');

  // --- 10. Die Miniatur navigiert nicht mehr ueber ihren Inhalt ----------
  const swift = fs.readFileSync(path.join(root, 'schirm/main.swift'), 'utf8');
  // lastIndexOf: `NaviKnopf` hat sein eigenes mouseDown - gemeint ist das der Ansicht.
  // Panel und Mitlese-Tap teilen druckBeginn/druckEnde; beide gehoeren dazu.
  const md = swift.slice(swift.lastIndexOf('override func mouseDown'), swift.indexOf('func rollen('));
  check(!/KLICK besuchen/.test(md), 'ein Klick auf den Inhalt besucht keinen Schreibtisch mehr');
  check(/ZEIGER klick/.test(md), 'er geht als Zeigerereignis an das echte Fenster');
  check(/fensterAn\(p\)/.test(md), 'und nur dann, wenn dort wirklich ein Fenster liegt');
  const navi = swift.slice(swift.indexOf('final class NaviKnopf'), swift.indexOf('final class Ansicht'));
  check(/KLICK besuchen/.test(navi), 'nur der Fussknopf navigiert noch');
  check(!/Schreibtisch 3/.test(swift), 'die Schreibtischnummer steht nirgends fest im Code');
  const rust = fs.readFileSync(path.join(root, 'src-tauri/src/vorschau.rs'), 'utf8');
  check(/fn ziel_erlaubt/.test(rust) && /aufgenommene\(\)\.contains/.test(rust),
    'ferngesteuert wird nur, was die Miniatur wirklich zeigt');

  // --- 11. Der Klick in die Miniatur klappt sie nicht mehr zu -----------
  // Der Beobachter "Druck NEBEN die grosse Miniatur" fragte die absichtlich
  // LEERE Trefferzone des Overlays ab und bekam fuer jeden Druck "nicht
  // drin" - auch mitten in der Miniatur. Er fragt jetzt ihre wirkliche
  // Flaeche.
  const rs = fs.readFileSync(path.join(root, 'src-tauri/src/lib.rs'), 'utf8');
  const beob = rs.slice(rs.indexOf('Grosse Miniatur + Druck NEBEN sie'),
                        rs.indexOf('Grosse Miniatur + Druck NEBEN sie') + 1400);
  check(/h\.fw > 0/.test(beob) && !/h\.vw > 0/.test(beob),
    'der Beobachter fragt die Flaeche der Miniatur, nicht die leere Trefferzone');
  check(/hit\.fx = x;/.test(rs) && /hit\.vx = 0;/.test(rs),
    'beide Rechtecke bleiben getrennt gefuehrt');

  // --- 12. Die Wahl des Noki-Schreibtischs -------------------------------
  // Der ECHTE Tastenweg (Abgriff, Pfeil im Praefix-Fenster) - nicht ein
  // Pruefhaken daneben. Frueher rief er `schreibtisch_weiter` (echte
  // Navigation), waehrend der Test ueber `wswahl:` die Wahl pruefte.
  const tap = rs.slice(rs.indexOf('fn modifier_event_innen'), rs.indexOf('fn right_shift_monitor'));
  check(/matches!\(code, 123 \| 124 \| 125 \| 126\)/.test(tap) && /arbeitsplatz_wahl_planen/.test(tap),
    'LINKS-VON-1 + Pfeil reiht im echten Tastenweg nur die Wahl ein');
  check(!/schreibtisch_weiter|sichtbar_zum_space|overlay_verdecken/.test(tap),
    'der Tastenweg navigiert nicht und verdeckt nichts');
  check(!/fn schreibtisch_weiter/.test(rs), 'es gibt keinen Navigationsweg mehr fuer dieses Kuerzel');
  check(/catch_unwind\(\|\| modifier_event_innen/.test(rs) && /catch_unwind\(\|\| gedrueckt_innen/.test(rs),
    'kein Fehler aus einem Tastenrueckruf beendet Noki');
  check(/wswahl:/.test(rs) && /arbeitsplatz_wahl_planen\(&h, vor\)/.test(rs),
    'der Pruefhaken nimmt denselben Einstieg wie die Taste');
  // Genau die eine Funktion - der naechste `fn` gehoert schon nicht mehr dazu.
  const wStart = rs.indexOf('fn arbeitsplatz_waehlen');
  const wahl = rs.slice(wStart, rs.indexOf('\n#[cfg(not(target_os', wStart));
  check(!/sichtbar_zum_space/.test(wahl), 'die Wahl bewegt den Nutzer nicht');
  check(/naechster_arbeitsplatz/.test(wahl) && /einstellungen_setzen/.test(wahl),
    'sie waehlt ueber die uuid und schreibt die Reservierung fort');

  // --- 13. Der Fussknopf hat eine eigene Trefferflaeche ------------------
  const ht = swift.slice(swift.indexOf('override func hitTest(_ punkt'),
                         swift.indexOf('override func hitTest(_ punkt') + 500);
  check(/navi\.frame\.contains/.test(ht),
    'hitTest reicht den Druck an den Navigationsknopf weiter');

  // --- 14. Navigation prueft nach JEDEM Schritt neu ----------------------
  const nav = rs.slice(rs.indexOf('pub fn sichtbar_zum_space'),
                       rs.indexOf('fn dock_wisch'));
  check(/loop \{/.test(nav) && /space_reihenfolge\(\)/.test(nav),
    'der Wechsel liest die Reihenfolge in der Schleife, nicht einmal vorab');
  check(!/for _ in 0\.\.n/.test(nav), 'keine vorausberechnete Schrittzahl mehr');
  check(/frist/.test(nav) && /zu viele Schritte/.test(nav),
    'er ist durch Zeit UND Schrittzahl begrenzt');
  check(/if jetzt == sid/.test(nav),
    'und endet nur, wenn der Bildschirm wirklich am Ziel steht');

  // --- 15. Die Wahl fasst die Ortsbindung der Figur nicht an -------------
  check(!/ortsbindung_loesen\(\)/.test(wahl),
    'die Wahl loest keine Ortsbindung - sonst zieht die Figur um und wird unsichtbar');
  // Die Definition, nicht die erste Erwaehnung im Kommentar.
  const vStart = rs.indexOf('fn overlay_verdecken(app:');
  const verd = rs.slice(vStart, rs.indexOf('fn overlay_freigeben(app:', vStart));
  check((verd.match(/thread::spawn/g) || []).length === 2 && /notbremse/i.test(verd),
    'ein verdecktes Overlay kommt in jedem Fall zurueck');

  console.log(ok.map(s => 'ok - ' + s).join('\n'));
  console.log('\nVoice-Sitzung: alle Pruefungen bestanden');
  await browser.close();
  server.close();
})().catch(e => { console.error(String(e && e.message || e)); process.exit(1); });
