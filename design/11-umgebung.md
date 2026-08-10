# 11 · Die Umgebung — Boden, Wände, Räume, Durchgänge

> Fünf Räume auf zwei Ebenen, fünf Durchgänge, eine Treppe. Bewusst
> minimalistisch: eine saubere räumliche Grundlage, kein fertiges Zuhause.
> Keine Möbel, keine Dekoration, keine Agentenlogik.
>
> Alles hier Beschriebene läuft in `noki.html`. Die Zahlen sind aus dem Code
> abgeschrieben, nicht umgekehrt.

---

## Die eine Tatsache, die alles bestimmt

Vor dieser Stufe hatte Noki **keinen Ort**. Gehen war ein Laufband: `bodenZ`
zählte, wie weit der abstoßende Fuß wandert, und verschob damit die Phase eines
Streifenmusters am Boden. Der Kommentar im Shader sagte es selbst — *„ohne sie
wäre ein Gang auf der Stelle von Stillstand nicht zu unterscheiden."* Es gab
keine Position, keine Blickrichtung, keine Kollision und keine Weltgeometrie
außer einer unendlichen Ebene.

„Begehbar" heißt deshalb zwangsläufig, dass Noki eine Position und eine
Blickrichtung bekommt. Das ist der einzige unvermeidbare Eingriff dieser Stufe.
Alles Übrige liegt additiv daneben:

* **`mapChar` ist Zeichen für Zeichen unverändert.** Die Figur wird nicht
  umgebaut, sondern nur mit einem anderen Punkt aufgerufen —
  `mapSzene(p) = opU(mapChar(zuFigur(p)), mapWelt(p))`. Eine Umgebung, die die
  Figur nicht anfasst, kann auch keine Bewegung beschädigen.
* **`#welt=0` stellt die alte leere Bühne wieder her.** Damit bleibt jedes
  bestehende Referenzbild reproduzierbar.

---

## Grundriss

Fünf Räume auf **zwei Ebenen**, kein Riegel mehr. Der Hauptbereich ist selbst
L-förmig, der Werkraum liegt 0.36 tiefer und wird über eine Treppe erreicht.

```
   z
   ↑
 3.0 ┌────────────┬────────────────────────────┐
     │  bedroom   ▯                            │
     │ 3.2 × 1.6  │        main_room           │
 1.4 ├────────────┼──────▯─────┐   L-förmig    │
     │ werkstatt  ▯   flur      │   13.6 E²     │
     │ 3.2 × 2.0  │ 2.2 × 2.0  │               │
     │  −0.36 !   │            │               │
-0.6 └────────────┴────────────┼──────▯────────┤
                               │    room_3     │
                               │  2.8 × 2.0    │
-2.6                           └───────────────┘
    -4.8       -1.6          0.6              3.4  → x
```

| Kennung | Name | Ebene | Fläche | im Menschmaßstab |
|---|---|---|---|---|
| `main_room` | Hauptbereich (L) | 0 | 13.6 E² | 42 m² |
| `room_3` | Nebenraum | 0 | 5.6 E² | 17 m² |
| `bedroom` | Schlafzimmer | 0 | 5.1 E² | 16 m² |
| `werkstatt` | Werkraum | **−0.36** | 6.4 E² | 20 m² |
| `flur` | Flur | 0 | 4.4 E² | 13 m² |

**Zusammen 35 E² — im Menschmaßstab 107 m².** Rechnet man `1.0 = 1.75 m`, sind
das die Maße einer echten Wohnung; die Räume sind damit im selben Verhältnis
zur Figur wie beim Menschen zum Zimmer.

Fünf Durchgänge, zwei davon zwischen Flur und Hauptbereich — daraus wird ein
Rundweg statt lauter Sackgassen.

---

## Volumen: die Höhe, nicht die Fläche

Die Grundfläche stimmte längst — 107 m² im Menschmaßstab. Trotzdem wirkte die
Wohnung flach, und die Rechnung sagt genau warum:

| | Mensch (1.75 m) | vorher | jetzt |
|---|---|---|---|
| Raumhöhe | 2.50 m = **1.43** Körperhöhen | 0.62 | **1.43** |
| lichte Türhöhe | 2.00 m = **1.14** | — (Loch bis zur Oberkante) | **1.14** |
| Grundriss | — | stimmt | **unverändert** |

Die Wand stand auf **43 % der proportionalen Höhe**. Das Anheben auf `1.43`
ändert **kein einziges Grundrissmaß**, verdreifacht aber beinahe das Volumen des
Hauptraums (8.4 → 19.4 E³). Noki wird dabei *nicht* zum Winzling: sein
Verhältnis zum Raum bewegt sich auf das Menschmaß **zu**, nicht davon weg.

**Die Türbreite bleibt bei `0.85`.** Proportional wären `0.51` — aber Nokis
Schultern messen 41 % seiner Höhe, beim Menschen sind es 26 %. Eine
proportionale Tür ließe ihm 5 % je Seite und wäre unpassierbar. Proportional ist
hier also die Höhe, nicht die Breite; alles andere hieße, die Figur zu ignorieren.

Über jeder Öffnung sitzt seit dieser Stufe ein **Sturz** von `1.14` bis zur
Wandoberkante. Erst er macht aus einem Loch in der Wand eine Tür.

> **Und er hat eine Falle aufgestellt:** `frei()` rechnet nur in der Ebene und
> kennt keine Höhe. Ohne Gegenmaßnahme lag der Sturz als Wand quer über jedem
> Durchgang, und Noki kam durch **keine einzige Tür**. Der Selbsttest hat es
> sofort gemeldet. Die Marke `ueberKopf` wird deshalb **gerechnet, nicht
> gesetzt**: sie gilt genau dann, wenn `TUER_H >= NOKI_H` — wenn er also
> wirklich darunter durchpasst.

---

## Die Treppe — bemessen nach seinem Bein, nicht nach dem Bauwesen

Eine im Menschmaßstab proportionale Stufe (0.17 m bei 1.75 m Körperhöhe)
entspräche bei Noki **0.097 — also 117 % seiner gesamten Beinlänge** von 0.083.
Er könnte sie so wenig begehen wie ein Mensch einen meterhohen Absatz.

Gebaut ist sie deshalb nach dem, was sein Körper hergibt:

| | |
|---|---|
| Steigung | **0.045** = 54 % seiner Beinlänge |
| Auftritt | 0.16 |
| Stufen | 8, zusammen 0.36 |
| hinunter | mit dem vorhandenen Gang |
| hinauf | mit dem Hüpfer — sein Scheitel liegt bei 0.060, also über der Stufe |

Damit der Hüpfer ihn hinaufträgt, darf er im Flug vorwärts fahren
(`HUEPF_SCHUB`); ohne das endete er per Endpunktgleichheit genau dort, wo er
begann. Die Stufen stehen im Distanzfeld, sind für die Kollision aber
**Fußboden und kein Hindernis** — sonst stünde die Treppe als Wand im
Durchgang.

---

## Zwei Ebenen

Jeder Raum trägt eine Höhe `y`. `welt.y` folgt der Bodenhöhe an seinem
Standort mit fester Rate, damit eine Stufe als Tritt liest und nicht als Ruck.
Angehoben wird an genau zwei Stellen — `rig.body[1]` und `rig.hueft[1]` —,
alles Übrige rechnet weiter relativ zum örtlichen Fußboden, auch die
Bodenbedingung des Liegens. Der analytische Boden schneidet gegen jede Ebene
einzeln, von oben nach unten; der erste Treffer, dessen Ebene zum Raum an
dieser Stelle passt, ist der sichtbare.

Eine Wand zwischen zwei Ebenen steht auf dem tieferen Boden und reicht über
den höheren hinaus — das ergibt sich aus der Ableitung, ohne Sonderfall.

---

## Eine Tabelle, zwei Seiten

`RAUM` und `TUER` sind die einzige Wahrheit über die Umgebung. `waende()` leitet
daraus die Liste achsparalleler Quader ab — eine durchgehende Wand wird an einer
Tür in zwei Stücke geteilt, die Öffnung entsteht also durch **Weglassen**, nicht
durch Abziehen. Aus derselben Liste entstehen:

* der **GLSL-Text** der Wände (`weltGLSL()`), zur Ladezeit in den Shader
  eingesetzt — `FRAG` ist ohnehin ein JS-String;
* die **Kollisionsprüfung** im JS (`frei`, `schiebe`).

Wären es zwei gepflegte Listen, liefen sie auseinander, und Noki bliebe an einer
Wand hängen, die man nicht sieht — oder liefe durch eine, die man sieht.

---

## Tempo — und was es kostet

Nokis Bein misst **8.3 % seiner Körperhöhe**; beim Menschen sind es 49 %. Seine
Schrittlänge ist damit 6.2-mal kürzer. Menschentempo (0.80 Körperhöhen je
Sekunde) bräuchte bei diesem Schritt **12.3 Schritte je Sekunde**.

`WELT_GANG = 3` löst das anders: die Weltstrecke ist das Dreifache des
Fußwegs. Das ergibt 0.83 Körperhöhen je Sekunde — genau Menschentempo — bei
unverändertem Gangbild.

**Der Preis ist ausdrücklich zu nennen:** damit ist die Rutschfreiheit
aufgegeben. Bis dahin galt, dass Noki um genau den Weg vorrückt, den sein
abstoßender Fuß zurückgelegt hat; der Selbsttest hat das über drei Stufen
hinweg zugesichert. Jetzt rutscht er sichtbar. Das war eine bewusste
Entscheidung gegen die Alternativen (12 Schritte je Sekunde, oder ein deutlich
anderes Gangbild mit weit ausholenden Schritten). Die alte Prüfung heißt
deshalb nicht mehr „rutscht nicht", sondern prüft nur noch die innere
Stimmigkeit des Gangmusters — und der Faktor steht im Selbsttest-Bericht,
damit ihn niemand übersieht.

---

## Türen, die er selbst öffnet

Jeder Durchgang hat ein **Türblatt** an einem Band. Es ist ein dünner Quader
(`0.035`), der im Shader um seine senkrechte Bandachse gedreht wird — gedreht
wird dabei nicht der Quader, sondern der Abfragepunkt, und zwar rückwärts:
steht das Blatt um `+a` offen, liegt der Punkt im Blattsystem bei
`rotY(p − band, −a)`. Maße und Winkel kommen aus **derselben `TUER`-Tabelle**,
aus der auch die Kollision ihre Sperre baut; eine Tür, die man sieht, aber nicht
spürt, wäre schlimmer als keine.

**Der Ablauf ist eine einzige Fortschrittsachse `tuerU`.** Der vordere
Abschnitt (42 %, also 0.50 s) gehört der Hand, der hintere dem Blatt: das Blatt
setzt sich erst in Bewegung, wenn die Hand daran ist. Andersherum sähe es aus,
als ginge die Tür von allein auf und er langte hinterher.

* Kommt Noki einer geschlossenen Tür auf `0.55` nahe **und liegt sie vor ihm**,
  hält er an und greift danach. Ohne die zweite Bedingung greift er nach jeder
  Tür, an der er dicht vorbeigeht.
* Gebremst wird über `gS`, damit **Beinschwung und Vorrücken zusammen**
  aufhören. Nur eins von beidem zu stoppen ergäbe Gehen auf der Stelle oder
  Gleiten ohne Schritt.
* Der Arm läuft **additiv über die vorhandenen Kanäle** `armSw`/`armFw`, in
  derselben Klammer wie Sitz- und Hüpfversatz — er nutzt die Stehgrenzen, statt
  sie aufzuweichen. Es trifft nur **eine** Hand: die, auf deren Seite die Klinke
  sitzt, also am freien Blattrand gegenüber dem Band.
* Ein angefangener Griff wird zu Ende geführt, auch wenn der Joystick losgelassen
  wird. Auf halbem Weg abzubrechen hieße, die Hand in der Luft stehen zu lassen.
* Hinter ihm fällt die Tür wieder zu, sobald er `1.10` entfernt ist.

Beide Kanäle sind an beiden Enden **bitgleich null** — `cWeich(0) = 0` macht den
Anfang, `cWeich(1) = 1` das Ende. Dieselbe Zusicherung wie beim Hüpfen.

---

## Kollision

Kreis gegen achsparallele Rechtecke, kein Physiksystem. Geht der volle Schritt
nicht, wird jede Achse einzeln versucht — dadurch **gleitet** Noki an einer Wand
entlang, statt davor stehen zu bleiben. Sind beide Achsen blockiert, bleibt er
stehen: ein Ausweichen „irgendwohin" wäre ein Sprung, und ein Sprung durch eine
Wand ist schlimmer als ein Halt.

Drei Dinge im Distanzfeld sind **kein** Hindernis und fallen heraus:

| | warum |
|---|---|
| Treppenstufen (`stufe`) | Fußboden, nicht Wand |
| Türstürze (`ueberKopf`) | Kopffreiheit — `frei()` kennt keine Höhe |
| offene Türblätter (`auf ≥ 0.5`) | die Öffnung ist frei |

Das Türblatt sperrt bewusst **nicht** als gedrehtes Rechteck, sondern als die
lichte Öffnung: halb offen ist keine halbe Tür. Noki soll erst hindurch, wenn sie
wirklich aufgeschwungen ist.

---

## Darstellung

* **Der Boden bleibt analytisch geschnitten.** Läge er im Distanzfeld, stünde die
  Kamera beim Neigen nach unten irgendwann darunter, der erste Abtastpunkt läge
  im Boden und die Figur verschwände.
* **Der Sichtachsen-Freischnitt.** Mit `0.62` hohen Wänden sah man über alles
  hinweg — das war der ganze Grund für die niedrige Bauweise, kein Stilentscheid.
  Auf Menschmaß gebracht verdeckt die vordere Wand die Figur zwangsläufig.
  Statt die Kamera umzubauen, überspringt der Marsch **Wandwerkstoff entlang der
  Sichtlinie**: ein rundes Loch genau dort, wo die Figur sonst verschwände.

  Es ist eine **Röhre** von festem Weltradius (`0.90`), kein Kegel mit der Spitze
  in der Kamera. Der Kegel liegt näher, weil er ein Loch von immer gleicher
  *Bild*größe ergäbe — und genau das ist hier falsch: steht die Kamera dicht
  hinter einer Wand, füllt diese das ganze Bild, und ein bildkonstantes Loch macht
  daraus ein Guckloch. Ausprobiert und verworfen; vom Raum war kein
  Quadratzentimeter mehr zu sehen. Die Röhre öffnet umso weiter, je näher die
  Wand an der Linse steht (Winkelradius `0.90/s`), und bleibt bei einer Wand
  direkt vor Noki immer noch bei `0.28` rad — mehr als seine halbe Höhe von
  `0.156`. Zu klein wird sie nie.

  Geschnitten wird **nur** Wand, Sturz, Pfosten und Türblatt. Die Treppenstufen
  trugen bis zu dieser Stufe dasselbe Material wie die Pfosten und wären
  mitgeschnitten worden — ein Loch im *Fußboden*, genau verkehrt herum. Sie haben
  deshalb jetzt `M_STUFE` mit **denselben Farbwerten**: am Bild ändert sich
  nichts, am Schnitt alles. Der Selbsttest liest das Schnittfenster aus dem
  Shadertext und prüft jede Materialnummer einzeln dagegen.
* **Wandschatten am Boden sind gerechnet, nicht marschiert.** Ein weicher
  Schattenmarsch über die ganze Fläche hätte jeden Wandquader *je Bodenpixel*
  gekostet; die Verdunklung aus dem Kantenabstand sieht an einer Wandfuge kaum
  anders aus. Der geworfene Schatten der **Figur** bleibt marschiert — er ist
  der eine, der trägt — und nur in ihrer Nähe.
* **Die Fuge im Durchgang.** Die rohe Raumkante liegt überall unter einer Wand
  und ist unsichtbar; genau in der Türöffnung wird sie zum Strich am Boden, der
  den Raumwechsel markiert.
* **Der Hintergrund wird im hellen Thema abgesenkt** (Faktor `0.40`), solange
  die Umgebung an ist. Ohne das landen helle Wand und heller Hintergrund nach
  ACES und Gamma beide bei `0.71` und sind nicht zu unterscheiden. Gemessen
  statt geschätzt:

  | Fläche | Ausgabewert |
  |---|---|
  | Wandoberkante | 0.70 |
  | Wand, beleuchtet | 0.68 |
  | Hintergrund | 0.60 … 0.63 |
  | Boden (Raumton −0.17 … +0.16) | 0.50 … 0.59 |
  | Wand, abgewandt | 0.37 |

---

## Kamera

Genau zwei Zahlen mussten sich ändern, damit die Räume sichtbar sind:

* Das Kameraziel war fest einkompiliert (`vec3(0.0, 0.500, 0.0)`) und ist jetzt
  das Uniform `u_ziel`, das Noki weich nachgeführt folgt. Ohne das liefe er beim
  ersten Schritt aus dem Bild.
* `DIST_MAX` von `4.2` auf `9.0`, damit sich das Haus überblicken lässt.

Dazu zwei Anfangswerte — mit Umgebung startet die Kamera bei `pitch 26°` und
`dist 3.60` statt `9°` und `2.20`, sonst füllt Noki das Bild und vom Raum ist
nichts zu sehen. Ziehen, Rad, Pinch, Ansichtsknöpfe, Grenzen und Glättung sind
unberührt.

---

## Bedienung

Ein **virtueller Joystick** unten links, für Finger gebaut. Er fängt seine
Zeiger mit `setPointerCapture` ein — deshalb streitet er sich nicht mit dem
Kamera-Ziehen auf dem Canvas. Weiter außen heißt schneller (Totzone `0.12`, aus
ihr heraus auf `0…1` gedehnt). Die Richtung ist **kamerabezogen**: oben heißt
„von der Kamera weg". Weltbezogen müsste man beim Drehen der Ansicht umdenken.

Die Leiste ist auf dem Handy bildschirmbreit; solange sie offen ist, weicht der
Joystick. Beide werden nicht gleichzeitig gebraucht, und beim Anfassen der Figur
schließt sich die Leiste ohnehin von selbst.

---

## Was der Selbsttest hier prüft

Die Umgebung wird **gefahren, nicht betrachtet**: derselbe Weg, den ein Nutzer
mit dem Joystick nimmt, läuft Bild für Bild durch `updateRig`. Ein Bild, in dem
Noki in einer Wand steckt, dauert eine sechzigstel Sekunde und ist auf keinem
Standbild zu sehen.

* Noki startet im Hauptbereich.
* Der Weg Hauptbereich → Schlafzimmer → zurück → Nebenraum wird abgefahren; die
  Raumfolge muss stimmen, und **kein einziges Bild** darf in einer Wand liegen.
* Aus jeder Raummitte in **acht Richtungen** gegen Wände und Ecken gefahren: die
  Position bleibt außerhalb aller Quader, und das Gleiten führt nicht durch eine
  Ecke hindurch.
* Jeder Durchgang **zweimal** quer abgetastet, mit offenem und mit geschlossenem
  Blatt. Die erste Runde misst den *Bau* — dafür muss die Tür offen stehen, sonst
  misst man das Blatt statt der Öffnung; der Spielraum für den Mittelpunkt muss
  mindestens einen Figurenradius betragen. Die zweite belegt, dass das Blatt
  überhaupt etwas tut: **kein einziger** Punkt darf geschlossen frei bleiben.
  Ein Türblatt, das nie sperrt, ist keins.
* **Er öffnet von selbst:** jede Tür wird mit *geschlossenem* Blatt angefahren.
  Kommt er drüben an, hat er sie selbst geöffnet — und die Prüfung verlangt
  zusätzlich, dass sie zum Zeitpunkt der Ankunft wirklich offen stand.
* **Lichte Höhe** `TUER_H ≥ NOKI_H + 0.05`, sonst streift er den Sturz.
* **Der Freischnitt trifft nur Wände.** Das Schnittfenster wird aus dem
  Shadertext gelesen und jede Materialnummer dagegen geprüft — Figur, Gegenstände,
  Haut und Treppenstufen müssen draußen liegen.
* **Jede Tür hat ihr Blatt am eigenen Winkel** `u_tuer<i>` im erzeugten Shader.
  Ein Blatt am falschen Uniform schwänge auf, wenn eine *andere* Tür geöffnet wird.
* Raumgrößen gegen das Maß der Figur, auch fürs Hinlegen.
* **Nullprobe:** bei `welt=0` bewegt sich nichts.

Eine Erkenntnis der Prüfung ist im Code festgehalten: **blind geradeaus zu
fahren genügt nicht.** Beim Wenden läuft Noki einen Bogen und versetzt sich
dabei quer um rund `0.17`. Nach zwei Wendungen stand er neben der Tür statt
davor und lief gegen die Wand. Wer zielt, kommt hindurch — die Prüfung führt
den Kurs deshalb je Bild auf einen Zielpunkt nach, wie es eine Hand am Joystick
tut. Gesucht oder geplant wird dabei nichts; es ist kein Pathfinding.

---

## Was diese Umgebung nicht kann

* **Die Kollision gilt nur der stehenden Figur** — einem Kreis von `0.20`.
  Liegend misst Noki rund `0.85`. Legt er sich dicht vor eine Wand, ragt er
  hindurch. Die Räume sind groß genug, dass es im normalen Gebrauch nicht
  auffällt; sauber gelöst wäre es erst mit einer posenabhängigen Hülle.
* **Keine Klinke, keine Hand am Blatt.** Er langt hin, und das Blatt schwingt —
  aber die Finger fassen nichts an, und der Griffabstand ist nicht ausgerechnet
  wie beim Greifsystem. Es ist eine Geste, keine Kopplung.
* Türen kennen keinen Widerstand: sie sind entweder zu oder offen und schwingen
  immer gleich schnell. Nichts klemmt, nichts schlägt an.
* Keine Decke, keine Fenster, keine Möbel.
* Kein Pathfinding, keine Agentenlogik, keine Wegsuche. Die Kennungen
  `main_room`, `bedroom`, `room_3` und die Durchgänge stehen bereit — was daraus
  einmal ein „geh ins Schlafzimmer" macht, ist nicht Teil dieser Stufe.

---

## Adressparameter

```
noki.html#welt=0                        die alte leere Bühne
noki.html#pos=-2.30,0,1.57              Standort x, z und Kurs im Standbild
                                        Kurs 0 = nach +z, +pi/2 = nach +x
noki.html#pos=0,0,0&still=1&ui=0        reproduzierbares Bild eines Raumes
noki.html#tuer=tuer_flur,0.42          Standbild eines Türgriffs: Kennung, u
                                       (setzt Blattwinkel UND Armhaltung;
                                        #pos= muss vorher stehen)
```
