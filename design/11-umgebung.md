# 11 · Die Umgebung — Boden, Wände, Räume, Durchgänge

> Drei Räume, zwei Durchgänge, ein Boden. Bewusst minimalistisch: eine saubere
> räumliche Grundlage, kein fertiges Zuhause. Keine Möbel, keine Dekoration,
> keine Agentenlogik.
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

```
        z
        ↑
 +1.20  ┌────────────┬──────────────────┬───────────┐
        │            │                  │           │
        │  bedroom   │    main_room     │  room_3   │
  0.00  │  2.0 × 2.4 ▯   2.6 × 2.4      ▯ 1.9 × 2.4 │
        │            │                  │           │
 -1.20  └────────────┴──────────────────┴───────────┘
      -3.30        -1.30              +1.30       +3.20  → x

        ▯ = Durchgang, 0.80 breit, Mitte bei z = -0.15
```

| Kennung | Name | Bereich x | Bereich z | Maße | Bodenton |
|---|---|---|---|---|---|
| `main_room` | Hauptbereich | −1.30 … +1.30 | −1.20 … +1.20 | 2.60 × 2.40 | ±0 |
| `bedroom` | Schlafzimmer | −3.30 … −1.30 | −1.20 … +1.20 | 2.00 × 2.40 | −0.170 |
| `room_3` | Nebenraum | +1.30 … +3.20 | −1.20 … +1.20 | 1.90 × 2.40 | +0.155 |

| Durchgang | von → nach | in der Wand bei | Mitte | Breite |
|---|---|---|---|---|
| `tuer_schlaf` | `main_room` → `bedroom` | x = −1.30 | z = −0.15 | 0.80 |
| `tuer_neben` | `main_room` → `room_3` | x = +1.30 | z = −0.15 | 0.80 |

Wandhöhe `0.62`, halbe Wanddicke `0.12`, Türpfosten `0.78` hoch und
`0.15` halbdick. Kollisionsradius der Figur `0.20`.

**Warum alle Räume gleich tief sind.** Nicht aus Bequemlichkeit: dadurch fällt
die gemeinsame Wand zweier Räume auf genau dieselbe Kante, und die
Zusammenfassung in `waende()` erkennt sie als *eine* Wand. Bei verschiedenen
Tiefen stünden dort zwei überlappende Quader, jede Tür würde doppelt gebaut und
die Pfosten stünden zweimal ineinander.

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

## Warum die Räume so klein sind

Nokis Bein misst von der Hüfte bis zum Knöchel `0.083` — **8.3 % seiner Höhe**.
Seine Schrittlänge folgt daraus zu `0.068`, ein voller Zyklus trägt `0.135`.

| Schrittfrequenz | Schritte/s | Tempo | 2.6 breiter Raum |
|---|---|---|---|
| 0.72 (bisher) | 1.4 | 0.097 /s | 26.7 s |
| 2.05 (Vollausschlag) | 4.1 | 0.278 /s | 9.4 s |

Für ein Streifenmuster war das gleichgültig. Für einen Weg durch drei Räume sind
es halbe Minuten. Zwei Dinge folgen daraus:

1. Der Joystick setzt die **Schrittfrequenz**, nicht die Strecke. Schneller
   heißt schneller treten. Würde stattdessen die Strecke skaliert, glitte Noki
   bei halber Kraft über den Boden.
2. Die **Räume sind nach seiner Schrittlänge bemessen**, nicht umgekehrt. Ein
   Haus in menschlichen Proportionen wäre für ihn ein Marathon.

Die Strecke bleibt exakt an den Fuß gekoppelt: `welt` rückt je Bild um genau
den Weg vor, den der abstoßende Fuß relativ zum Boden zurücklegt. Die
Zusicherung des Selbsttests, dass **immer genau ein Fuß stillsteht**, bedeutet
damit ab jetzt buchstäblich: *Noki rutscht nicht, während er läuft* — bei jedem
Tempo.

---

## Die Richtungskonvention

Sie steht hier, weil sie einmal falsch war und niemand es dem Code ansehen
konnte. Drei Dinge müssen dieselbe Zahl meinen:

```
Nokis Gesicht     lokal +z          (CANON.visorC = [0, 0.255, +0.200])
Welt → lokal      rotY(p − pos, +kurs)      in `zuFigur`
Weltvorwärts      (sin kurs, 0, cos kurs)   in `schiebe`
```

Aus den ersten beiden folgt die dritte: `lokal → Welt` ist `rotY(v, −kurs)`,
und das bildet lokales `+z` auf `(sin kurs, 0, cos kurs)` ab. **Stünde in
`zuFigur` ein `−kurs`, wäre die Blickrichtung an der x-Achse gespiegelt** —
bei ±90° zeigten Blick und Bewegung exakt entgegengesetzt, und die Augen
lägen auf der falschen Kopfseite.

Genau dieser Fehler war einmal drin, zusammen mit einer Umkehrung zu viel in
der Joystickachse (`atan2(nx, −ny)` statt `atan2(nx, ny)`). Der Selbsttest
prüft beides jetzt: er liest das Vorzeichen **aus dem Shader-Text** statt es
abzuschreiben, und er fährt acht Stickrichtungen über vier Kamerawinkel und
vergleicht den tatsächlichen Versatz mit dem Sollwert.

Ein dritter Ort hängt daran: die Zuhör-Haltung dreht den Kopf mit
`welt.kurs − camY` zum Betrachter. Das blosse `−camY` war richtig, solange
der Rumpf sich nie drehen konnte.

---

## Kollision

Kreis gegen achsparallele Rechtecke, kein Physiksystem. Geht der volle Schritt
nicht, wird jede Achse einzeln versucht — dadurch **gleitet** Noki an einer Wand
entlang, statt davor stehen zu bleiben. Sind beide Achsen blockiert, bleibt er
stehen: ein Ausweichen „irgendwohin" wäre ein Sprung, und ein Sprung durch eine
Wand ist schlimmer als ein Halt.

---

## Darstellung

* **Der Boden bleibt analytisch geschnitten.** Läge er im Distanzfeld, stünde die
  Kamera beim Neigen nach unten irgendwann darunter, der erste Abtastpunkt läge
  im Boden und die Figur verschwände. Niedrige Wände ändern daran nichts.
* **Wandschatten am Boden sind gerechnet, nicht marschiert.** Ein weicher
  Schattenmarsch über die ganze Fläche hätte sechzehn Quader *je Bodenpixel*
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
* Jeder Durchgang quer abgetastet — der Spielraum für den Mittelpunkt muss
  mindestens einen Figurenradius betragen, damit man nicht zielen muss.
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
* Keine Türblätter, keine Decke, keine Fenster, keine Möbel.
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
```
