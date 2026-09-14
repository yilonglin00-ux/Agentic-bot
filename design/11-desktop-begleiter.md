# 11 · Desktop-Begleiter

> Phase 3.14. Wie aus der Charakterstudie eine App wird, die JARVIS heißt und Noki zeigt.
> Diese Datei beschreibt nur, was in dieser Stufe dazugekommen ist; alles darunter
> (Rig, Ausdruck, Bewegungssprache, Greifsystem, Bewegungsraum, Zustandsbild) bleibt
> unverändert und steht in [03](03-rig-und-animation.md) bis [10](10-greifsystem.md).

---

## Die Rollenverteilung

| Ebene | Wer | Was |
|---|---|---|
| Produkt | **JARVIS** | Die App. Name, Fenster, Menüleiste, Bundle. |
| Sichtbare Präsenz | **Noki** | Die Figur *in* dieser App. Keine zweite App. |
| Verhalten | **Noki** | Entscheidet selbst, was ein gemeldetes Ereignis bedeutet. |
| Meldung | **JARVIS-Core** | Meldet Zustände und Ereignisse. Steuert keine Animation fern. |

Die native Seite (`desktop/src-tauri/src/lib.rs`) **liest** drei Dateien, die der Core
ohnehin schreibt, und verwaltet das Fenster. Sie entscheidet nichts über Nokis Verhalten,
und es gibt weiterhin keinen Rückkanal in den Action Layer.

---

## Das Fenster

Ein rahmenloses, durchsichtiges Panel von 460 × 400 Punkten, ohne Titelleiste, ohne
Dock-Symbol (`ActivationPolicy::Accessory`). Nokis Raum aus Phase 3.13 — Boden, Raster,
Horizont, Lichtvolumen, Bodenkontakt, Lichtfuge — bleibt vollständig erhalten; die Bühne
ist seine Umgebung, nicht die Seite. Abgerundete Fassung, weicher Schatten.

Bedient wird das Panel so:

| Geste | Wirkung |
|---|---|
| Ziehen **auf** Noki | Er wird aufgehoben (siehe unten) |
| Ziehen **neben** Noki | Das ganze Panel wandert über den Schreibtisch |
| Klick auf Noki | Kleine Reaktion |
| Doppelklick auf Noki | Er verschwindet, JARVIS läuft weiter |
| Menüleiste → *Noki zeigen* | Er kommt zurück |

Das Symbol in der Menüleiste hat außerdem drei Sichtbarkeitsebenen
(Abschnitt 14): **Im Vordergrund** (über gewöhnlichen Fenstern, Vorgabe),
**Wie ein Fenster** (kann verdeckt werden) und **Im Hintergrund** (unter
gewöhnlichen Fenstern). Eine echte Schreibtisch-Ebene — hinter allen Fenstern,
im Hintergrundbild — steht Drittanbietern unter macOS nicht offen.

Dieselbe Datei bleibt im Browser genau das, was sie war: `desktop/index.html` schaltet die
App-Eigenschaften nur ein, wenn sie tatsächlich in Tauri läuft (`data-app="tauri"`).
`#studio=1` ist unverändert die 360°-Charakterstudie mit allen Reglern.

---

## Autonomes Verhalten

Der Bewegungsraum aus Phase 3.12 war gebaut, aber niemand lief darauf: `raum.dreh` stand
fest, ein Ziel gab es nicht, und die einzige Fortbewegung war die Einlage B8 — fünf
Sekunden geradeaus. Phase 3.14 setzt eine kleine Aktivitätswahl darüber.

Neun Aktivitäten, gewichtet gezogen, mit Sperre über die letzten zwei Wahlen:

| Aktivität | Gewicht | Was passiert |
|---|---|---|
| `gehen` | 4.0 | Ziel auf der Ebene wählen, eindrehen, hinlaufen, anhalten |
| `stehen` | 3.2 | 4–11 s nichts tun |
| `umsehen` | 2.6 | Einlage B1 |
| `wenden` | 1.7 | Auf der Stelle umdrehen |
| `gewicht` | 1.6 | Gewicht verlagern |
| `neugier` | 1.3 | E5, E2 oder D3k |
| `sitzen` | 1.3 | Hinsetzen, 10–20 s, aufstehen |
| `schlaf` | 0.45 | Hinlegen, einschlafen, **10 s** schlafen, aufwachen, aufstehen |
| `liegen` | 0.28 | Hinlegen, 9 s, aufrichten |

Hinlegen ist die auffälligste Aktivität und lag mit 0.8 gleichauf mit dem Umsehen. Seit
Phase 3.16 tragen `liegen` und `schlaf` zusammen 0.73 statt 1.7, dazu eigene Wartezeiten
(`liegen` 420 s, `schlaf` 360 s, und `schlaf` respektiert die Liegesperre). Das frei
gewordene Gewicht ging an `stehen`, `umsehen` und `gewicht`. Gemessen über 900 simulierte
Sekunden: **11× vorher, 3× nachher**. Verboten ist es nicht — die Wege bleiben offen.

Gemessen über 900 simulierte Sekunden: rund 90 Aktivitäten, 9 Arten, **13–18 % gehend**,
**28–31 % Leerlauf**. Er läuft also nicht dauernd und zappelt nicht.

Sitzen, Liegen und Schlafen laufen über die **vorhandenen** Wünsche (`sitzWunsch`,
`liegeWunsch`, `schlafWunsch`) — dieselben Abläufe, die im Studio die Tasten auslösen.
Es gibt keine zweite Ablaufmechanik.

### Laufen und Körperdrehung

Der Gang trägt Noki über die Ebene: als Strecke dient genau der Wert, der auch die
Bodenbänder schiebt (`bodenZ`). Ortsübersetzung und Fußbewegung können deshalb gar nicht
auseinanderlaufen — der Selbsttest prüft es trotzdem jedes Bild.

Gedreht wird mit begrenzter Winkelgeschwindigkeit (1.85 rad/s), und die Rate fällt zum Ziel
hin ab: die Drehung klingt aus, statt anzuhalten. Bei einer Abweichung über 0.95 rad wird
erst gedreht und dann gelaufen; darunter dreht er während der ersten Schritte weich ein.

> **Ein Fehler, der ohne die Autonomie nie sichtbar geworden wäre:** `raum.idreh` lief
> vorher über eine gerade Interpolation dem Sollwinkel nach. Solange `raum.dreh` immer 0
> war, fiel das nicht auf. Sobald wirklich gedreht wird, kann das Ziel über ±π springen —
> und die gerade Interpolation nimmt dann den langen Weg einmal herum. `dWinkel()` nimmt
> jetzt den kurzen.

### Eine einzige Laufebene: WALK_Y

Noki läuft nicht in einem unteren *Bereich*, sondern auf genau **einer** waagerechten
Linie. Sie heißt `WALK_Y` und ist `FELD.unten`:

```
WALK_Y = unterer Rand der sichtbaren Arbeitsfläche − 14 pt Fußrand
```

Maßgeblich ist die **Arbeitsfläche**, nicht der Bildschirm: `noki_schirm_info` liefert
zusätzlich zur Bildschirmgeometrie die `work_area` des Monitors — unter macOS die
`visibleFrame`, also ohne Menüleiste und **ohne Dock**. Deshalb steht Noki auf dem
sichtbaren Schreibtisch statt hinter dem Dock. Fehlt die Angabe oder ist sie unplausibel,
gilt wieder der ganze Bildschirm. Bezugspunkt ist Nokis **Fußpunkt** (`raum.y`), nicht
seine Körpermitte; `leinwandStellen()` setzt die Leinwand über `FUSS_ANTEIL` genau darauf.

Auf dieser Linie bewegt er sich ausschließlich waagerecht: `raumTakt` ruft `raumGehen`
immer mit `dy = 0`. Der Selbsttest zählt jedes Bild der 900-Sekunden-Autonomie mit und
meldet, sobald die Standlinie auch nur um einen Punkt abweicht.

| Zustand | Bedingung | Was gilt |
|---|---|---|
| **frei** | \|`raum.y` − WALK_Y\| ≤ 4 pt | Gehen, Wenden, Sitzen, Liegen, Schlafen |
| **front** | weiter oben abgesetzt | keine Fortbewegung; Blick, Emotionen, Maus, Klick, alle fünf Zustände laufen weiter |

**Einrasten.** Wer beim Loslassen innerhalb von 50 pt über WALK_Y ist, wird auf die Ebene
gesetzt — aber nicht geschnitten: `raum.y` steht sofort auf WALK_Y, die *dargestellte*
Lage `raum.iy` läuft in gut einer halben Sekunde weich nach (`raum.snapY`). Weiter nach
unten ziehen geht gar nicht erst: `griffZieh` klemmt auf WALK_Y, unter der Ebene bleibt
er also nie.

### Gehtempo

`GANGF` ist der **einzige** Tempo-Regler: 0.72 (Vorlage) → 0.92 → 1.18 → **1.40** Zyklen
je Sekunde. Weil der Bildschirmweg aus der wirklich zurückgelegten Fußstrecke kommt
(`bodenZ`) und nicht aus einer eigenen Geschwindigkeit, ziehen Schrittfrequenz und
Ortsversatz immer gemeinsam an — schneller heißt **schnellere Schritte**, nicht größere
Schritte im alten Takt. Gleiten ist damit bauartbedingt ausgeschlossen; die Schrittlänge
(`SCHWUNG`) bleibt unverändert. Rund 2.8 Schritte je Sekunde ist zügiges Gehen, kein
Rennen. `GEH_TEMPO` (0.190 Körperhöhen/s) ist nur die daraus abgeleitete Zahl für die
Sicherheitsdauer eines Weges.

Die langen Gehziele richten sich nach der wirklich nutzbaren Breite
(`0.62 × (FELD.rechts − FELD.links)`) statt nach einer festen Punktzahl: auf einem breiten
Schreibtisch kommt Noki auch wirklich weit.

`GEH_TEMPO` ist **abgeleitet**, nicht frei gewählt: je Gangzyklus legt der Fuß 0.1356
Körperhöhen zurück (Messwert aus `bodenZ`), also ist `GEH_TEMPO = GANGF · 0.1356`. Es wird
bei jeder Tempoänderung mitgezogen, sonst schätzte die Autonomie die Laufdauer falsch.

---

## Vorrang

Nokis Eigenleben steht ganz unten:

```
6  ERROR
5  aktive Benutzerinteraktion (anfassen, klicken, ziehen; 7 s Nachlauf)
4  LISTENING / PROCESSING / SPEAKING
3  Erscheinen, Verschwinden, Frontmodus
2  eigene Aktivitäten
1  Leerlauf
```

Ein Abbruch ist **nie ein Schnitt**. `autoAbbrechen()` setzt genau die Wünsche, mit denen
Noki sich von selbst wieder aufrichtet, und lässt die vorhandenen Kurven vollständig
durchlaufen: wer schläft, wacht erst auf, richtet sich dann auf, steht dann auf. Diese
Reihenfolge ist nicht verdrahtet — sie ergibt sich aus den Bedingungen der Kurven selbst.

---

## Fenster als Hindernisse

Nokis Laufebene ist ein echter Raum: was sichtbar darin steht, geht er nicht durch.

### Woher die Fenster kommen

`CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | ExcludeDesktopElements)`.
Bewusst diese Quelle, weil sie **ohne zusätzliche Berechtigung** auskommt: Bildschirm-
aufnahme verlangt macOS nur für `kCGWindowName`, den Fenster**titel** — und genau der wird
hier nicht gelesen. Abgefragt werden Rechteck, Ebene, Deckkraft, Fensternummer und der
Programmname des Besitzers.

**Nicht** gelesen: Titel, Inhalt, DOM, Text, Screenshots, OCR, Bedienungshilfen. Es gibt
keinen Browser-Tab-Zugriff; „Tab" heißt hier ausschließlich sichtbare Fenstergeometrie.

Gefiltert wird auf Fensterebene 0–3 (gewöhnliche Fenster und schwebende Paneele).
Darüber liegen Menüleiste (24), Kontrollzentrum (25) und Nokis eigenes Panel (5),
darunter Schreibtisch-Widgets — beides ist kein Hindernis. Dazu: eigene PID aus,
Deckkraft < 0.15 aus, Kanten unter 60 pt aus, höchstens 24 Fenster.

Der Beobachter läuft mit **4 Hz** und meldet nur, wenn sich die Liste wirklich geändert
hat. 60 Hz wäre hier Verschwendung: Fenster bewegen sich in Menschentempo, und die
Kollision selbst rechnet das Frontend ohnehin jedes Bild.

### Koordinaten

Die Rechtecke kommen bereits in logischen Punkten und im selben Schreibtischsystem wie
`CGEventGetLocation` — Ursprung links oben am Hauptbildschirm, y nach unten. Umgerechnet
wird an genau **einer** Stelle im Frontend, mit demselben Ursprungsabzug wie beim Zeiger:

```
fensterX = rechteck.x − buehne.x        (Schreibtisch → Fenster)
fensterY = rechteck.y − buehne.y
```

Die rohe Liste bleibt unverändert liegen. Ändert sich der Bildschirm oder WALK_Y, wird
sie neu ausgerechnet statt verworfen.

### Wann ein Fenster blockiert

Nur wenn es die Laufebene **wirklich schneidet**:

```
oben <= WALK_Y <= unten
```

Ein Fenster, das über Noki endet, ist kein Hindernis — er läuft darunter entlang. Aus
jedem schneidenden Fenster wird eine gesperrte Strecke auf der x-Achse:

```
kl = links  − halbe Körperbreite − 3 pt
kr = rechts + halbe Körperbreite + 3 pt      halbe Körperbreite = 0.42 · NOKI_HOCH
```

Überlappende Strecken werden **nach** dem Aufblähen zusammengefasst: passt Noki zwischen
zwei Fenster nicht hindurch, ist die Lücke keine. Mehr als diese sortierte Liste braucht
es nicht — Noki läuft auf einer Geraden, also genügt ein 1D-Modell. Kein Pfadfinder,
kein Physikmodul, kein zweites Distanzfeld.

### Die Begegnung

Ein Fenster ist **keine Wand**. Läuft Noki darauf zu, wird einmal gewürfelt, was er damit
anfängt:

```
keins → (Vorausschau) → EINE Entscheidung → durch | umkehr | lehnen → keins
```

Die Vorausschau ist **1.6 Körperhöhen** vor ihm — größenbezogen, nicht als fester
Pixelwert —, damit noch Zeit für eine sichtbare Reaktion bleibt. Die Entscheidung fällt
**genau einmal je Begegnung**; danach ist dieselbe Kante 8 s gesperrt. Ohne diese Sperre
würfelte er Bild für Bild neu und flatterte zwischen den Zuständen.

Gewichtet wird aus **einem** Zufallswert, nicht gleichverteilt über drei Zustände:

| Wert | Reaktion | Anteil |
|---|---|---|
| 0.00 – 0.50 | **durchgehen** | 50 % |
| 0.50 – 0.75 | **umkehren** | 25 % |
| 0.75 – 1.00 | **anlehnen** | 25 % |

Die Zuordnung steht allein in `begWahl(w)` — eine reine Funktion ohne Zufall im Rumpf,
damit der Selbsttest die Grenzen mit festen Werten prüfen kann.

**durchgehen.** Er sieht vielleicht kurz hin und läuft weiter. Sein Ziel wird hinter das
Fenster verlegt, und die Sicherheitsdauer des Laufs wächst mit — sonst bräche der Lauf
mitten im Fenster ab. Damit das Fenster ihn dabei wirklich **verdeckt**, wird Nokis Panel
für die Dauer des Durchgangs eine Ebene tiefer gelegt (`noki_ebene_durchgang`). Die vom
Nutzer in der Menüleiste gewählte Ebene bleibt gespeichert und gilt sofort wieder, sobald
der Durchgang endet — auch beim Verbergen, beim Wiederkommen und bei jeder ausdrücklichen
Auswahl. Seine Weltposition läuft dabei durchgehend weiter; es wird nichts gesprungen.

**umkehren.** Er hält an der Kollisionskante, dreht sich und bekommt ein neues Ziel
**bis zum gegenüberliegenden Bildschirmrand** — nicht ein paar Punkte zurück, sonst
liefe er sofort wieder dasselbe Fenster an. Die Aktivität wird dabei ausdrücklich
weitergeführt, statt sich auf die Reihenfolge im Bild zu verlassen.

**anlehnen.** Er hält am **Kontaktpunkt** (Kante minus halbe Körperbreite), dreht sich zur
Kante — 0.62 · Seitenansicht, das Gesicht bleibt sichtbar —, der Rumpf rollt dagegen, die
Schulter gibt nach, das Standbein wechselt. Nach 3–8 s löst er sich wieder. Alles nur als
**Versatz** auf dem fertigen Rig, in denselben geprüften Kanalgrenzen wie Zustandsgebärde
und Aufheben; es gibt keine zweite Rig-Mechanik, und der Ort ändert sich dabei nicht.
Kommt er von links, lehnt er an der linken Kante, von rechts an der rechten.

Das Anlehnen endet von selbst, sobald die Kante nicht mehr da ist — Fenster verschoben
(> 12 pt), geschlossen, Noki hochgehoben, Laufebene verlassen, oder JARVIS übernimmt. Er
klebt nicht am Fenster fest und bleibt nie in der Luft stehen.

### Was unberührt bleibt

Ziehen ist nie blockiert: wer Noki mit der Hand über ein Fenster zieht, darf das. Auch die
Zielwahl kennt keine Fenster mehr — was auf dem Weg liegt, entscheidet die Begegnung
unterwegs.

---

## Aufheben

Beim Ziehen hat Noki keine Kontrolle über seinen Ort.

* Die Füße verlieren den Boden (`u_hoch`) — der Strahl wandert, nicht die Figur, wie schon
  bei `u_ort`. Kosten: eine Subtraktion.
* Der Schlagschatten weitet und schwächt sich von selbst, weil der Bodenpunkt weiter weg
  liegt. Das Lichtvolumen hängt an der Brust und geht mit.
* Der Körper hängt: Beine pendeln nach, Arme fallen zur Seite, der Rumpf rollt in die
  Zugrichtung. Die Trägheit steckt bereits in `raum.ix/iz` und wird nur benutzt.
* Die Antenne bekommt eine **eigene** gedämpfte Feder auf dem Darstellungswert. In
  `rig.ant` zu schreiben wäre der Fehler, den [Abschnitt 12b](../desktop/index.html)
  schon beschreibt: die Feder dort regelte den Versatz binnen zweier Bilder gegen sich
  selbst weg.

Beim Loslassen wird die Lage bewertet — **erst dann**: innerhalb der Ebene gilt wieder
freies Verhalten, außerhalb der Frontmodus aus Phase 3.12 (keine Fortbewegung, aber
Reaktionen, Emotionen und alle fünf Zustände).

---

## Augen und Display

Die Augen sind **keine Geometrie**, sondern werden auf das Visierglas gemalt
(`paintGlass`). Sie gehören damit lokal zum schwarzen Display:

```
toHead(p)                     Kopfraum
  uv = (hp.x, hp.y − 0.249)   Displaykoordinaten, Ursprung Displaymitte
  Augenmitte |uv.x| = 0.104   fest, unabhängig von Tempo, Neigung, Spin
  + Blickversatz              klein, DARIN, siehe unten
```

**Die Ursache des Wegwanderns** lag eine Ebene höher: die Geometrie wird über
`mapChar` mit `zuFlug(p)` in die Fluglage gebracht, die aufgemalten Details bekamen
aber den **rohen** Marschpunkt. Kopf und Display drehten sich also, Augen, Brustscheibe
und Mundlinie blieben im ungedrehten Rahmen stehen — genau deshalb wanderten sie bei
Flugneigung, FLITZEN und Spin aus dem Display. Jetzt geht der Punkt einmal durch
`zuFlug()`, bevor gemalt wird; damit rotiert das Display mitsamt seinen Augen als eine
Fläche.

**Der Blickversatz** war ein fester Betrag (0.016 waagerecht, 0.013 senkrecht) und stand
in keinem Verhältnis zur Augengröße. Ein großes Auge (`ueberrascht`, Ring 0.080) wurde
davon über die Kante geschoben. Jetzt ist der Spielraum das, was zwischen dem **äußeren
Rand des Augenrings** und der Displaygrenze übrig bleibt:

```
ringR = u_eye.y + u_eye.z
frX   = clamp(0.180 − (0.104 + ringR), 0, 0.020)
frY   = clamp(0.079 − (u_eye.y · offen + u_eye.z), 0, 0.016)
```

Ein kleines Auge (`denkend`) darf dadurch weiter schauen als ein großes, und beim Blinzeln
(kleines `offen`) wächst der senkrechte Spielraum mit. Der Selbsttest prüft alle
Ausdrücke × drei Lidstellungen × elf Blickrichtungen — einschließlich Werten außerhalb
des gültigen Bereichs, die der Clamp abfangen muss.

Die Displaymitte liegt bei `hp.y = 0.249` statt wie früher 0.255: die sichtbare schwarze
Fläche ist gegenüber dem Visierschnitt leicht nach unten versetzt, weil sie mit der
Kopfschale verschnitten wird.

## Mausblick

Ist der Zeiger in Nokis Nähe, folgen erst die Augen, dann der Kopf. Die Ausschläge sind
klein (Kopf ≤ 0.32 rad, Neigung ≤ 0.14 rad) und liegen **innerhalb** derselben Klammern
wie Drift und Zuwendung in `updateRig` — der Mausblick kann das Rig also nicht aus seinem
geprüften Bereich schieben. Im Schlaf fällt er weg.

Der Wahrnehmungsradius ist `MAUS_NAH = 7.2` Körperhöhen mit weichem Abfall
(`(1 − u)^1.35`): nah deutliche Zuwendung, mittig klarer Blick, weit draußen nur noch
eine Spur, jenseits davon wieder der eigene Blick. Die Augen folgen mit 0.10 s
Zeitkonstante, der Kopf mit 0.24 s — der Blick läuft also voraus, der Kopf kommt nach.
Die Antenne hängt ohnehin an der Kopfgeschwindigkeit und reagiert dadurch von selbst mit.

**Die Achse — einmal durchgerechnet, nicht geraten.** Kopf und Augen laufen über zwei
verschiedene Größen und brauchen deshalb verschiedene Vorzeichen:

| Kanal | Größe | Zeiger rechts | Zeiger oben |
|---|---|---|---|
| `rig.head[1]` / `[0]` | **Winkel** (`rotY`/`rotX` am Strahl, also `+` = links bzw. unten) | negativ | negativ |
| `rig.look[0]` / `[1]` | **Versatz** im Kopfraum (`paintGlass`, `+x` = Bild rechts, `+y` = oben) | positiv | positiv |

Kopf-Gierwinkel und Blick-Versatz sahen bis Phase 3.15 beide mit `-dx` zum Zeiger — der
Kopf richtig, die Augen dadurch **von ihm weg**; senkrecht war es genau umgekehrt. Der
Blick las sich deshalb schwach und unentschieden. Der Selbsttest prüft jetzt beide Achsen
mit ihrem jeweils richtigen Vorzeichen.

> Die gleiche Vorzeichenverwechslung steckt noch in den **Greif-Einlagen** (`GRIFF_HALT`:
> `hY` und `lx` dort gleichsinnig) und in `B1 Umsehen`. Das ist ein alter, sichtbarer,
> aber sehr kleiner Fehler im Gesicht und wurde hier bewusst **nicht** angefasst — er
> gehört nicht zur Laufebene und würde jede vorhandene Einlage verändern.

Der Zeiger setzt die Ruheuhr ausdrücklich **nicht** zurück: ein vorbeiziehender Zeiger ist
Neugier, keine Interaktion. Täte er es, käme Nokis Eigenleben nie zum Zug.

---

## Streicheln

Der Zeiger ist mehr als eine Blickrichtung. Bewegt er sich **um Noki herum**, versteht er
das als Zuwendung — aber nicht jede Bewegung zählt. Als Streicheln gilt nur, was drei
Dinge zugleich erfüllt:

| | Bedingung |
|---|---|
| nah | innerhalb von **1.35 Körperhöhen** |
| maßvoll | schneller als **0.30**, langsamer als **7.0** Körperhöhen/s |
| zugewandt | ein **Richtungswechsel** (in einer der beiden Achsen) **oder** 0.8 s ununterbrochene ruhige Bewegung in der Nähe — das langsame Umkreisen wechselt die Richtung selten, ist aber genauso Zuwendung |

Alle Schwellen stehen in Körperhöhen je Sekunde — sie hängen an Nokis Größe, nicht an
einer Bildschirmauflösung. Der zweite Weg gilt nur bis 3.0 Körperhöhen/s: ein einzelner
schneller Durchzug erfüllt weder ihn noch den Richtungswechsel und bleibt folgenlos.

Daraus wächst ein **Maß** (0…1.2): +1.60/s bei guter Bewegung, −0.28/s ohne.

```
Maß ≥ 0.30   → glücklich
Maß ≥ 0.85   → aufgeregt
```

**Die Antwort ist immer positiv.** Es gibt keinen Weg mehr, über die Maus einen negativen
Ausdruck auszulösen: hektisches Fahren ist einfach *kein* Streicheln und läuft ins Leere,
statt ihn zu nerven. Die frühere Hektik-/Duldungsmechanik und die Vorstufe „zufrieden"
sind entfallen — die erste erkannte Berührung führt direkt zu **glücklich**.

**Warum es früher zu spät auslöste**, waren drei Dinge zusammen, und das erste war das
eigentliche Problem: bei **jeder Richtungsumkehr geht das Zeigertempo durch null**. Mit der
schnellen Glättung (τ 0.12) fiel es dabei unter die untere Schwelle, `nahZeit` wurde auf 0
zurückgesetzt und das Maß baute sich wieder ab — genau in dem Moment, in dem eine Hand hin
und her streicht. Dazu kam ein zäher Aufbau (0.62/s) und eine Vorstufe bei 0.22, hinter der
„glücklich" erst 0.9 s später kommen durfte. Jetzt: Glättung τ 0.18, untere Schwelle 0.18
statt 0.30, `nahZeit` wird beim kurzen Ausreißer nur **abgebaut** statt genullt, Aufbau
1.60/s, und die erste Stufe ist gleich die glückliche. Gemessen reichen **zwei Striche in
gut einer halben Sekunde**.

`setAusdruck` wird nur beim **Stufenwechsel** gerufen und frühestens 0.55 s nach dem letzten
— kein Ausdruck je Bild. Solange eine Stufe steht **oder auch nur das Maß anläuft**, hält
sie die Grundstimmung 2.2 s zurück; sonst könnte die Stimmungsmaschine mitten in die
beginnende Berührung hinein etwa „müde" setzen. Danach übernimmt die Stimmungslogik von
selbst wieder — sie ist der Rückweg, es braucht keinen eigenen.

> **Einheitenfalle.** `stimmungBis` ist eine **Dauer ab `ausdruckAb`**, keine Uhrzeit:
> `stimmungTakt` prüft `time - ausdruckAb < stimmungBis`. Das Streicheln schrieb dort
> anfangs `time + STR_NACH`, also eine Uhrzeit in eine Dauer. Daraus wurde eine Wartezeit
> von Tausenden Sekunden, und der glückliche Ausdruck blieb praktisch für immer stehen.
> Richtig ist `(time − ausdruckAb) + STR_NACH`. Der Selbsttest misst jetzt ausdrücklich,
> dass er nach der Nachwirkung wieder zur Grundstimmung zurückfindet — und dass er es nach
> 12 s garantiert getan hat.

**Im Schlaf passiert nichts:** bei manuellem Schlaf, `schlafU` oder `liegeU` wird der
Zustand zurückgesetzt. Die vorhandene Weckregel bleibt die einzige Tür.

`genervt` bleibt als Eintrag in der `EMO`-Tabelle bestehen (im Studio manuell wählbar),
wird von der Mausinteraktion aber **nicht mehr ausgelöst**. In der Grundstimmungswahl stand
er ohnehin nie.

## Zubehör: was der Rechner gerade tut

Noki kann sichtbar auf den Zustand des Rechners reagieren. Der erste Fall: **spielt Spotify,
trägt er Kopfhörer.**

Es gab dafür keine vorhandene Infrastruktur — im JARVIS-Kern steht Spotify nur in der
Namensliste erlaubter Apps für `app.open`. Gebaut ist deshalb die kleinste read-only
Brücke, die macOS hergibt, in zwei Schritten:

```
1. pgrep -x Spotify        läuft der Prozess überhaupt?
2. osascript … player state  playing / paused / stopped
```

Schritt 1 ist nicht optional: `tell application "Spotify"` würde eine geschlossene App
**starten**, statt sie zu fragen. Abgefragt wird alle **2 s** in einem eigenen Thread, und
gemeldet wird nur bei Änderung (`noki://spotify`) — dasselbe Muster wie beim
Fensterbeobachter. Der Renderer fragt das System nie selbst; er liest einen bekannten
Stand. Kein Konto, keine API, kein Netz. macOS fragt beim ersten Zugriff einmal nach der
Automations-Erlaubnis; wird sie verweigert, gilt schlicht „spielt nicht".

**Die Kopfhörer** sind Geometrie im Kopfraum, gebaut in `hp` — also **hinter `toHead`**.
Damit machen sie jede Kopf- und Körperdrehung, die Flugneigung und den YAW_SPIN von selbst
mit; es gibt keinen zweiten Transform und nichts im Bildschirmraum.

| | |
|---|---|
| Ohrmuschel | flache Kapsel außen über der Ohrkapsel, `\|x\| ≈ 0.30` |
| Bügel | Torus in der XY-Ebene, Radius 0.30, nur der obere Teil |
| Akzent | schmaler Ring auf der Muschel, Material `M_SCHIRM` — die vorhandene kühle blau-weiße Anzeigefläche, kein neues Material |

Der Bügel sitzt bewusst etwas **vor** den Antennen (z +0.035): auf gleicher Tiefe liefe er
mitten durch sie hindurch. Die Muscheln sitzen seitlich außen und lassen das schwarze
Display frei.

Ein- und ausgeblendet wird durch **Wachsen**: `u_zubehoer.x` skaliert alle Maße. Am
Distanzfeld ist das exakt, und optisch bauen sie sich auf, statt aufzupoppen (0.45 s).

Zustand und Zubehör sind **getrennt**: schläft Noki, während Musik läuft, bleiben die
Kopfhörer und die Schlafpose bleibt dominant. Für später weitere Accessoires reicht ein
Eintrag mehr in `zubehoer` und eine Form mehr im Shader.

## Grundstimmung

Bis Phase 3.16 konnte **niemand** einen Ausdruck zurücknehmen. `setAusdruck` wurde von
`setZustand`, den acht `ereignis()`-Fällen, den Abläufen und `umweltEreignis` gesetzt —
und dort blieb er stehen. `lob` und `jarvis_offen` setzen `gluecklich`; sobald eines davon
einmal kam, sah Noki dauerhaft überglücklich aus, obwohl STANDBY eigentlich `neutral`
trägt. Nur die Zahl `stimmung` klang ab, das Gesicht nicht.

Die Grundstimmung ist **keine zweite Ausdrucksmechanik**. Sie wählt nur, welchen
vorhandenen `EMO`-Eintrag `setAusdruck` als nächstes bekommt; Überblendung, Rig, Augen,
Lider und Antenne bleiben unverändert.

| Ausdruck | Gewicht | Verweildauer |
|---|---|---|
| neutral | 44 % | 16–46 s |
| zufrieden (leicht freundlich) | 22 % | 10–24 s |
| neugierig | 12 % | 8–18 s |
| denkend | 8 % | 8–18 s |
| gluecklich (deutlich) | 7 % | 5–12 s |
| muede | 4 % | 9–21 s |
| stolz | 2 % | 5–11 s |
| aufgeregt (sehr stark) | 1 % | 4–8 s |

Gewürfelt wird **nicht je Bild**: `setAusdruck` stempelt seinen Zeitpunkt, und erst nach
der Verweildauer fällt die nächste Wahl. Die Zuordnung steht allein in `stimmungWahl(w)` —
eine reine Funktion ohne Zufall im Rumpf, damit der Selbsttest die Grenzen mit festen
Werten prüfen kann.

Situative Ausdrücke bleiben unangetastet: ein Ereignis setzt weiterhin sofort sein
Gesicht, stempelt denselben Zeitpunkt, bekommt dadurch seine volle Standzeit — und wird
danach von der Grundstimmung sanft abgelöst statt für immer stehenzubleiben. Während
eines JARVIS-Zustands (alles außer STANDBY), eines laufenden Ablaufs und im Schlaf ruht
die Grundstimmung ganz.

---

## Schweben

Verlässt Noki seine Laufebene, hat er nichts mehr, worauf er stehen könnte. Statt ihn mit
Laufbeinen in der Luft hängen zu lassen, ziehen sich die Unterschenkel ein (0.038 → 0.006
Weltmaß) und aus den flachen Füßen werden gedrungene, gerundete **Hover-Module** mit einer
dunklen Austrittsfläche darunter. Nichts wird abgeschnitten: dieselbe Kette aus
Oberschenkel, Unterschenkel und Abschluss, nur andere Beträge — und bei `u_schweb.x = 0`
stehen exakt die alten Zahlen da, das Gehen bleibt millimetergenau wie bisher.

Darunter steht eine weiche Energieform: zwei Kerne je Modul, als geschlossene Formel im
selben Muster wie das Lichtvolumen der Brust — **kein zweiter Marsch, keine Partikel,
keine zweite Renderschicht**. Gefärbt wird mit `u_eyeCol`, also Nokis eigenem Ton.

Zwei Feinheiten, die beide erst am Bild sichtbar wurden:

* Der Schein wird **nach** dem Marsch addiert. Davor addiert verschwand er genau dort, wo
  er hingehört — der Boden wird mitgemarscht und überschrieb ihn. Auf der Figur selbst
  fällt er auf 30 %, damit die Silhouette scharf bleibt.
* Auf dem freigestellten Schreibtisch kommt die Deckung sonst nur von der Figur. Die
  Energieform trägt deshalb ihre eigene Alpha (`schwebA`), sonst wäre sie unsichtbar.

Ausgelöst wird alles über die **vorhandene** Laufebenenlogik (`inLaufZone`) — es gibt
keine zweite Ortslogik. Getragen wird nicht geschwebt: dort hängt der Körper, und das ist
eine eigene, schon gebaute Darstellung. Der Übergang läuft weich in 0.55 s hinein und
hinaus, der Takt kommt aus dem Frontend wie bei jeder anderen Bewegung: ruhiges Pulsieren
mit zwei Frequenzen, links und rechts gegeneinander versetzt, dazu ein kleiner Auftrieb
über `u_hoch`, der den Bodenschatten von selbst aufweicht.

Blick, Kopf, Antenne, Emotionen und Klickreaktionen laufen im Schweben unverändert weiter.

---

## Fliegen

Schweben heißt seit Phase 3.17 nicht mehr, an einer Stelle zu hängen. Außerhalb der
Laufebene bewegt sich Noki **aktiv** durch den Raum — mit einer eigenen Bewegungsart:

| | Gehen (auf WALK_Y) | Fliegen (darüber) |
|---|---|---|
| Antrieb | Schritte, Strecke **ist** der Fußweg | gleitender Schub, Geschwindigkeit |
| Richtung | nur waagerecht | waagerecht **und** senkrecht |
| Tempo | 0.190 Körperhöhen/s | **0.62** / **1.40** / **3.60** (≈ 3.3× / 7.4× / 19×) |
| Ziele | überall auf WALK_Y | bevorzugt **freie Flächen** |
| Hindernisse | Fenster auf der Laufebene | keine — er fliegt darüber |
| Gangzyklus | läuft | steht (`gehenZ = 0`) |

**Flugraum.** Die unterste Ebene bleibt dem Gehen vorbehalten:

```
oben  = FELD.oben + 0.20 · NOKI_HOCH
unten = FELD.unten − max(SNAP_TOLERANZ + 0.5 · NOKI_HOCH, 1.3 · NOKI_HOCH)
```

Der Abstand liegt bewusst **über** `SNAP_TOLERANZ` — sonst zöge ihn das Einrasten aus dem
Flug heraus auf den Boden. Auf dem Testgerät: Flugraum 116…865 bei WALK_Y 942. Zusätzlich
klemmt der Flug senkrecht hart bei `FELD.unten − 6`, also sicher außerhalb
`BODEN_TOLERANZ`; der Gehmodus kann damit nicht versehentlich einrasten. Wer per Hand in
den Streifen dazwischen gezogen wird, wird **nicht** hochgeschoben — sein nächstes Flugziel
liegt im Flugraum, also steigt er von selbst.

**Bewegung.** Ziel wählen → weich ansteuern → kurz vor dem Ziel abbremsen → ruhen →
neues Ziel. Beschleunigung über eine Zeitkonstante, im Ruhen treibt er langsam auf und ab.
Kein Sprung, kein Zufallszucken. Während eines JARVIS-Zustands bleibt er stehen, wo er
ist, statt davonzufliegen.

### Lebensraum: freie Flächen statt Zufallskoordinaten

Nokis Zuhause sind die **freien Bereiche** des Schreibtischs — Ränder, Lücken
zwischen Fenstern, alles, worüber gerade kein Fenster liegt. Fenster sind
**Transitbereich**, kein Aufenthaltsort: er fliegt vor, hinter oder durch sie
hindurch, aber sein Ziel liegt wieder im Freien.

Gerechnet wird ausschließlich mit den Rechtecken, die der Fensterbeobachter
ohnehin liefert — keine Bildauswertung, keine zweite Abfrage:

```
1. jedes Fenster um 0.22 Körperhöhen aufblähen (persönlicher Abstand)
2. alle Kanten im Flugraum ergeben ein Gitter aus Spalten und Zeilen
3. Zellen ohne Fenster darüber sind frei
4. Histogramm-Durchlauf je Zeile -> größtmögliche freie Rechtecke
5. zu kleine Rechtecke wegwerfen
```

Schritt 4 ist derselbe Stapel-Durchlauf wie beim größten Rechteck im Histogramm:
linear in der Gittergröße statt alle Streifen einzeln durchzuprobieren. Gerechnet
wird **nur bei der Zielwahl**, nicht je Bild.

**Mindestgröße** (Abschnitt 4 der Vorgabe): 0.95 Körperhöhen breit — Nokis
sichtbare Breite ist 0.84 — und 1.25 hoch, also Körper plus die Hover-Energie
darunter. Ein Fingerbreit Spalt ist kein Aufenthaltsort; lieber wenige gute
Flächen als viele unbrauchbare.

**Gewichtung.** Größe zählt gedämpft (Wurzel der Fläche), sonst schluckte die eine
große Fläche alle anderen. Darauf die Vorliebe für die Ränder:

| | Zuschlag |
|---|---|
| berührt linken oder rechten Rand des Flugraums | +0.90 |
| berührt den oberen Rand | +0.50 |
| echter Zwischenraum (berührt keinen Seitenrand) | +0.30 |

Die Zone, in der er gerade steht, wird zusätzlich auf 45 % abgewertet — sonst
bliebe er in der größten Fläche hängen und sähe den Rest nie. Innerhalb der
gewählten Zone fällt ein **zufälliger** Punkt mit Abstand zu ihren Kanten: dieselbe
Zone liefert nie zweimal denselben Ort, es entsteht kein Rasterlook.

Bleibt keine brauchbare Zone übrig — ein bildschirmfüllendes Fenster im
Vordergrund —, fällt er auf den alten Weg mit Zufallskoordinaten zurück. Lieber
ein Ziel im Transitbereich als gar keines.

### Hauptfenster: Transit mit Tempo

Nicht jedes Fenster ist gleich wichtig. Ein **Hauptfenster** ist ein gewöhnliches
Programmfenster (`ebene ≤ 0`), das mindestens 2.2 × 1.6 Körperhöhen misst und **≥ 6 % der
Arbeitsfläche** belegt. Kleine Overlays, Hilfsfenster, schwebende Paneele und
Hintergrundflächen sind es nicht — entschieden wird über Ebene und Fläche, nie über den
Programmnamen.

Schneidet die geplante Route ein Hauptfenster (Strecke gegen Rechteck, Slab-Verfahren —
nicht nur die Endpunkte), verschiebt sich die Tempowahl nach oben:

| | ohne Fenster | über einem Hauptfenster |
|---|---|---|
| schneller als NORMAL | 45 % | **80 %** |
| davon FLITZEN (lange Strecke) | 22 % | **35 %** |

Über Fenstern wird der Weg zugleich **direkter**: dort will er durch, nicht Kunststücke
zeigen. In freien Bereichen ändert sich nichts — dort darf er langsam bleiben, schweben
oder herumliegen. Genau das trennt Aufenthaltsraum von Transitbereich.

### Hintergrundflächen zählen als Freiraum

Manche Fenster liegen dauerhaft ganz hinten und dienen faktisch als Hintergrund.
Für Noki sind sie Teil seines Lebensraums: sie dürfen weder Hindernis sein noch
freien Raum belegen noch eine Tiefenentscheidung auslösen.

Erkannt wird über die **echte Fensterreihenfolge des Systems**, nicht über
Programmnamen. `CGWindowListCopyWindowInfo` mit `kCGWindowListOptionOnScreenOnly`
liefert die Fenster von vorn nach hinten; der Index in dieser Liste geht als
`rang` (0 = vorn) zusammen mit `ebene` (`kCGWindowLayer`) ins Frontend. Die native
Seite liefert damit nur **Tatsachen**; die Regel steht an einer Stelle im Frontend:

```
von hinten nach vorn lesen:
  solange ein Fenster  ebene <= 0  UND  >= 80 % der Arbeitsfläche bedeckt
      -> Hintergrundfläche
  beim ersten, auf das das nicht zutrifft -> Schluss
```

Ein maximiertes Safari-Fenster **im Vordergrund** bleibt dadurch ein gewöhnliches
Fenster — es ist nicht hinten. Mehrere gestapelte Hintergrundflächen werden alle
erfasst. Fehlt `rang`, gilt nichts als Hintergrund: im Zweifel lieber ein
Hindernis zu viel als eines zu wenig. Aussortiert wird in `fensterSetzen`, also an
derselben Stelle wie alle anderen Filter — es gibt **eine** Fensterliste, und was
dort herausfällt, ist danach für Hindernisse, Tiefenebene und Zielsuche
gleichermaßen nicht mehr da.

### Bahn statt Gerade

Bis Phase 3.20 zeigte die Sollgeschwindigkeit **jedes Bild direkt auf das Ziel**. Damit war
jede Strecke zwangsläufig eine Gerade — genau daher kam der steife, ferngesteuerte
Eindruck. Es gab schlicht keine andere Bahn.

Jetzt bekommt jede Strecke eine **Bahn**: eine Catmull-Rom-Kurve durch wenige Stützpunkte,
einmal in eine Polylinie abgetastet (12 Punkte je Abschnitt) und mit einer
Weglängentabelle versehen. Dadurch ist sie nach **Weglänge** parametrisiert — in der
Kurvenmitte wird er nicht langsamer, was bei roher Parameterinterpolation passiert wäre.
Keine neue Abhängigkeit: das sind dreißig Zeilen Arithmetik.

Gefolgt wird mit einem **Vorhalt**: er steuert nicht den Punkt an, auf dem er steht,
sondern einen Punkt davor — `0.30 · NOKI_HOCH + 0.10 s · Tempo`. Dadurch schneidet er
Kurven weich an, statt ihnen nachzulaufen. Der zurückgelegte Weg wird aus der
**tatsächlichen** Bewegung fortgeschrieben (Geschwindigkeit auf die Tangente projiziert);
liefe stattdessen die Sollgeschwindigkeit weiter, ränne die Bahn ihm in engen Kurven
davon.

Weil er die Kurve wirklich fliegt, **ist** seine Geschwindigkeit die Kurventangente. Die
gesamte vorhandene Haltungslogik hängt schon daran und braucht keine zweite Quelle.

### Manöver

Die Manöver unterscheiden sich **nur** darin, welche Stützpunkte gesetzt werden. Es gibt
keinen eigenen Hauptzustand je Manöver und keine zweite Bewegungsmechanik.

| | Stützpunkte | Wirkung |
|---|---|---|
| `direkt` | keine | die gerade Verbindung |
| `bogen` | einer, quer zur Verbindung | Bogen; die Kurvenlage fällt daraus ab |
| `schwung` | zwei, erst hoch, dann tief | kleine Achterbahn |
| `loop` | fünf, im Kreis um einen Punkt bei 45 % | echter Kreis in der **Ortskurve** |
| `spin` | keine (gerade Bahn) | **YAW_SPIN**: 360° um die eigene Hochachse |
| `zickzack` | 2–4, abwechselnd quer | kurze schnelle Haken |
| `steigfall` | drei, hoch–oben–tief | schnell hinauf, Richtungswechsel, hinunter |

**Gewichte** — große Figuren bleiben selten, sonst wären sie nichts Besonderes:

| Bedingung | direkt | bogen | roll | schwung | loop |
|---|---|---|---|---|---|
| kurze Strecke | 75 % | 25 % | – | – | – |
| über einem Hauptfenster | 70 % | 20 % | 10 % | – | – |
| NORMAL | 65 % | 35 % | – | – | – |
| FAST | 50 % | 32 % | – | 18 % | – |
| FLITZEN | 33 % | 26 % | 20 % | 14 % | **7 %** |

**Platzprüfung.** Der Rand bleibt frei: `0.55 · NOKI_HOCH` Abstand, sonst klemmte
`raumTakt` die Bahn ab und aus dem Bogen würde eine Schramme an der Kante. Geprüft werden
nur die *eingeschobenen* Punkte — Start und Ziel stehen fest, und Ziele liegen bevorzugt am
Rand.

Bogen und Schwung werden bei Platzmangel **gestutzt** statt verworfen: ein kleinerer Bogen
ist besser als keiner. Bleibt danach weniger als 0.35 Körperhöhen Auslenkung übrig, ist es
kein Manöver mehr, sondern eine Gerade mit Umweg — dann fällt die Wahl auf `direkt`.

Der **Loop** ist strenger. Sein Kreis reicht vom Bahnpunkt aus zwei Radien in die
Querrichtung; passt das nicht, wird die andere Seite versucht und der Radius auf den
vorhandenen Platz gestutzt. Unter 0.9 Körperhöhen Radius wird er abgelehnt — ein zu enger
Kreis ist kein Loop mehr. Außerdem darf er nicht über einem Hauptfenster liegen: dort
wirkt ein Kreis unmotiviert. Passt nichts, fällt die Wahl auf die gerade Strecke zurück —
nie auf gar keine.

Auf einem flachen Flugraum kommt der Loop deshalb schlicht nicht vor. Das ist kein Fehler,
sondern die Bedingung: er braucht Höhe.

**YAW_SPIN** ist die 360°-Drehung um Nokis **eigene, mitgeneigte Hochachse** — wie ein
Mensch, der sich einmal auf der Stelle dreht: Seite → Front → Rücken → andere Seite. Der
Wert geht als `u_flugSpin` in die Fluglage, **innen** vor der Neigung. Der Kopf bleibt auf
derselben Seite der Körperachse, die Flugbahn läuft unverändert weiter.

> Zwei falsche Achsen lagen davor. Erst `u_pose.x` — die **Nickachse**; eine volle
> Umdrehung darauf ist ein **Rückwärtssalto**. Danach `u_ort.z` — das ist eine **Weltachse
> durch Nokis Bodenpunkt**: bei geneigtem Körper beschrieb der Rumpf damit einen Kegel,
> also eine große Kreisbahn um einen Punkt außerhalb seiner selbst. Erst die dritte Fassung
> dreht um eine körpereigene Achse und um einen körpereigenen Pivot.

Er hängt am zurückgelegten **Weg**, nicht an einer Uhr: er beginnt bei 30 % der Bahnlänge,
dauert höchstens sechs Körperlängen und läuft über eine Glockenkurve weich an und aus.
Dadurch gehört er zur Bewegung, statt unabhängig davon abzulaufen. Er dreht die sichtbare
Orientierung, nicht die Bahn — der Selbsttest prüft beides getrennt (Abweichung von der
Bahn unter 0.35 Körperhöhen) und weist zusätzlich nach, dass dabei **keine** Vollrotation
auf einer Nickachse entsteht.

Ist er vorbei, wird die volle Umdrehung **weggerechnet**, nicht zurückgedreht: 2π ist
dieselbe Lage wie 0. Ohne das drehte er sich am Ende sichtbar ein zweites Mal, in die
Gegenrichtung.

### Die Achsen im Rig

Bevor irgendetwas gedreht wird, muss klar sein, was welche Achse tut. Im Shader gilt:

| | Funktion | wo angewandt | Wirkung |
|---|---|---|---|
| `rotY` | **Yaw** — Hochachse | `u_ort.z` | Front → Seite → Rücken → andere Seite |
| `rotX` | **Pitch** — Nicken | `u_pose.x` (in `toBody`) | kippt vor/zurück |
| `rotZ` | **Roll** — seitlich | `u_body.w` (in `toBody`) | legt zur Seite |

Entscheidend ist der **Angriffspunkt**: `toBody` dreht um die **Hüfte** (`hueft = (0, 0.160, 0)`)
und erfasst nur, was am Rumpf hängt — Kopf, Hals, Arme. **Die Beine gehen nicht durch
`toBody`**; sie werden getrennt über `u_leg` geschwenkt.

Genau daran lag die geknickte Flugpose: Rumpf 42° um die Hüfte nach vorn *plus* Beine 46°
nach hinten ergibt zwangsläufig ein Hohlkreuz mit herausstehendem Hinterteil. Es waren zwei
gegenläufige Teildrehungen, wo eine Gesamtausrichtung hingehört.

**Die Flugneigung ist deshalb eine Ganzkörperdrehung** (`u_flugNeig`, am Eingang von
`mapChar`, um die Körpermitte bei y = 0.30). Sie erfasst Kopf, Rumpf, Hüfte und Beine
gleichermaßen — die Figur bleibt eine Linie. `u_pose.x` trägt weiterhin Sitzen, Aufheben
und Anlehnen; der Flug fasst es nicht mehr an.

### Die Fluglage als eine Transformation

Neigung und Eigendrehung sind **eine** Transformation, definiert an genau einer Stelle im
Shader und in JavaScript gespiegelt:

```
Objektdrehung  O = Neigung(rotX) · Eigendrehung(rotY)      um KOERPER_MITTE (0, 0.30, 0)
zuFlug(p)  = O⁻¹ p    Abfragepunkt   -> ungedrehter Körperraum   (mapChar)
ausFlug(p) = O  p     Körperraum     -> Fluglage                 (Düsen, Energie)
```

Die Eigendrehung steht **innen**: sie wird von der Neigung mitgekippt und ist damit Nokis
eigene Achse, nicht die der Welt. Der Drehpunkt liegt in der Figur, also verschiebt keine
Drehung den Schwerpunkt.

**Räume und wo sie ineinander übergehen:**

| Raum | Inhalt | Übergang |
|---|---|---|
| lokal | Rig: Hüft-, Bein-, Düsenkette; Ursprung am Bodenpunkt | — |
| Fluglage | lokal + Eigendrehung + Neigung, um die Körpermitte | `ausFlug` / `zuFlug` |
| Modell | + Blickrichtung `u_ort.z` (Zuwendung zur Flugrichtung) | `rotY` am Strahl |
| Welt | + Ort auf der Ebene `u_ort.xy`, `u_hoch` | Strahlursprung |

Jeder Übergang wird **genau einmal** durchlaufen.

### Der Energiestrahl hängt am Modul

Der Strahl wird aus derselben Beinkette gebaut wie die Geometrie —
`Hüfte → Oberschenkel → Knie → Schiene → Düsenversatz → Düsenmund` — und danach mit
`ausFlug()` in die Fluglage gebracht. Genau dieser letzte Schritt fehlte: die Ganzkörper­-
neigung wirkte nur in `mapChar` auf die SDF-Abfrage, der Strahl wurde weiter im
ungedrehten Körperraum gerechnet. Die Figur kippte, der Strahl blieb stehen — und wanderte
dadurch scheinbar zum Bauch.

Auch die **Richtung** kommt jetzt aus dem Modul: Grundachse ist die Düsenachse selbst (das
Unterschenkelstück, an dem das Modul hängt), darauf blendet die beschleunigungsabhängige
Schubachse mit 55 %. Der Strahl tritt damit immer aus dem Modul heraus und läuft nie durch
Bauch, Hüfte oder Bein.

Der Selbsttest prüft das über eine **JavaScript-Spiegelung derselben Kette**: Abstand
Mund–Modul und Mund–Körpermitte bleiben über acht Lagen (Neigungen und Vierteldrehungen)
auf 10⁻⁶ konstant, und die Austrittsachse zeigt in jeder Lage vom Körper weg.

### Drei Flugwinkel außerhalb der Rig-Kanäle

Kurvenlage (`flugBank`), Manöverrolle (`flugRoll`) und entspanntes Liegen wirken **nur im
Flug** und werden erst beim Hochladen auf `u_body.w` und `u_pose.x` addiert. Sie stehen
bewusst außerhalb von `rig.body`/`rig.pose`: die dortigen Kanäle sind eng geklemmt
(`koerper.rollen` ±0.050) und je Bild ratenbegrenzt — eine ganze Umdrehung passte da nicht
hinein, und sie soll die geprüften Grenzen auch gar nicht erst berühren.

**Die Kurvenlage** kommt aus dem **Querteil der tatsächlichen Beschleunigung**: wie scharf
die Kurve ist, sagt genau dieser Wert. Auf der Geraden ist er null. Derselbe
Beschleunigungsvektor trägt mit seinem **Längsteil** das Bremsen — Kurvenlage, Abfangpose
und Schubumkehr stammen damit aus einer einzigen echten Größe.

### Keine Rücklage

Die Ganzkörperneigung ist nach hinten **hart begrenzt**: `clamp(neigZiel, −0.14, 1.20)`.
Nach vorn bleibt der ganze Weg bis zur gestreckten FLITZEN-Achse offen, nach hinten reicht
es gerade für eine Andeutung. Zwei Quellen speisten dort vorher deutlich mehr ein — das
entspannte Luftliegen (jetzt −0.12 statt −0.32) und das Abfangen beim Bremsen (jetzt
Faktor 0.22 statt 0.30). Beide sind damit sichtbar, ohne dass er je aussieht, als kippe er
um; und weil der Bremsanteil an der laufenden Bremsbeschleunigung hängt, bleibt danach
nichts stehen. Gemessen im Schweben: **0.054 rad**, nach einem FLITZEN mit Vollbremsung
**0.008 rad**.

### Ganzkörperposen schließen sich aus

Sitzen, Liegen und Schlafen sind **Ganzkörperposen**. Sie dürfen nie gleichzeitig mit einer
Flugbewegung laufen — sonst zieht eine sitzende Figur durch den Bildschirm. Genau das war
zu sehen, und die Ursache war eine Lücke in `flugAktiv()`: die Funktion fragte den Ort ab
(`inLaufZone`), aber nie die **Körperpose**. `schwebZiel()` schaltete zwar das Schweben ab,
solange `sitzU`/`liegeU` standen — der Flug lief davon unbeeindruckt weiter.

Jetzt gilt in `flugTakt`, jeden Takt, vor jeder Entscheidung:

```
sitzU > 0.02  oder  liegeU > 0.02   (und kein manueller Schwebeschlaf)
   → Wünsche zurücknehmen (sitzWunsch/liegeWunsch = null)
   → laufende Strecke beenden, keine neue beginnen
   → Sollgeschwindigkeit = 0
   → erst wenn die Pose abgebaut ist, geht es weiter
```

Der manuelle **Schwebeschlaf ist ausgenommen**: der bleibt bestehen, bis geweckt wird, und
fliegt ohnehin nicht. Die Priorität ist damit eindeutig — Schlaf, sonst Sonderpose, sonst
aktiver Flug; Emotionen, Augen und kleine Kopfreaktionen bleiben additiv.

### Entspannt in der Luft liegen

Neben dem aufrechten Schweben gibt es eine zweite Art zu ruhen (30 % der Ruhephasen): er
lehnt sich **leicht** zurück (−0.32 rad als Ganzkörperdrehung, nicht als Rumpfknick), hängt
etwas zur Seite, hebt den Kopf und lässt den Antrieb nur noch stabilisieren (−32 % Energie).
Die erste Fassung ging mit −0.85 rad viel zu weit — das sah aus, als falle er nach hinten
um, und war zusammen mit dem Hüftknick der Grund für die „zu stark zurückgelehnte"
Hoverhaltung. Das ist **kein Schlaf** — Augen offen, Glimmen normal,
Blick, Antenne und Stimmung laufen weiter; der Selbsttest prüft ausdrücklich, dass weder
`liegeU` noch `schlafU` noch der Schwebeschlaf dabei mitlaufen. Der Übergang dauert 1.3 s
in beide Richtungen.

### Start und Ankunft

Der Anlauf gibt es jetzt gestaffelt: FLITZEN holt 0.28 s aus, FAST nur 0.13 s, NORMAL
startet direkt. Bei der **Ankunft** richtet er sich wieder auf: der Bremsanteil zieht bis
zu 0.30 rad von der Vorneigung ab, während die Schubachse umschlägt — er fängt sich
sichtbar ab, statt in der Sprinthaltung stehenzubleiben.

### Kleine Abweichungen je Strecke

Damit derselbe Schnellflug nicht jedes Mal gleich aussieht, fällt beim Zielsetzen einmal
ein kleiner Satz Abweichungen: Neigung ×0.86…1.14, eine feste kleine Seitenlage ±0.06 und
ein Kopfversatz ±0.05. Alles bleibt plausibel zur Flugrichtung — es sind Variationen der
Haltung, keine zusätzlichen Bewegungen.

### Woher der steife Eindruck kam

Alle drei Ursachen lagen an derselben Stelle und waren keine Frage der Optik:

1. **Die Route war immer eine Gerade** — die Sollgeschwindigkeit zeigte jedes Bild direkt
   auf das Ziel. Andere Bahnen gab es nicht.
2. **Die Haltung hatte nur eine Eingangsgröße**, die Geschwindigkeit. Kurvenlage und
   Bremsen standen nirgends, weil es keine Kurven und kein sichtbares Abfangen gab.
3. **Ruhe hatte nur eine Form** — aufrecht schweben. Wer immer gleich dasteht, wirkt, als
   warte er auf den nächsten Befehl.

Die Erweiterung setzt genau dort an und lässt alles Übrige daran hängen: weil er die Kurve
wirklich fliegt, **ist** seine Geschwindigkeit die Kurventangente — die vorhandene
Haltungslogik brauchte keine zweite Quelle.

### Ruhen und Fliegen im Wechsel

Dauerbewegung wirkt unruhig, deshalb ist der Flug **getaktet**:

```
HOVER_IDLE ──┬─ aufrecht schwebend        (70 %)
             └─ entspannt liegend         (30 %)
   │  freie Zone wählen ─▶ Punkt in der Zone
   │  ─▶ Stufe nach Strecke UND Hauptfenster auf dem Weg
   │  ─▶ Manöver nach Stufe, Länge und Platz
   ▼
FLIGHT_NORMAL ┐
FLIGHT_FAST   ├── Bahn folgen ── abbremsen ──▶ HOVER_IDLE
FLIGHT_DASH   ┘   (oder: gleich die nächste Strecke, 30 %)
   ▲
   └── FAST/DASH beginnen mit `anlauf` (0.13 / 0.28 s Ausholen)

Manöver sind KEINE eigenen Hauptzustände: sie sind die Stützpunkte der
Bahn plus, bei der Rolle, ein Winkel entlang des zurückgelegten Wegs.
SLEEP hat Vorrang vor allem; Drag und JARVIS-Zustände halten die
Autonomie an, ohne die laufende Strecke zu zerstören.
```

| | Dauer |
|---|---|
| Ruhen | meist 2–9 s, in einem Viertel der Fälle 8–22 s |
| Flug | bis das Ziel erreicht ist, Frist als Notbremse |

Nach einem erreichten Ziel folgt **nicht** immer eine Pause: mit 30 % hängt er gleich die
nächste Strecke an (`FLUG_KETTE`). Ohne das zerfiel der Flug in lauter kurze Hopser mit
Pause dazwischen, und der Ruheanteil lag bei 63 %. Der Wert wurde mit dem FLITZEN
nachjustiert: dessen weite Ziele verlängern die einzelne Strecke, sodass weniger
Verkettung nötig ist, um im verlangten Band 35–50 % zu bleiben. Im Ruhen steht er nie starr: er
treibt auf und ab, Blick, Atem, Antenne und Stimmung laufen weiter.

**Drei Tempostufen.** Das Tempo fällt **einmal je Strecke**, nie unterwegs:

| | Tempo | Anziehen | Bremsweg | wann |
|---|---|---|---|---|
| NORMAL | 0.62 Körperhöhen/s | τ 0.45 s | 1.7 · NOKI_HOCH | immer möglich |
| FAST | 1.40 Körperhöhen/s | τ 0.32 s | 3.2 · NOKI_HOCH | ab mittlerer Strecke |
| FLITZEN | 3.60 Körperhöhen/s | τ 0.22 s | 4.5 · NOKI_HOCH | nur auf langen Strecken |

Gemessen auf dem Gerät: **33 / 76 / 194 Punkte/s** — Gehen liegt bei 30. Die Relation ist
also `GEHEN << NORMAL < FAST << FLITZEN`, jede Stufe rund 2.3–2.6-mal so schnell wie die
darunter. Schnell heißt **nicht abrupt**: jede Stufe zieht über eine Zeitkonstante an und
bremst über einen längeren Weg wieder ab.

**Wann geflitzt wird.** Die Auswahl hängt an der Strecke *und* am Zufall — sonst würde
jede lange Strecke zum Sprint und das Besondere ginge verloren:

```
kurz    (< 45 % der Langstrecke)  ->  82 % NORMAL, 18 % FAST
mittel                            ->  55 % NORMAL, 45 % FAST
lang                              ->  22 % FLITZEN, 42 % FAST, 36 % NORMAL
```

„Lang" ist dabei nicht starr an Nokis Größe gebunden, sondern **gedeckelt durch den
Flugraum**: `max(3.5 · NOKI_HOCH, min(8 · NOKI_HOCH, Diagonale · 0.50))`. Auf einem
kleinen Bildschirm läge die Schwelle sonst jenseits der längsten möglichen Strecke, und es
würde nie geflitzt — genau das zeigte der erste Testlauf. Im Betrieb sind rund **10–20 %
der Strecken** ein Sprint.

**Ausholen und Bremsen.** Vor dem FLITZEN steht eine kurze Vorbereitung von 0.28 s
(`anlauf`). Sie ist keine gespielte Startanimation: er nimmt in dieser Zeit Schwung
**gegen** die Zielrichtung, und die Vorbereitungspose entsteht daraus von selbst — aus
echter Bewegung, wie alles andere auch. Danach zieht er über τ 0.22 s an. Vor dem Ziel
bremst er über 4.5 Körperhöhen ab; der Überschuss bleibt unter einer Körperhöhe.

**Haltung nach Stufe** (gemessener Beitrag, ohne Atem und Gewicht):

| | NORMAL | FAST | FLITZEN |
|---|---|---|---|
| Rumpfneigung `pose[0]` | 0.12 | 0.26 | 0.74 |
| Körperdrehung zur Flugrichtung | 0.58 | 0.76 | 1.23 |
| Hovermodule nach hinten `leg` | 0.17 | 0.38 | 0.80 |
| Strahllänge `schubLang` | 0.15 | 0.22 | 0.37 |

Im FLITZEN steht der Körper damit fast in der **Seitenansicht** (1.23 von 1.30 rad) und
weit nach vorn gelegt — erst diese Kombination liest sich als Streckung *in* die
Flugrichtung. Zwei Dinge waren dafür nötig, beide erst am gerenderten Bild sichtbar:

* Bei nur halber Drehung kippte die starke Neigung ihn zur **Kamera** statt in die
  Flugrichtung; er sah aus, als schlage er vornüber. Die Drehung musste mit.
* Rumpf- und Kopfdrehung **addieren** sich. Mit voll mitgedrehtem Kopf schob sich das
  Gesicht über die Seitenansicht hinaus, und man sah nur noch den Hinterkopf. Der Kopf
  bekommt deshalb ein Budget: `(DREH_SEITE − |Rumpfdrehung|) / (DREH_SEITE · 0.6)`. Bei
  ruhigem Flug ändert das fast nichts (Faktor 0.92), im FLITZEN geht sein Anteil auf
  nahezu null zurück.

**Nicht die Schlafpose.** FLITZEN und Liegen benutzen dieselbe Rumpfachse, sind aber
getrennte Zustände: im Schlaf steht `manuellSchlaf`/`liegeU`, es gibt keine Translation,
die Augen sind zu und der Antrieb im Standby. Im FLITZEN sind Augen und Ausdruck aktiv,
die Beschleunigung ist maximal und der Antrieb arbeitet am stärksten. Der Selbsttest prüft
ausdrücklich, dass im FLITZEN weder `liegeU` noch `schlafU` noch der Schwebeschlaf
mitlaufen.

**Enge Kanäle.** `koerper.rollen` (±0.050) und `koerper.x` (±0.015) bekommen einen eigenen,
**gesättigten** Treiber (`flugPose.rx`, auf ±1 begrenzt, bevor er geglättet wird). Der
volle FLITZ-Wert von 5.8 schlüge dort ohnehin nur an die Klemme, würde aber beim
Richtungswechsel die erlaubte Änderung je Bild sprengen — der Selbsttest meldete genau das
als Ruck. Die Neigung darunter benutzt weiterhin den vollen Wert; sie hat den Platz dafür.
Auch die Flugdrehung ist auf `DREH_RATE` gedeckelt: beim Richtungswechsel im FLITZEN
springt ihr Ziel sonst von −1.23 auf +1.23.

**Haltung.** Alles kommt aus dem Flugvektor, nichts aus einer zweiten Richtungslogik:

| Größe | Quelle | Wirkung |
|---|---|---|
| `raum.dreh` | `vx` | Körper dreht in die Flugrichtung, höchstens 0.45 · Seitenansicht |
| `body[3]` | `vx` | legt sich in die Kurve (wie beim Anlehnen) |
| `pose[0]` | Tempo **und** `vy` | zieht nach vorn, Nase hoch beim Steigen, runter beim Sinken |
| `head[1]` | `vx` | Kopf sieht voraus |
| `head[0]` | Tempo und `vy` | Kopf folgt der Senkrechten |

Die Haltung liest den Flugvektor **geglättet**, der Antrieb dagegen sofort. Rumpf
τ 0.70 s (`FLUG_POSE_TAU`), Kopf τ 0.35 s (`FLUG_KOPF_TAU`) — der Kopf ist damit weiterhin
sichtbar schneller als der Rumpf. Ohne diese Trennung sprang `koerper.rollen` beim
Anziehen des Schnellflugs um 0.018 je Bild und damit weiter als jede andere Bewegung des
Rigs; der Selbsttest meldete es als Ruck. Energie und Schubachse bleiben ungefiltert: die
dürfen sofort reagieren.

Die Körperdrehung wird **nachgeführt**, nicht gesetzt (`flugDreh`, τ 0.55 s), und läuft
danach durch den vorhandenen Nachlauf `raum.idreh` (τ 0.35 s). Ohne diese erste Glättung
fiel das Drehziel beim Ende des Flugs in einem Bild von ±0.59 auf 0 zurück — ein
sichtbarer Ruck, den der Selbsttest als Drehsprung meldete. Der Kopf hängt direkt an der
Geschwindigkeit und reagiert dadurch sichtbar schneller als der Rumpf.

**Mausblick.** Bleibt vollständig erhalten. Nur sein Anteil am **Kopf** sinkt mit dem
Flugtempo — Bezug ist seit der dritten Stufe das FLITZ-Tempo, nicht mehr das normale:
`mwKopf = mw · (1 − 0.80 · |v| / Flitztempo)`. Im ruhigen Flug bleiben damit 86 % des
Zeigereinflusses (vorher waren es 45 %), im Schnellflug 69 %, im FLITZEN nur noch 20 % —
langsam gehört der Kopf dem Benutzer, schnell der Flugrichtung. Die **Augen** behalten
ihren vollen Anteil und folgen dem Zeiger auch im Sprint.

**Antrieb.** Aus dem Fuß wird eine **Düsengondel**: die flache, breite Sohle
(0.074 × 0.046 × 0.092 Halbmaß) wird zu einem aufrechten Körper (0.036 × 0.049 × 0.036) —
halb so breit, höher als tief. Dadurch verschwindet die Schuhsilhouette, und der
Unterschenkel (0.038 → 0.004) läuft sichtbar *in* das Modul hinein, statt darauf zu
stehen. Darunter sitzt eine dünne, etwas weiter ausgestellte Scheibe als Düsenmund.
Schienbein und Düsenversatz stehen als `SCHIENE`/`DUESENVERSATZ` an genau einer Stelle im
Shader — Geometrie und Energieform lasen vorher verschiedene Zahlen, und der Strahl saß
nicht am Modul.

Der Strahl ist **gerichtet**, nicht rund. Vorher standen dort zwei kugelsymmetrische
Gaußkerne — die lesen sich zwangsläufig als Leuchtkreis um die Füße. Jetzt liegen fünf
Stützpunkte auf einer **Achse** unter der Düse: nach unten wird der Radius größer
(0.020 → 0.078), die Gewichtung kleiner ((1−s)^0.85) und die Farbe blauer — weißer Kern
`(0.90, 0.96, 1.00)` am Mund, blauer Auslauf `(0.16, 0.44, 1.00)` am Ende. Weiterhin
geschlossene Formel, kein zweiter Marsch, keine Partikel.

In der **Kurve** arbeitet das kurvenäußere Modul bis zu 22 % stärker, beim entspannten
Liegen gehen beide um ein Drittel zurück. Auch daran liest sich die Lage ab, ohne dass
etwas gespielt wäre.

Die Achse `u_schub` kommt aus dem Frontend und wirkt der Bewegung **entgegen**: nach
rechts fliegen heißt nach links unten blasen. Steigen macht sie kräftiger, Sinken
kompakter, und ihre Länge wächst mit dem Tempo (0.15 / 0.22 / 0.37 Weltmaß). Die Energie
steigt mit dem Tempo (Faktor 1.5 / 2.0 / 2.6), das kurvenäußere Modul arbeitet stärker,
Steigen kostet mehr als Sinken.

**Beim Bremsen dreht die Achse um.** Sie folgt nicht der Geschwindigkeit, sondern dem
Anteil der **tatsächlichen Beschleunigung**, der der Bewegung entgegensteht — gemessen aus
zwei aufeinanderfolgenden Bildern, nicht aus einer gespielten Kurve. Wer verzögert, drückt
nach vorn; wer beschleunigt, nach hinten. Dieselbe Rechnung trägt auch den Start aus dem
Ausholen heraus, wo die Geschwindigkeit ihr Vorzeichen wechselt.

### Tiefenebenen

Im Flug liegt Noki nicht immer auf derselben Ebene gegenüber fremden Fenstern:

```
vorn   — er bleibt vor den Fenstern sichtbar
hinten — ein Fenster darf ihn verdecken; sein Ort läuft darunter
         unverändert weiter, er taucht drüben wieder auf
```

Entschieden wird **an der Fensterkante**, nicht irgendwann unterwegs. Solange er im
Freien ist, schaut er 0.45 s voraus (`FLUG_VORSCHAU`, also Tempo × Zeit); trifft dieser
Punkt ein Fenster, das er noch nicht entschieden hat, fällt **jetzt** die Wahl:

```
draußen, Vorausschau trifft Fenster 5   →  würfeln: 45 % hinten, 55 % vorn,
                                            Fenster 5 vormerken
im Fenster 5                            →  gebunden, keine neue Wahl
draußen, Vorausschau frei               →  vorn, Vormerkung löschen
```

Die Bindung an die **Fenster-ID** ist der Kern: solange er im selben Fenster steckt, wird
nichts neu gewürfelt, und die Ebene kann nicht mitten im sichtbaren Fensterbereich
umspringen. Mehrere Fenster hintereinander bekommen jeweils eine eigene Entscheidung.
Gerät er ohne Entscheidung in ein Fenster — weil es über ihm aufging —, bleibt er vorn und
damit sichtbar.

Umgesetzt über dieselbe Umschaltung wie beim Durchgehen auf der Laufebene
(`noki_ebene_durchgang`). Sie wird jedes Bild **neu angewendet**, aber nicht neu gewählt —
dadurch heilt sich die Ebene selbst, falls etwas anderes sie zwischendurch zurücksetzt.
Während eines JARVIS-Zustands bleibt er vorn, im Schlaf bleibt alles, wie es ist, und
`flugAus()` stellt die Ebene in jedem Fall wieder her.

Der Selbsttest fliegt sechs Mal durch ein Testfenster und prüft: Entscheidung fällt
außerhalb oder auf der Kante, **null** Wechsel mit Abstand innerhalb des Fensters, beide
Ebenen kommen vor, ohne Fenster steht er wieder vorn.

Das gilt unverändert im **FLITZEN**. Die Vorausschau ist eine Zeit, keine Strecke
(`FLUG_VORSCHAU` = 0.45 s), wächst also mit dem Tempo mit: bei 194 Punkten/s fällt die
Entscheidung rund 87 Punkte vor der Kante. Beim Durchqueren läuft der Ort ununterbrochen
weiter — bei 60 Bildern je Sekunde sind das 3.2 Punkte je Bild, also kein Sprung, sondern
eine schnelle, aber lückenlose Bewegung hinter dem Fenster hindurch.

Ausgelöst wird alles über dieselbe `inLaufZone()` wie das Schweben; es gibt keine zweite
Ortslogik. Auf WALK_Y ist der Flug aus, Snap und Dragging bleiben unberührt.

---

## Klickreaktionen

Gewichtet, mit Sperre über die letzten zwei: B1 Umsehen (2.6), E2 Nicken (2.4), E5
Nachfragen (1.6), C1 Winken (1.1), D3k Überraschung (0.7). Schläft er, ist Aufwachen die
Reaktion.

### Doppelklick: schlafen legen

Ein Doppelklick legt Noki schlafen — und zwar **dort, wo er gerade ist**:

| Ort | Schlaf |
|---|---|
| auf WALK_Y | der vorhandene Bodenschlaf: hinlegen, Augen zu, Glimmen herunter |
| im Flugraum | **Schwebeschlaf**: er bleibt in der Luft stehen |

Der Schwebeschlaf ist kein zweiter Zustand neben dem Flug, sondern ein Aufsatz darauf:
`flug.phase` steht auf *ruhen*, die Sollgeschwindigkeit ist ein sehr langsames Heben und
Senken, der Antrieb geht auf einen gleichmäßigen Standby (Energie ≈ 0.80, Strahl kurz und
senkrecht). Kopf gesenkt, leicht zur Seite geneigt, Arme entspannt, Augen zu. Er fällt
**nicht** auf die Laufebene und sucht sich kein Ziel mehr.

Der Schlaf hat **Vorrang vor der Autonomie**: `autoTakt` steigt sofort aus, die Stimmung
bleibt stehen, der Mausblick verstummt. Beendet wird er nur durch eine Interaktion —
erneuter Doppelklick oder Aufheben. Ein JARVIS-Zustand weckt ihn nicht.

Das frühere Verhalten des Doppelklicks (verbergen) liegt jetzt im Menüleisten-Symbol
unter **„Noki verbergen"** — es ging sonst ersatzlos verloren.

**Keine Sprechblasen.** Noki sagt von sich aus keinen Text. Die früher diskutierten
Sprechblasen sind verworfen — sie hätten die Haltungsregel aus [06](06-interaktion-und-verhalten.md)
gebrochen, sobald ein Satz über die Anwesenheit des Benutzers gesagt hätte.

---

## Erscheinen und Verschwinden

Eine Kugel wird zu Noki. Bewusst **keine** zweite Marschschleife und kein zweites
Distanzfeld: die vorhandene Figur wird um ihren Bodenpunkt skaliert — eine gleichförmige
Skalierung ist am Distanzfeld exakt `p/s` und `d·s` — und darauf wird die geschlossene
Kugelformel geblendet. Bei `u_spawn = 1` kostet das genau einen Vergleich je Abtastpunkt,
und `mapChar` ist Zeile für Zeile die alte Funktion.

```
0.00 s   Lichtpunkt
0.20 s   Kugel wächst
0.55 s   daraus formt sich der Körper
1.25 s   Noki steht
danach   kurzes Winken (C1)
```

Verschwinden ist dieselbe Blende rückwärts, in 0.50 s. Danach wird auch nicht mehr
gezeichnet: ein verborgenes Fenster kostet keine Bildrate.

Der Ort wird zweimal gemerkt: die **Fensterlage auf dem Schreibtisch** (native Seite, in
`app_config_dir/fensterort.json`) und **Nokis Stelle auf seiner Ebene** (`localStorage`).
Beide werden beim Wiederkommen geklemmt — auf den nächsten sichtbaren Bildschirm und auf
die nächste gültige Stelle der Ebene. Noki erscheint nie außerhalb des Sichtbaren.

---

## Angeschlossene Quellen

| Kanal | Quelle | Rate |
|---|---|---|
| `noki://state` | `$JARVIS_RUNTIME_DIR/visual.state` | 10 Hz |
| `noki://voice` | `$JARVIS_RUNTIME_DIR/visual.voice` | 30 Hz |
| `noki://umwelt` | `$JARVIS_RUNTIME_DIR/jarvis-core.heartbeat` (mtime) | 1 Hz |
| `noki://zeigen` | Menüleiste | – |

Der **Sprechpegel ist echt**. `lib/voice.sh` lässt `lib/visual_envelope.py` die WAV vor dem
Abspielen analysieren und veröffentlicht daraus 30-mal je Sekunde einen normierten Wert.
Die native Seite liest ihn und reicht ihn an `window.NokiStimme.pegel()` weiter — dieselbe
Schnittstelle wie vorher. Es wird kein Audio analysiert und kein Mikrofon geöffnet. Bleibt
der nächste Wert aus, fällt die Darstellung nach ihrer Frist von selbst auf die eigene
Silbenkurve zurück.

Für Umgebungsereignisse gibt es einen **Adapter mit festem Wortschatz**
(`window.NokiUmwelt`): `jarvis_offen`, `jarvis_weg`, `fenster_neu`, `objekt_nah`, `weg`.
Angeschlossen ist heute nur das Lebenszeichen des Cores. Fensterbeobachtung, Browser-Tabs,
Bildschirmaufnahme und Accessibility sind **bewusst nicht gebaut**.

---

## macOS-Grenzen

**Schreibtische (Spaces).** Ein `NSWindow` gehört normalerweise zu dem Space, auf dem es
erzeugt wurde — genau deshalb war Noki lange nur auf einem Schreibtisch zu sehen. macOS
kennt dafür ein einzelnes Bit im Collection Behavior:
`NSWindowCollectionBehaviorCanJoinAllSpaces`. tao setzt über
`set_visible_on_all_workspaces` **ausschließlich dieses Bit** und lässt alle übrigen Flags
unangetastet — es gibt also keine zweite Instanz, kein Polling und keine Space-Erkennung.

Die Einstellung heißt in der Menüleiste **„Auf allen Schreibtischen"**, liegt bei den
Ebenen und ist ohne Neustart wirksam. Gespeichert wird sie als `alle_schreibtische` in
`app_config_dir/einstellungen.json`; fehlt der Wert, gilt **`true`** — ein Begleiter, der
beim ersten Space-Wechsel verschwindet, ist keiner.

> **Space-Zugehörigkeit ≠ Fensterebene.** `set_always_on_top`/`-bottom` ändern unter macOS
> nur `setLevel`, `set_visible_on_all_workspaces` nur `collectionBehavior`. FRONT/BEHIND
> und die Schreibtischwahl können sich deshalb nicht gegenseitig überschreiben; ein
> Rust-Test hält beide Zustände gleichzeitig und prüft, dass keiner den anderen verändert.

**Vollbild.** Das gesetzte Bit gilt für gewöhnliche Schreibtische. Ein fremder
Vollbild-Space ist ein Sonderfall: dafür wäre zusätzlich
`NSWindowCollectionBehaviorFullScreenAuxiliary` nötig, und ob das trägt, hängt an
Aktivierungsrichtlinie und Fensterebene. Bewusst **nicht** gesetzt — die Vorgabe war,
normale Schreibtische zuverlässig zu bedienen und keine fragilen Hacks für den
Vollbildfall zu bauen.

**Mission Control** zeigt dieselbe eine Fensterinstanz; `canJoinAllSpaces` erzeugt keine
Kopien, sondern ordnet dasselbe Fenster jedem Space zu.

**Menüleiste statt Control Center.** Drittanbieter können kein Control-Center-Modul
stellen. Das Symbol liegt in der normalen Menüleiste (`NSStatusItem` über Tauris Tray). Sie
ist im Desktop-Modus zugleich Nokis **einzige** Bedienoberfläche: Namensschild,
Zustandszeile und Studioleiste sind dort per CSS abgeschaltet, sichtbar ist ausschließlich
die Figur.

Das Menü ist bewusst **eine** Liste mit einer einzigen Trennung vor dem Beenden. Vorher
zerlegten drei Trennlinien es in vier Blöcke, und das las sich wie mehrere gestapelte
Flächen — „Fenster im Fenster". Die Zusammengehörigkeit der drei Ebenen trägt schon ihr
Häkchen; dafür braucht es keine eigene Kammer.

```
Noki zeigen
Noki verbergen
✓ Im Vordergrund
  Wie ein Fenster
  Im Hintergrund
✓ Auf allen Schreibtischen
──────────────
JARVIS beenden
```

**Kein Programmpaket über die Tauri-CLI.** Sie ist hier nicht installiert. `bauen.sh`
stellt `JARVIS.app` aus derselben Binärdatei zusammen, die cargo baut, und signiert sie
ad hoc. Das ist ein echtes, startbares Programmpaket, ersetzt aber keine Signierung mit
Entwicklerzertifikat und keine Notarisierung — für die Weitergabe an andere Rechner wäre
`cargo tauri build` der richtige Weg.

**macOSPrivateApi.** Für ein durchsichtiges Fenster nötig. Bekannte Folge: die App ist
damit nicht App-Store-fähig.
