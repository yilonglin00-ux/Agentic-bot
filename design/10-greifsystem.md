# 10 · Das universelle Greifsystem

> Erweiterung von [09](09-stufe2-und-greifsystem.md). Design, Rig und alle bestehenden
> Animationen bleiben unverändert — neu sind ausschließlich das Handgelenk, drei Griffarten
> und die Nutzungsfolgen.

---

## Die Ausgangslage, ehrlich benannt

Die Aufgabenstellung verlangt „welche Finger liegen wo" und „die Finger dürfen nicht durch
Objekte hindurchgehen" — und im selben Atemzug, das Design der Figur nicht zu ändern.

**Noki hat keine Finger.** Seine Hand ist eine Kugel mit Radius `0.032`, festgelegt in
[02](02-formensprache-material.md) als Teil seiner Formensprache. Beides zusammen geht nicht.

Die getroffene Entscheidung: **Die Kugelhand bleibt. Ein Handgelenk kommt dazu.** An einer
Kugel ist es unsichtbar — am gehaltenen Gegenstand sieht man es sofort. Damit werden
Eindrehen, Kippen und Drehen echte Bewegungen, ohne dass die Silhouette sich um einen
Millimeter ändert.

Aus „welcher Finger liegt wo" wird dadurch eine andere, aber gleichwertige Frage:
**Wo liegt der Griffpunkt, und welche Fläche des Gegenstands berührt die Hand?** Genau das
ist unten je Gegenstand festgelegt.

---

## Der neue Kanal

| Kanal | Bereich | Wirkung |
|---|---|---|
| `hand.x` | `−0.70 … +0.50` | Beugen und Strecken — Trinken, Tippen |
| `hand.y` | `±1.60` | Drehen um die Unterarmachse — Smartphone wenden |
| `hand.z` | `±0.90` | Kippen zur Seite — Schrauben |

Die Reihenfolge im Griffraum ist **erst Handgelenk, dann Grifflage**: Das Gelenk dreht die
ganze Hand samt Inhalt, die Grifflage sitzt in der bereits gedrehten Hand. Anders herum
würde der Gegenstand um sich selbst kreiseln statt mit der Hand zu schwenken.

---

## Die sieben Griffarten

Jede legt fest, wie Hand und Arm den Gegenstand tragen. Ein neuer Gegenstand bekommt eine
Griffart zugewiesen — mehr braucht es nicht, damit er sich richtig verhält.

| Griffart | Handgelenk | Arm | Wofür |
|---|---|---|---|
| **Kraftgriff** | `+0.10 / 0` | neutral | Werkzeuge mit Stiel: Schraubenzieher, Hammer, Schraubenschlüssel |
| **Präzisionsgriff** | `+0.24 / +0.10` | leicht vor | Kleinteile: Schlüssel, Stift |
| **Henkelgriff** | `0 / −0.14` | leicht ein | Gefäße mit Henkel: Tasse, Werkzeugkoffer |
| **Flachgriff** | `−0.22 / 0` | vor | Flächen: Smartphone, Buch, Fernbedienung |
| **Zwei-Hand-Griff** | `−0.14 / 0` | beide vor | Schweres oder Großes: Tablet, Kiste |
| **Zeigegriff** | `+0.30 / 0` | weit vor | Taschenlampe, alles Gerichtete |
| **Traggriff** | `0 / −0.06` | hängend | Flasche, Pflanze, Koffer im Gehen |

---

## Die drei Hauptgegenstände

### Tasse — Henkelgriff

| Frage | Antwort |
|---|---|
| **Wo greift die Hand?** | Am Henkel, nicht am Gefäß. Griffpunkt `(−0.026, −0.004, 0.004)` in der Handebene — der Henkelring liegt im Griffbogen, das Gefäß hängt seitlich daneben |
| **Berührungsfläche** | Der Henkelring auf der Innenseite der Handkugel; das Gefäß berührt die Hand nicht |
| **Eine oder zwei Hände?** | Eine. Bei `0.35` Gewicht kein Grund für zwei |
| **Aufnehmen** | Arm senkt sich (`hinlangen`), Handgelenk `+0.26` gebeugt, Blick auf die Hand; beim `fassen` streckt sich das Gelenk auf `+0.10` |
| **Wie ändert sich die Haltung beim Benutzen?** | Zum Mund: Arm auf `1.86` aus, Handgelenk `−0.30`. Beim Trinken Arm auf `2.05`, Hand auf Höhe der unteren Gesichtskante; das Gelenk kippt über `0.9 s` weich auf `−0.85` — **das Gefäß neigt sich, nicht der Arm.** Siehe die Nachrechnung unten |
| **Ablegen** | Rückweg über `halten`, Gelenk auf `+0.22`, Arm senkt sich langsamer als beim Aufnehmen — vorsichtig abstellen heißt: die letzte Bewegung ist die langsamste |
| **Folge** | Hinlangen `1.0` → Fassen `0.55` → Halten `1.0` → Zum Mund `1.4` → Trinken `2.6` → Halten `1.2` → Ablegen `1.5` → Leer `0.6` |

### Smartphone — Flachgriff

| Frage | Antwort |
|---|---|
| **Wo greift die Hand?** | Rückseite, unteres Drittel. Griffpunkt `(0.012, −0.026, 0.022)`, Grifflage `1.05 / 0.16` — das Gerät liegt schräg an der Handinnenseite, Anzeige zum Gesicht |
| **Berührungsfläche** | Die ganze Rückseite liegt an der Handkugel an; kein Rand steht frei |
| **Eine oder zwei Hände?** | Eine. Das Tablet dagegen zwei — gleiche Griffart, anderes Gewicht (`0.15` gegen `0.55`) |
| **Aufnehmen** | Wie Tasse, aber flacher: Gelenk kippt beim Fassen nur auf `+0.10` |
| **Ansehen** | `betrachten`: Arm `0.96`, Gelenk `−0.30`, Kopf dreht `−0.21` zur Hand, Blick `−0.34` |
| **Scrollen** | Gelenk wiegt `±0.05` mit `3.1 Hz`, Blick wandert mit — die Augen folgen dem Daumen, den es nicht gibt |
| **Tippen** | Gelenk nickt `+0.09` in einseitigen Stößen mit `6.2 Hz` — nur nach unten, wie ein Antippen |
| **Wischen** | Gelenk dreht `±0.22` um die Unterarmachse mit `2.4 Hz` |
| **Drehen** | Gelenk dreht einmalig `1.55` über `1.1 s` — Hoch- zu Querformat |
| **Ablegen** | Wie Tasse |
| **Folge** | Hinlangen `0.95` → Fassen `0.5` → Ansehen `1.3` → Scrollen `2.2` → Tippen `1.6` → Wischen `1.4` → Drehen `1.6` → Ablegen `1.5` → Leer `0.6` |

### Schraubenzieher — Kraftgriff

| Frage | Antwort |
|---|---|
| **Wo greift die Hand?** | Am dicken Griffteil, nicht am Schaft. Griffpunkt `(0.004, −0.006, 0.006)`, Grifflage `0.30 / 0.10` — die Klinge zeigt nach unten aus der Faust |
| **Berührungsfläche** | Der Griffzylinder `0.034 × 0.018` liegt quer in der Hand, die Kuppe schaut oben heraus |
| **Eine oder zwei Hände?** | Eine — Kraftgriff ist einhändig definiert |
| **Aufnehmen** | Gelenk `+0.26` beim Hinlangen, `+0.10` beim Fassen |
| **Ansetzen** | Eigene Phase: Arm `0.66` aus und `0.62` vor, Kopf `+0.20` gesenkt, Blick `−0.36` auf die Spitze. Gelenk `+0.34` — die Klinge steht senkrecht auf der gedachten Schraube |
| **Eindrehen** | **Ein Ratschen, kein Kreisel.** Das Gelenk dreht über `62 %` des Zyklus um `1.35` durch, löst dann über `38 %` zurück und setzt neu an. Zyklus `1.35 Hz`. Ein durchgehendes Kreiseln sähe aus wie ein Motor, nicht wie eine Hand |
| **Ausdrehen** | Dieselbe Mechanik mit umgekehrtem Vorzeichen |
| **Absetzen** | Über `halten` zurück, dann `ablegen` — kontrolliert heißt: erst aus der Arbeitshaltung lösen, dann senken |
| **Folge** | Hinlangen `1.0` → Fassen `0.55` → Ansetzen `1.1` → Eindrehen `3.6` → Ausdrehen `2.4` → Halten `1.0` → Ablegen `1.6` → Leer `0.6` |

---

## Gewicht wirkt sichtbar

Jeder Gegenstand hat ein Gewicht von `0` bis `1`. Es verändert die Haltung, ohne dass eine
eigene Animation nötig wäre:

| Wirkung | Formel |
|---|---|
| Arm sinkt | `−0.16 · Gewicht` auf den Ausschwenkwinkel |
| Rumpf lehnt sich gegen | `−0.020 · Gewicht` nach hinten |
| Kopf senkt sich | `+0.05 · Gewicht` |
| Atem wird ruhiger | `×(1 − 0.12 · Gewicht)` |

Tablet `0.55`, Tasse `0.35`, Schraubenzieher `0.25`, Smartphone `0.15`, Schlüssel `0.08`.
Man sieht den Unterschied, ohne dass er benannt werden müsste.

---

## Die dreizehn weiteren Gegenstände

Griffart zugewiesen, Grifflage und Gewicht festgelegt. Jeder ist ein Geometriezweig plus ein
Eintrag — das Animationssystem bleibt unberührt.

| Gegenstand | Griffart | Gewicht | Grifflage `drehX / drehZ` | Nutzbewegung |
|---|---|---|---|---|
| Schraubenschlüssel | Kraft | 0.35 | `0.30 / 0.10` | Ratschen wie Schraubenzieher |
| Hammer | Kraft | 0.50 | `0.28 / 0.10` | Ausholen und Zuschlagen über `hand.z` |
| Stift | Präzision | 0.05 | `0.42 / 0.14` | kleine Bögen über `hand.z` |
| Schlüssel | Präzision | 0.08 | `0.22 / 0.06` | Drehen um `hand.y` |
| Taschenlampe | Zeigen | 0.20 | `0.20 / 0.10` | keine Bewegung, Linse leuchtet |
| Fernbedienung | Flach | 0.10 | `0.85 / 0.12` | Tippen wie Smartphone |
| Buch | Flach | 0.30 | `0.95 / 0.12` | Blättern über `hand.y` |
| Tablet | Zwei Hände | 0.55 | `1.05 / 0.16` | Tippen, beide Arme vorn |
| Kleine Box | Zwei Hände | 0.40 | `0.15 / 0.06` | Tragen |
| Werkzeugkoffer | Henkel | 0.70 | `0.00 / 0.06` | Tragen, Arm deutlich gesenkt |
| Flasche | Tragen | 0.45 | `0.10 / 0.08` | Kippen wie Tasse |
| Glas | Henkel | 0.25 | `0.05 / 0.10` | Kippen wie Tasse |
| Geschenk | Zwei Hände | 0.30 | `0.15 / 0.06` | Reichen |
| Pflanze | Tragen | 0.40 | `0.00 / 0.04` | Tragen |

---

## Was das System nicht kann

Damit niemand mehr erwartet, als da ist:

- **Keine Finger.** Die Hand umschließt nichts. Der Gegenstand liegt an der Handkugel an und
  bewegt sich mit ihr. Aus jedem üblichen Blickwinkel liest sich das als Halten; aus
  nächster Nähe sieht man, dass nichts zugreift.
- **Die Hände können sich nicht treffen.** Bei `±0.205` Schulterbreite und `0.24` Armlänge
  reichen sie nicht bis zur Körpermitte. Der Zwei-Hand-Griff bringt deshalb beide Arme nach
  vorn und rückt den Gegenstand zur Mitte — es liest sich als beidhändiges Tragen, ist aber
  kein echtes Zusammenführen. Ein solches bräuchte längere Arme, und das wäre eine
  Designänderung.
- **Der Gegenstand hat keinen Platz in der Welt.** Er wächst beim Fassen in die Hand und
  schrumpft beim Ablegen heraus — beides genau dann, wenn die Hand unten ist. Ein
  liegenbleibender Gegenstand braucht die Vorwärtskinematik des Arms auch auf der
  Rechenseite. Das ist der nächste sinnvolle Schritt.

---

## Das Objektmenü

Im Reiter **Gegenstand** stehen oben die fünf Objekte. Ein Klick wählt eines aus — darunter
erscheinen **ausschließlich dessen Aktionen**. Insgesamt 37, verteilt auf fünf Objekte.

| Objekt | Aktionen |
|---|---|
| **Smartphone** | Display ansehen · Tippen · Scrollen · Wischen · Nachricht lesen · Nachricht schreiben · Telefonieren · Foto aufnehmen · Drehen · Einstecken |
| **Tablet** | Lesen · Tippen · Scrollen · Notizen machen · Präsentation zeigen · Video betrachten · Ablegen |
| **Schlüssel** | Aufnehmen · Betrachten · Ins Schloss führen · Abschließen · Aufschließen · Schlüsselbund bewegen · Einstecken · Ablegen |
| **Schraubenzieher** | Aufnehmen · Schraube ansetzen · Festziehen · Lösen · Kontrollieren · Ablegen |
| **Tasse** | Aufnehmen · Kurz betrachten · Zum Mund führen · Trinken · In der Hand halten · Vorsichtig abstellen |

### Jede Aktion ist vollständig

Der Läufer setzt vor jede Aktion automatisch das **Aufnehmen**, wenn nichts in der Hand liegt,
und danach das **saubere Beenden** in die Halten-Stellung. Aktionen mit `ende` schließen
stattdessen mit Ablegen ab. Eine Aktion ist damit immer:

```
[Hinlangen → Fassen]  →  eigentliche Bewegung  →  [Halten | Ablegen → Leer]
```

Eine Aktion besteht dadurch nur noch aus ihrer Kernbewegung — der Rahmen entsteht von selbst.
Eine neue Aktion ist eine Zeile:

```
{ id, label, e: Ausdruck, ph: [[Phase, Dauer], …], ende: 0|1 }
```

### Die Bewegungen sitzen im Handgelenk

Keine der Aktionen bewegt den Arm anders als die Greifphase es vorgibt. Was sie unterscheidet,
ist die Handgelenkbewegung:

| Aktion | Bewegung |
|---|---|
| Tippen | Gelenk nickt `+0.09`, einseitig, `6.2 Hz` |
| Schreiben | dasselbe mit `8.4 Hz`, Blick wandert schneller |
| Scrollen | Gelenk wiegt `±0.05` mit `3.1 Hz`, Blick geht mit |
| Wischen | Gelenk dreht `±0.22` um die Unterarmachse |
| Lesen | keine Handbewegung — nur der Blick wandert über die Zeilen |
| Drehen | einmalig `1.55` über `1.1 s` |
| Telefonieren | Gelenk kippt `+0.10`, Kopf neigt sich `+0.10` dagegen |
| Foto | ruhiges Halten, bei `1.4 s` ein kurzer Stoß — der Auslöser |
| Zeichnen | zwei überlagerte Achsen, `3.6` und `2.3 Hz` |
| Abschließen | **eine gefasste Vierteldrehung** `1.35` über `1.1 s`, kein Kreisen |
| Aufschließen | dieselbe Drehung in Gegenrichtung |
| Schlüsselbund | gedämpftes Klingeln: `±0.16` mit `7.2 Hz`, exponentiell abklingend |
| Festziehen | Ratschen: `62 %` durchdrehen, `38 %` lösen und neu ansetzen |
| Kontrollieren | langsames Vor-die-Augen-Drehen mit `0.8 Hz` |
| Trinken | Gelenk kippt über `0.9 s` weich auf `−0.85` |

### Die Trinkhaltung — eine Korrektur

Die frühere Fassung hob den Arm nur auf `1.24`. Nachgerechnet stand die Hand damit bei
`y = 0.325` — **`0.155` unterhalb der Kopfunterkante**. Die Tasse kam nie ans Gesicht; „zum
Mund führen" war ein leeres Versprechen.

Jetzt: Arm auf `2.05`, Hand bei `y = 0.510`, also auf Höhe der unteren Gesichtskante. Der
Abstand zum Kopf ist nachgerechnet:

```
Kopf-Halbbreite bei y = 0.510   0.213
Linke Tassenkante               0.325
Freiraum                        0.112
```

Die Tasse kann den Kopf damit nicht durchdringen — weder in der Anfahrt noch beim Kippen,
weil das Kippen im Handgelenk sitzt und die Hand nicht verschiebt.

**Warum nicht direkt vor dem Gesicht?** Bei `±0.205` Schulterbreite und `0.24` Armlänge
erreicht die Hand die Körpermitte nicht. Die Tasse kommt seitlich ans Gesicht, der Kopf neigt
sich ihr entgegen. Das ist die glaubwürdigste Lösung ohne Designänderung.

---

## Einen Gegenstand hinzufügen

1. **Geometrie**: ein Zweig in `sdGegenstand` aus den vorhandenen Grundformen
2. **Eintrag** in `GEGENSTAND`: `{ typ, art, label, gewicht, off, drehX, drehZ, nutz }`
3. Fertig — Griffarten, Phasen, Gewichtswirkung und Abläufe greifen automatisch

Eine eigene Nutzungsfolge braucht es nur, wenn der Gegenstand etwas kann, das keine der
vorhandenen Nutzbewegungen abdeckt.
