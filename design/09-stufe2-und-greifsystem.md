# 09 · Stufe 2 — Abläufe und Greifsystem

> Erweiterung, keine Änderung. Alles aus Stufe 1 bleibt unangetastet: Maßkanon, Rig,
> Leitsätze, Zeitkaskade, Stimmung, die dreizehn laufenden Animationen. Stufe 2 setzt
> ausschließlich darauf auf.
>
> Werte in den Einheiten aus [03](03-rig-und-animation.md), Kurvennamen und Leitsätze aus
> [04](04-bewegungssprache.md), Prioritäten aus [06](06-interaktion-und-verhalten.md).

---

## Was neu ist

| Baustein | Was er leistet |
|---|---|
| **Sequenzsystem** | verkettet vorhandene Bausteine zu mehrstufigen Abläufen — die Aufgabenschleife entsteht daraus, ohne dass eine einzige Kombination eigens gebaut würde |
| **Zehn neue Einlagen** | D3 Überraschung · C2 Aufwachen · E2 Nicken · E5 Nachfragen · E7 Zeigen · F1 Annehmen · F4 Fehler · G1 Stolz · G3 Heranlehnen · G5 Dankbarkeit |
| **Zwei neue Ausdrücke** | Aufgeregt und Zufrieden — beide waren in [05](05-gesicht-und-emotionen.md) beschrieben, aber noch nicht umgesetzt |
| **Neun Abläufe** | Aufgabe gelingt · Aufgabe misslingt · Aufwachen · Wiedersehen · Erklären · Nachfragen · Dankbarkeit · Aufregung · Greifen |
| **Greifsystem** | Handkoordinaten, vier Griffarten, sieben Phasen, vier gebaute Gegenstände |

---

## Das Sequenzsystem

Ein Ablauf ist eine Liste von Schritten. Jeder Schritt setzt beim **Eintritt** einige Werte und
läuft dann seine Dauer ab:

| Feld | Wirkung beim Eintritt |
|---|---|
| `d` | Dauer des Schritts in Sekunden |
| `e` | Ausdruck (überblendet über `0.35 s`, wie immer) |
| `clip` | startet eine Einlage nach den üblichen Prioritätsregeln |
| `achtung` | Aufmerksamkeit: `1` = schaut dich an, `0` = schaut sich um |
| `greif` | Greifphase |
| `haltung` | Grundhaltung erzwingen |

**Warum das der richtige Zuschnitt ist:** Die Aufgabenschleife braucht keine eigene Animation.
Sie ist eine Kette aus Annehmen, Nachdenken, Aufregung, Stolz und Zufriedenheit — alles
Bausteine, die für sich stehen und einzeln geprüft werden können. Neue Abläufe kosten dadurch
eine Handvoll Zeilen und kein neues Rig.

Während ein Ablauf läuft, ruht die Zeitkaskade. Er wird von einer Einlage höherer Priorität
überlagert, aber nicht abgebrochen — der nächste Schritt kommt trotzdem.

---

## Die dreizehn Animationen

Jede mit Auslöser, Start- und Endzustand, Körper, Kopf, Augen, Ausdruck, Armen, Dauer und
Anschluss.

### 1 · Aufwachen und den Nutzer erkennen — Ablauf `aufwachen`

| | |
|---|---|
| **Auslöser** | Eingabe nach 5 min bis 3 h Abwesenheit |
| **Start → Ende** | Schlaf (sitzend, Augen zu) → Zufrieden (stehend) |
| **Körper** | Ruck nach oben `+0.016`, Arme fahren aus, dann Aufrichten in den Stand |
| **Kopf** | `−0.13` hoch im Schreck, danach `−0.05` beim Aufrichten |
| **Augen** | Ring springt von `0` auf `+0.014`, Blick fängt sich, sucht dich |
| **Ausdruck** | Überrascht `0.55 s` → Neugierig `0.70 s` → Glücklich `2.0 s` |
| **Arme** | beide `+0.42` im Schreck, dann Winken |
| **Dauer** | `7.6 s` gesamt · Einlage `C2k` `1.6 s`, *schnell-an* → *weich* |
| **Anschluss** | endet in Zufrieden, der zweiten Ruhestellung — geht von dort in jeden Zustand |

Der einzige erlaubte direkte Übergang von Schlaf zu Überraschung. Er ist der beste
Einzelmoment der Figur, weil er die Regel bricht, die sonst überall gilt.

### 2 · Überraschung — Einlage `D3k` + Ausdruck `ueberrascht`

| | |
|---|---|
| **Auslöser** | unerwartetes Ereignis, Beginn des Ablaufs `aufregung` |
| **Start → Ende** | beliebig → derselbe Zustand, aufgelöst nach spätestens `1.3 s` |
| **Körper** | weicht `−0.020` zurück **und** hebt sich `+0.013` — Schreck geht nach hinten oben |
| **Kopf** | `−0.10` hoch |
| **Augen** | Ring `+0.016` (größter des ganzen Satzes), Glimm `+0.45` |
| **Ausdruck** | Mundlinie `−0.45`, das offene O |
| **Arme** | beide `+0.45`, `+0.16` nach vorn |
| **Dauer** | `1.2 s` · Einsatz `0.07 s` *schnell-an* |
| **Anschluss** | muss in etwas übergehen — Neugier, Freude oder Erleichterung |

**Die einzige Animation ohne Staffelung.** Glimm, Augen und Kopf setzen gemeinsam ein. Genau
dieser Bruch der eigenen Regel macht den Schreck spürbar.

### 3 · Aufregung — Ausdruck `aufgeregt`, Ablauf `aufregung`

| | |
|---|---|
| **Auslöser** | überraschend Positives, Erfolg mit gutem Ergebnis |
| **Start → Ende** | Überrascht → Glücklich → Zufrieden |
| **Körper** | Atem `×2.4`, Rumpf wiegt, Arme `0.45` mit Vorlage `0.24` |
| **Kopf** | `−0.10`, Neigung wechselt `±0.09` |
| **Augen** | Ring `0.074` **weit offen**, kein Bogen; Glimm-Puls `3.2 Hz` — der schnellste |
| **Ausdruck** | Mundlinie `+0.70` |
| **Arme** | `0.45` beidseitig, pendelnd |
| **Dauer** | Ablauf `5.9 s`, Ausdruck hält `2.2 s` |
| **Anschluss** | klingt zwingend ab — Aufregung ist nicht haltbar |

Der Unterschied zu Freude in einem Satz: **bei Freude schließt Noki die Augen, bei Aufregung
reißt er sie auf.** Freude geht nach innen, Aufregung nach außen.

### 4 · Zustimmendes Nicken — Einlage `E2`

| | |
|---|---|
| **Auslöser** | Zustimmung, Bestätigung, Abschluss von Erklären und Nachfragen |
| **Start → Ende** | beliebig → derselbe Zustand |
| **Körper** | federt `±0.006` gegen, verzögert (Leitsatz 4) |
| **Kopf** | `+0.11` in zwei Nickern mit abnehmender Amplitude |
| **Augen** | unverändert, Glimm `+0.12` |
| **Ausdruck** | unverändert — Nicken ist eine Geste, keine Stimmung |
| **Arme** | unverändert |
| **Dauer** | `0.95 s` · *pendelnd* |
| **Anschluss** | endet exakt bei null, geht daher in alles über |

### 5 · Nachdenklichkeit — Ausdruck `denkend` (Stufe 1) im Ablauf

Die Nachdenklichkeit selbst lief schon. Neu ist ihre **Rolle in der Aufgabenschleife**: Sie
trägt dort die Wartezeit, mit Glimm-Puls `0.5 Hz` und alle `1.5–2.5 s` wanderndem Blick, und
ersetzt den Ladebalken vollständig. Aufmerksamkeit sinkt auf `0.25` — er ist gerade nicht bei
dir, und das darf man sehen.

### 6 · Nachfragen — Einlage `E5`, Ablauf `nachfragen`

| | |
|---|---|
| **Auslöser** | mehrdeutige Eingabe, fehlende Angabe |
| **Start → Ende** | Neutral → Neutral, über Neugierig |
| **Körper** | `+0.020` nach vorn |
| **Kopf** | Neigung `+0.24` — **die stärkste des ganzen Satzes** |
| **Augen** | Ring `+0.006`, Blick bleibt auf dir stehen und wartet |
| **Ausdruck** | Neugierig, Glimm `+0.10` |
| **Arme** | unverändert |
| **Dauer** | `1.5 s` *federnd*, Ablauf `4.7 s` mit abschließendem Nicken |
| **Anschluss** | löst sich über E2 Nicken auf — Frage gestellt, Antwort quittiert |

### 7 · Begrüßung nach längerer Abwesenheit — Ablauf `wiedersehen`

| | |
|---|---|
| **Auslöser** | Eingabe nach über einem Tag |
| **Start → Ende** | Schlaf → Zufrieden |
| **Körper** | wie Aufwachen, danach `+0.048` heranlehnen |
| **Kopf** | Neigung `+0.18` beim Heranlehnen |
| **Augen** | Ring `+0.005`, **Blick hält `3.3 s`** statt der üblichen `0.8 s` |
| **Ausdruck** | Überrascht → Neugierig → Glücklich |
| **Arme** | Schreck, später Winken |
| **Dauer** | `11.0 s` — der längste Ablauf |
| **Anschluss** | Zufrieden |

**Der lange Blick ist alles.** Er erzählt „ich hab dich vermisst", ohne es zu behaupten — und
ohne einen Vorwurf, dass du weg warst. Siehe die Haltungsregel in
[06](06-interaktion-und-verhalten.md).

### 8 · Aufmerksam zuhören — Zustand `achtung` (Stufe 1), erweitert

Neu ist, dass die Abläufe die Aufmerksamkeit **gezielt setzen**: `1` beim Zuhören und beim
Präsentieren, `0.25` beim Nachdenken, `0` beim Hantieren mit einem Gegenstand. Bei
Aufmerksamkeit `1` dreht Noki den Kopf zur Kamera (bis `±0.55`), neigt ihn `+0.08`, drosselt
den Atem auf `0.95` — und tut sonst nichts.

### 9 · Etwas erklären oder zeigen — Einlage `E7`, Ablauf `erklaeren`

| | |
|---|---|
| **Auslöser** | Erläuterung, Verweis auf etwas |
| **Start → Ende** | Neutral → Zufrieden |
| **Körper** | unverändert — die Geste trägt allein |
| **Kopf** | dreht `−0.15` zur zeigenden Hand, Neigung `+0.09` |
| **Augen** | **Blick pendelt** mit `−0.32` zwischen Hand und Nutzer, zwei volle Wechsel |
| **Ausdruck** | Mundlinie `+0.32` |
| **Arme** | rechter Arm `+0.78` aus, `+0.46` nach vorn |
| **Dauer** | `2.8 s` · Hülle `0.20 / 0.26` |
| **Anschluss** | schließt mit E2 Nicken — erklärt und bestätigt |

Der pendelnde Blick ist der Unterschied zwischen **Zeigen** und **Erklären**: Wer nur zeigt,
schaut hin. Wer erklärt, vergewissert sich, ob du folgst.

### 10 · Stolz und Freude über eine gelungene Aufgabe — Einlage `G1`, Ablauf `aufgabe`

| | |
|---|---|
| **Auslöser** | `aufgabe_fertig`, mehrere Erfolge hintereinander, Stimmung über `+0.6` |
| **Start → Ende** | Denkend → Zufrieden |
| **Körper** | Brust `+0.028` vor |
| **Kopf** | `−0.15` hoch |
| **Augen** | **Bogen `0.50`** — nur angeschnitten: zufrieden, nicht überschwänglich |
| **Ausdruck** | Mundlinie `+0.55`, Glimm `+0.22` |
| **Arme** | beide `+0.09` |
| **Dauer** | `2.5 s` *federnd* |
| **Anschluss** | Zufrieden, hält minutenlang |

Im Ablauf geht dem Stolz eine Sekunde **Aufregung** voraus — erst der Ausbruch, dann die
ruhige Freude. Und der Blick sucht dich, **bevor** die Freude kommt (Leitsatz 7): Das ist der
Unterschied zwischen einer Erfolgsanimation und einem geteilten Moment.

### 11 · Enttäuschung / leichter Misserfolg — Einlage `F4`, Ablauf `fehlschlag`

| | |
|---|---|
| **Auslöser** | `fehler` |
| **Start → Ende** | Denkend → Neugierig → Neutral |
| **Körper** | Zusammenzucken `−0.012` in `0.055 s`, danach ruhig |
| **Kopf** | `+0.14` gesenkt, ab `62 %` Neigung `+0.16`: *nochmal versuchen?* |
| **Augen** | unverändert, Glimm `−0.30` |
| **Ausdruck** | Mundlinie `−0.35` — **ausdrücklich nicht Traurigkeit** |
| **Arme** | rechter Arm `+0.46` halb gehoben, `+0.20` vor — die Geste des „hm" |
| **Dauer** | `2.9 s` · *schnell-an* → *träge* |
| **Anschluss** | endet in Neugier — das Angebot, es nochmal zu versuchen |

Bedauern ist nach außen gerichtet und bietet einen nächsten Schritt an; Traurigkeit ist nach
innen gerichtet und lädt zum Trösten ein. Bei Fehlern will Noki das Erste.

### 12 · Dankbarkeit — Einlage `G5`, Ablauf `danke`

| | |
|---|---|
| **Auslöser** | Hilfe erhalten, Lob erwidern |
| **Start → Ende** | Neutral → Zufrieden |
| **Körper** | `+0.014` vor, kleine Verbeugung |
| **Kopf** | `+0.16` gesenkt |
| **Augen** | **Bogen `0.72`** — fast geschlossen, warm |
| **Ausdruck** | Mundlinie `+0.55`, Glimm `+0.30` |
| **Arme** | rechte Hand `+0.60` aus, `+0.48` vor — zur Brust |
| **Dauer** | `2.3 s` *weich* |
| **Anschluss** | Zufrieden |

Die Verbeugung ist bewusst **klein**. Eine tiefe Verbeugung wäre Unterwürfigkeit, und die
verbietet das Charakterkonzept ausdrücklich.

### 13 · Die vollständige Aufgabenschleife — Abläufe `aufgabe` und `fehlschlag`

```
Aufgabe erhalten →  F1 Annehmen        0.75 s   zwei Nicker, Glimm zieht an
Nachdenken       →  Denkend            3.20 s   Glimm 0.5 Hz, Blick wandert
Blick zurück     →  Neutral            0.45 s   er sucht dich zuerst (Leitsatz 7)
Ergebnis         →  Aufgeregt          0.75 s   der Ausbruch
                 →  Glücklich + G1     1.80 s   der geteilte Moment
Feedback         →  Zufrieden          2.40 s   die Ruhe danach
```

Der Misserfolgszweig ersetzt die letzten drei Schritte durch **F4 Fehler** und endet in
Neugier statt in Zufriedenheit. Beide Zweige dauern rund neun Sekunden und sind jederzeit
durch ein Ereignis höherer Priorität überlagerbar.

---

## Das Greifsystem

### Warum es so gebaut ist

Nokis Hand ist eine **Kugel ohne Finger** — das ist eine Festlegung aus
[02](02-formensprache-material.md) und bleibt es. Greifen kann also nicht durch Umschließen
entstehen. Es entsteht aus drei anderen Dingen:

1. **Der Arm bringt die Hand hin** — die Bewegung erzählt das Greifen, nicht die Hand
2. **Der Gegenstand rastet in ein Handkoordinatensystem ein** — Ursprung ist die Handmitte,
   die Achsen laufen mit dem Unterarm
3. **Der Blick geht mit** — ohne ihn wirkt jedes Hantieren wie ein angeklebtes Requisit

### Der Griffraum

```
Handmitte  =  Armgelenkkette + (0.052, −0.238, 0.006)
Griffraum  =  Handmitte + Versatz, gedreht um x und z
```

Ein Gegenstand braucht damit genau **fünf Zahlen**: Versatz `(x, y, z)` und zwei Drehwinkel.
Mehr nicht. Das ist der Kern der Erweiterbarkeit.

### Die vier Griffarten

| Griffart | Wie der Gegenstand liegt | Beispiele |
|---|---|---|
| **Stabgriff** | Längsachse quer durch die Faust, Werkzeugende zeigt nach unten | Schraubenzieher · Hammer · Schraubenschlüssel · Stift · Taschenlampe |
| **Flachgriff** | Fläche liegt an der Handinnenseite, leicht angekippt | Tablet · Smartphone · Buch |
| **Bügelgriff** | Henkel liegt im Griffbogen, Gefäß hängt daneben | Kaffeebecher · Werkzeugkoffer · Gießkanne |
| **Faustgriff** | kleiner Gegenstand ganz in der Handfläche, ragt nur wenig heraus | Schlüssel · Geschenk · kleine Kiste |

### Die sieben Phasen

Universell — unabhängig davon, welcher Gegenstand in der Hand liegt.

| Phase | Arm aus | Arm vor | Kopf | Blick | Gegenstand |
|---|---|---|---|---|---|
| **Leer** | `0.00` | `0.00` | `0.00` | — | — |
| **Hinlangen** | `0.26` | `0.54` | `+0.17` | `−0.30` zur Hand | — |
| **Fassen** | `0.30` | `0.56` | `+0.18` | `−0.30` | erscheint |
| **Halten** | `0.54` | `0.32` | `+0.04` | `−0.12` | mitgeführt |
| **Betrachten** | `0.96` | `0.48` | `−0.02` | `−0.34` | vor die Augen |
| **Benutzen** | `0.86` | `0.44` | `+0.02` | `−0.28` | eigene Bewegung |
| **Reichen** | `0.70` | `0.58` | `−0.05` | `0.00` zu dir | präsentiert |
| **Ablegen** | `0.26` | `0.54` | `+0.17` | `−0.30` | verschwindet |

Zwischen den Phasen wird über `0.55 s` überblendet — nie geschnitten, wie überall sonst auch.

**Benutzen** ist die einzige gegenstandsabhängige Phase. Sie greift auf die Griffdrehung zu:

| Nutzachse | Bewegung | Gegenstände |
|---|---|---|
| `drehen` | Griff kippt `±0.55` mit `5.4 Hz` | Schraubenzieher · Schlüssel · Schraubenschlüssel |
| `tippen` | Griff nickt `0.10` im Takt | Tablet · Smartphone |
| `kippen` | Griff neigt sich langsam bis `0.55` und zurück | Kaffeebecher · Gießkanne |
| `schlagen` | Griff holt aus und schnellt zurück | Hammer |
| `leuchten` | keine Bewegung, Kegel schaltet zu | Taschenlampe |

### Der Gegenstandseintrag

```
{ typ, griff, label, off:[x,y,z], drehX, drehZ, nutzAchse }
```

**Gebaut und geprüft** sind vier Gegenstände, die alle vier Griffarten abdecken:

| Gegenstand | Griffart | Aufbau |
|---|---|---|
| **Schraubenzieher** | Stabgriff | Griffzylinder `0.034×0.018` + Kuppe, Schaft `0.046×0.0055`, Klinge |
| **Tablet** | Flachgriff | Platte `0.050×0.070×0.0035`, Anzeigefläche emissiv |
| **Kaffeebecher** | Bügelgriff | Hohlzylinder `0.040×0.034`, Henkel als Ring `0.024/0.0065` |
| **Schlüssel** | Faustgriff | Ring `0.017/0.0045`, Bart mit Zahn |

Alle Gegenstände werden mit einem **gemeinsamen Maßstab `1.55`** um den Griffpunkt skaliert.
Bei Nokis Größe wären maßstabsgetreue Werkzeuge nicht mehr lesbar; ein Aufziehroboter darf
etwas kräftigere Werkzeuge tragen.

### Die zehn weiteren Gegenstände

Vollständig spezifiziert, noch nicht gebaut. Jeder ist ein Zweig in `sdGegenstand` plus ein
Eintrag in `GEGENSTAND` — nach Erfahrung mit den ersten vier je rund zehn Zeilen.

| Gegenstand | Griffart | Geometrie | `drehX` / `drehZ` | Nutzachse |
|---|---|---|---|---|
| Hammer | Stab | Stiel `0.050×0.013` + Kopf `0.030×0.016×0.016` | `0.30 / 0.10` | schlagen |
| Schraubenschlüssel | Stab | Flachstab `0.055×0.012×0.005`, Maul als U | `0.30 / 0.10` | drehen |
| Stift | Stab | Zylinder `0.045×0.006`, Spitze konisch | `0.42 / 0.14` | drehen |
| Taschenlampe | Stab | Zylinder `0.040×0.016`, Kopf `0.014×0.020`, Linse emissiv | `0.20 / 0.10` | leuchten |
| Smartphone | Flach | Platte `0.028×0.056×0.004` | `1.05 / 0.16` | tippen |
| Buch | Flach | Block `0.050×0.066×0.014`, Schnitt heller | `0.95 / 0.12` | blättern |
| Werkzeugkoffer | Bügel | Kasten `0.075×0.050×0.036`, Bügel als Ring | `0.00 / 0.06` | tragen |
| Geschenk | Faust | Würfel `0.038` mit Band als zwei Bänder | `0.15 / 0.06` | reichen |
| Kleine Kiste | Faust | Kasten `0.042×0.034×0.034` | `0.15 / 0.06` | tragen |
| Pflanze | Bügel | Topf als Kegelstumpf `0.034`, drei Blattkapseln | `0.00 / 0.04` | tragen |

### Was noch fehlt

Ehrlich benannt, damit niemand es für erledigt hält:

- **Der Gegenstand hat keinen Platz in der Welt.** Beim Fassen wächst er in die Hand hinein,
  beim Ablegen schrumpft er heraus — beides genau dann, wenn die Hand unten ist. Ein
  Gegenstand, der auf dem Boden liegen bleibt, braucht eine Vorwärtskinematik des Arms auch
  auf der Rechenseite. Das ist der nächste sinnvolle Schritt.
- **Die zweite Hand** ist noch nicht angebunden. Der Griffraum existiert für beide Arme, es
  fehlt nur die Auswahl.
- **Beidhändiges Tragen** (Werkzeugkoffer, Kiste) folgt daraus.

---

## Weiter

- [07 · Animationsliste](07-animationsliste.md) — die vollständige Liste, mit ▶ für alles,
  was läuft
- [06 · Interaktion und Verhalten](06-interaktion-und-verhalten.md) — das Verhaltensmodell,
  das die Abläufe später auslöst
