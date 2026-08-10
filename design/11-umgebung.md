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
