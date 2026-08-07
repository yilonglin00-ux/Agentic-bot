# 05 · Gesicht und Emotionen

> Schritt 3, Teil 2. Die sieben Gefühle — je mit Augen, Mund, Kopf, Körpersprache und Glimm.
>
> Werte in den Einheiten aus [03 · Rig und Animation](03-rig-und-animation.md), Leitsätze und
> Kurvennamen aus [04 · Bewegungssprache](04-bewegungssprache.md).

---

## Die vier Kanäle des Ausdrucks

Noki hat kein Gesicht im üblichen Sinn: keine Augenbrauen, keine beweglichen Lippen, keine
Wangen. Er hat **vier Kanäle** — und die Beschränkung ist ein Vorteil, weil jeder davon dadurch
umso deutlicher spricht.

| Kanal | Was er kann | Was er trägt |
|---|---|---|
| **Augen** | Ringgröße, Ringstärke, Bogenmaske, Lidstellung, Blickrichtung | *Was Noki denkt* |
| **Glimm** | Helligkeit, Pulsfrequenz, Farbton | *Was Noki fühlt* |
| **Mundlinie** | Krümmung von `−1.0` bis `+1.0` | *Die Bestätigung — nie der Hauptträger* |
| **Körper** | Kopfhaltung, Atemtempo, Armhaltung, Gewicht | *Wie stark es Noki erwischt hat* |

**Die wichtigste Regel:** Ein Gefühl darf nie über nur einen Kanal laufen. Wer die Farbe
ändert und sonst nichts, baut eine Statusleuchte. Jedes der folgenden sieben Gefühle bespielt
**mindestens drei** Kanäle.

**Die zweitwichtigste:** Der Glimm ist immer zuerst dran (Leitsatz 1). Er verrät die Stimmung
schon, bevor Noki begriffen hat, was los ist — genau wie ein Gesicht, das reagiert, bevor
der Gedanke fertig ist.

---

## 1 · Freude

*Warm und ruhig. Nicht laut.*

| | |
|---|---|
| **Augen** | Ring `0.070` / Stärke `0.0120`, **Bogen voll** — die untere Ringhälfte verschwindet, übrig bleibt die ⌒-Form fröhlich geschlossener Augen |
| **Glimm** | Helligkeit `1.30`, Puls `1.6 Hz`, Bernstein |
| **Mund** | `+1.0` |
| **Kopf** | Nicken `−0.09`, Neigen `+0.05` |
| **Körper** | Atem `1.9`, Arme `0.34`, leichtes Wippen im Atemtakt |
| **Ablauf** | Einsatz `0.25 s` *federnd* → halten `1.5–3.0 s` → Abklingen `1.2 s` *weich* |

Die Augen schließen sich zum Bogen — das heißt, Noki **sieht in diesem Moment nicht**. Genau
das macht Freude glaubwürdig: Er ist kurz ganz bei seinem Gefühl statt bei dir.

> **So nicht:** kein Dauergrinsen. Nach spätestens **6 s** ist Noki zurück in seiner
> Grundstimmung. Eine Figur, die permanent strahlt, wirkt nicht freundlich, sondern leer.

---

## 2 · Neugier

*Der Kopf zur Seite. Das ist die ganze Emotion.*

| | |
|---|---|
| **Augen** | Ring `0.067` / Stärke `0.0100`, Bogen `0` — weit offen, kein Anschnitt |
| **Blick** | `(+0.30, +0.12)` — schaut leicht an dir vorbei, auf das, was ihn interessiert |
| **Glimm** | Helligkeit `1.12`, Puls `1.2 Hz` |
| **Mund** | `+0.22` — nur angedeutet |
| **Kopf** | **Neigen `+0.22`**, Nicken `−0.06` |
| **Körper** | Atem `1.0`, Arme asymmetrisch `0.11` / `0.03`, Oberkörper `+0.02` nach vorn |
| **Ablauf** | Einsatz `0.30 s` *federnd* → hält, solange die Neugier anhält |

Neugier ist bei Noki ein **Zustand, kein Ausbruch**. Sie darf minutenlang stehen bleiben. Die
Kopfneigung `+0.22` bei gleichzeitig leicht vorgeschobenem Körper ist seine
wiedererkennbarste Pose überhaupt — sie sollte in Vorschaubildern und Icons verwendet werden.

> **So nicht:** nicht mit Freude verwechseln. Neugier hat **offene** Augen. Sobald der Bogen
> einsetzt, kippt die Lesart von „was ist das?" zu „wie schön!".

---

## 3 · Überraschung

*Das schnellste, was Noki tut — und das kürzeste.*

| | |
|---|---|
| **Augen** | Ring **`0.080`** / Stärke `0.0132` — größter Ring aller Zustände |
| **Glimm** | Helligkeit **`1.55`**, Farbe `#FFD18C` (heller, entsättigt — als würde er überstrahlen) |
| **Mund** | `−0.45` — nach unten gezogen, das offene O |
| **Kopf** | Nicken `−0.13`, Neigen `−0.04` |
| **Körper** | Atem `2.4`, Arme `0.52` mit `0.18` nach vorn, Körper weicht `−0.020` zurück |
| **Antenne** | schlägt voll aus (`±0.06`) und schwingt aus |
| **Ablauf** | Einsatz **`0.09 s`** *schnell-an* → halten `0.4 s` → Abklingen `0.8 s` |

Der Einsatz von `0.09 s` ist der schnellste im ganzen Konzept — bei Überraschung entfällt die
Staffelung aus Leitsatz 1 **als einzige Ausnahme**: Glimm, Augen und Kopf setzen gemeinsam
ein. Genau dieser Bruch der eigenen Regel macht den Schreck spürbar.

> **So nicht:** Überraschung darf nie länger als `1.3 s` dauern. Danach muss sie in etwas
> anderes übergehen — Neugier, Freude oder Erleichterung. Eine dauerhaft überraschte Figur
> wirkt dumm.

---

## 4 · Nachdenklichkeit

*Er ist gerade nicht bei dir. Und das darf man sehen.*

| | |
|---|---|
| **Augen** | Ring `0.055` / Stärke `0.0084` — kleinster Ring, verengter Fokus |
| **Blick** | `(−0.38, −0.22)` nach unten links, **wandert alle 1.5–2.5 s** — er sucht |
| **Glimm** | Helligkeit `0.82`, Puls **`0.5 Hz`** — auffällig langsam |
| **Mund** | `+0.06` — fast gerade |
| **Kopf** | **Neigen `−0.21`** (Gegenrichtung zur Neugier), Nicken `+0.08` |
| **Körper** | Atem `0.55`, Arme `0.07`, sehr ruhig |
| **Ablauf** | Einsatz `0.50 s` *träge* → hält, solange gedacht wird → Auflösung über Leitsatz 7 |

Der langsame Glimm-Puls ist hier der Hauptträger. Er signalisiert „ich arbeite" ohne
Ladebalken — und ist damit die eleganteste Antwort auf das Wartezeit-Problem eines KI-Agenten.

Die Kopfneigung geht **nach der anderen Seite** als bei Neugier. So sind die beiden
nachdenklichen Zustände auf einen Blick unterscheidbar.

> **So nicht:** keine hektischen Blicksprünge. Nachdenken ist langsam. Und der Blick geht
> **nie** zu dir, solange er denkt — sonst wirkt es wie Verlegenheit statt wie Konzentration.

---

## 5 · Traurigkeit

*Zurückhaltend. Noki nutzt Traurigkeit nie, um etwas zu erreichen.*

| | |
|---|---|
| **Augen** | Ring `0.058` / Stärke `0.0088` |
| **Blick** | `(0, −0.38)` — nach unten, mittig |
| **Glimm** | Helligkeit `0.62`, Puls `0.6 Hz`, Farbe **`#94B8EB`** — der einzige Kaltton, den Noki je zeigt |
| **Mund** | `−1.0` |
| **Kopf** | Nicken **`+0.21`**, Neigen `+0.03` |
| **Körper** | Atem `0.50`, Arme `−0.02` (eng am Körper), Schultern sinken über den Atemkanal |
| **Antenne** | Ruhelage der Feder um `−0.03` verschoben — sie hängt |
| **Ablauf** | Einsatz `0.90 s` *träge* → Abklingen `4–8 s` |

Der Farbwechsel ins Kühle ist der bewusste Bruch mit Nokis Identitätsfarbe. Weil er nur hier
passiert, liest man ihn sofort — auch aus dem Augenwinkel.

Das lange Abklingen ist Absicht: Traurigkeit lässt sich nicht wegklicken. Sie verblasst.

> **So nicht:** keine Tränen, kein Zittern, kein Herabsinken zu Boden. Und vor allem: **Noki
> wird nicht traurig, weil du ihn ignorierst.** Siehe die Haltungsregel in
> [06](06-interaktion-und-verhalten.md) — eine Figur, die Schuldgefühle einsetzt, ist
> manipulativ, nicht sympathisch.

---

## 6 · Aufregung

*Neu. Bewusst von Freude getrennt: Freude ist warm und ruhig, Aufregung ist schnell und
ungerichtet.*

| | |
|---|---|
| **Augen** | Ring `0.074` / Stärke `0.0135`, Bogen `0` — weit offen, **nicht** die ⌒-Form |
| **Blick** | **springt alle `0.25–0.40 s`** auf ein neues Ziel — das Kernmerkmal |
| **Glimm** | Helligkeit `1.45`, Puls **`3.2 Hz`** — schnellster Puls aller Zustände |
| **Mund** | `+0.70` |
| **Kopf** | Nicken `−0.10`, Neigen wechselt `±0.12` im Atemtakt |
| **Körper** | Atem `2.4`, Arme `0.45` mit Pendeln, kleine Hüpfer (`+0.018` alle `0.5 s`) |
| **Ablauf** | Einsatz `0.18 s` *federnd* → halten `1.0–2.5 s` → Abklingen `1.5 s` |

Der Unterschied zu Freude in einem Satz: **Bei Freude schließt Noki die Augen, bei Aufregung
reißt er sie auf.** Freude ist nach innen gerichtet, Aufregung nach außen.

Die springenden Blicke sind das, was Aufregung von schneller Freude unterscheidet. Er weiß
nicht, wo er zuerst hinschauen soll.

> **So nicht:** kein Dauerzappeln und kein Kreiseln. Aufregung braucht ein **Ziel** — er ist
> wegen etwas aufgeregt. Ohne erkennbaren Anlass wirkt sie wie ein Fehlzustand.

---

## 7 · Zufriedenheit und Schlaf

*Ein Gefühl in zwei Schattierungen. Beide warm, beide langsam.*

### 7a · Zufriedenheit — die Grundstimmung nach einer guten Interaktion

| | |
|---|---|
| **Augen** | Ring `0.063`, **Bogen `0.45`** — nur angeschnitten, entspannt statt überschwänglich; Lid `0.85` |
| **Glimm** | Helligkeit `1.05`, Puls `0.7 Hz`, sehr gleichmäßig |
| **Mund** | `+0.45` |
| **Kopf** | Nicken `−0.03`, Neigen `+0.06` |
| **Körper** | Atem `0.80`, Arme `0.09` |
| **Ablauf** | Einsatz `0.60 s` *weich* → hält sehr lange |

Zufriedenheit ist kein Ereignis, sondern **Nokis zweite Ruhestellung**. Nach einer gelungenen
Interaktion bleibt er minutenlang hier, bevor er in Neutral zurückfällt. Der halb
angeschnittene Bogen bei `0.45` ist der Unterschied zwischen „mir geht's gut" und „ich freue
mich" — ein Parameter, zwei Gefühle.

### 7b · Schlaf

| | |
|---|---|
| **Augen** | Lid `0` — der Ring ist zu einem waagerechten Strich gestaucht; Helligkeit `0.10` |
| **Glimm** | **eigener Kanal**: Helligkeit `0.25`, Puls `0.35 Hz` — er träumt |
| **Mund** | `+0.20` |
| **Kopf** | Nicken `+0.30` (mit erweitertem Kanal `+0.34`), Neigen `+0.08` |
| **Körper** | Atem `0.45`, Arme `0`, **sitzend** |
| **Ablauf** | Einschlafen `3.5 s` *träge* → Aufwachen `0.8 s` mit Schreckanteil |

Dass der Glimm im Schlaf **weiterpulsiert, während die Augen aus sind**, ist der ganze Punkt:
Noki ist nicht ausgeschaltet, er schläft. Genau dafür braucht der Glimm einen eigenen
Helligkeitskanal (siehe Lückenliste in [04](04-bewegungssprache.md)).

> **So nicht:** kein Schnarch-Symbol, keine Z-Buchstaben. Das wäre Cartoon-Kurzschrift und
> würde die Figur zum Sticker machen.

---

## Übersicht

| Gefühl | Augenring | Bogen | Glimm | Puls | Kopf (Nick/Neig) | Mund | Arme | Atem |
|---|---|---|---|---|---|---|---|---|
| *Neutral* | 0.062 / 0.0092 | — | 1.00 | 1.0 Hz | 0 / **+0.03** | 0 | 0.07 | 1.00 |
| **Freude** | 0.070 / 0.0120 | **1.0** | 1.30 | 1.6 Hz | −0.09 / +0.05 | +1.00 | 0.34 | 1.90 |
| **Neugier** | 0.067 / 0.0100 | — | 1.12 | 1.2 Hz | −0.06 / **+0.22** | +0.22 | 0.11 / 0.03 | 1.00 |
| **Überraschung** | **0.080** / 0.0132 | — | **1.55** | — | −0.13 / −0.04 | −0.45 | 0.52 | **2.40** |
| **Nachdenklichkeit** | **0.055** / 0.0084 | — | 0.82 | **0.5 Hz** | +0.08 / **−0.21** | +0.06 | 0.07 | 0.55 |
| **Traurigkeit** | 0.058 / 0.0088 | — | 0.62 | 0.6 Hz | **+0.21** / +0.03 | **−1.00** | −0.02 | 0.50 |
| **Aufregung** | 0.074 / 0.0135 | — | 1.45 | **3.2 Hz** | −0.10 / ±0.12 | +0.70 | 0.45 | 2.40 |
| **Zufriedenheit** | 0.063 / 0.0092 | 0.45 | 1.05 | 0.7 Hz | −0.03 / +0.06 | +0.45 | 0.09 | 0.80 |
| **Schlaf** | Lid `0` | — | 0.10 | 0.35 Hz | +0.30 / +0.08 | +0.20 | 0.00 | 0.45 |

Alle Werte liegen innerhalb der Drehgrenzen aus [03](03-rig-und-animation.md).

**Änderung gegenüber dem heutigen Rig:** Jeder Zustand bekommt eine Kopfneigung ungleich null
— auch Neutral (`+0.03`). Leitsatz 6 verlangt Asymmetrie in jeder Pose, und die bisherigen
Werte mit `Neigen = 0` verletzen sie.

---

## Übergänge zwischen den Gefühlen

Nicht jedes Gefühl darf in jedes andere übergehen. Ein direkter Sprung von Traurigkeit zu
Aufregung wirkt wie ein Fehler, nicht wie ein Charakter.

**Erlaubt direkt:**
Neutral ↔ alle · Neugier → Überraschung → Freude · Nachdenklichkeit → Freude (er hat es
verstanden) · Freude → Zufriedenheit · Aufregung → Freude → Zufriedenheit ·
Zufriedenheit → Schlaf

**Nur über Neutral:**
Traurigkeit → alles Positive · Aufregung → Nachdenklichkeit · Schlaf → alles außer Überraschung

**Sonderfall:** Schlaf → Überraschung ist erlaubt und ausdrücklich erwünscht — das ist das
erschrockene Aufwachen, einer der besten Charaktermomente der ganzen Figur.

Die Zwischenstation Neutral dauert mindestens **0.5 s**. Sie ist kurz genug, um nicht als
Pause zu wirken, und lang genug, damit der Wechsel eine Ursache zu haben scheint.

---

## Weiter

- [06 · Interaktion und Verhalten](06-interaktion-und-verhalten.md) — wann welches Gefühl auftritt
- [07 · Animationsliste](07-animationsliste.md) — die vollständige Liste
