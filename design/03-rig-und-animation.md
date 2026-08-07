# 03 · Rig und Animation

> Die Bewegungsgrundlage. Hierarchie, Drehpunkte, Grenzen und das Gefühlssystem.
>
> Alles hier Beschriebene läuft bereits live in `noki.html` — die Werte sind erprobt, nicht
> vorgeschlagen.

---

## Bauteilhierarchie

Die Namen sind gleichzeitig die Objektnamen für das spätere Mesh. Wer sie übernimmt, kann die
Animationswerte aus dem Viewer direkt auf das Rig legen.

```
wurzel
└── koerper                    Pivot (0, 0.315, 0)   · atmet, staucht
    ├── kopf_pivot             Pivot (0, 0.500, 0)   · Nick, Dreh, Neig
    │   ├── kopfschale
    │   ├── visier             ← trägt die Augen als Muster, nicht als Geometrie
    │   ├── ohrkapsel_l / _r
    │   ├── antenne_lang       Pivot (−0.075, 0.955, −0.020)  · federt nach
    │   │   └── glimm          ← Stimmungsanzeiger
    │   └── sensor_stummel     Pivot ( 0.086, 0.962, −0.020)  · federt schwächer nach
    ├── arm_l                  Pivot (−0.205, 0.400, 0.018)
    │   └── unterarm_l         Pivot lokal (0.036, −0.128, 0)
    │       └── hand_l
    └── arm_r                  Pivot ( 0.205, 0.400, 0.018)
        └── unterarm_r
            └── hand_r

bein_l / bein_r                Pivot (±0.085, 0.158, 0)  · nicht am Körper aufgehängt
└── fuss_l / fuss_r                                        Noki steht fest auf dem Boden
```

Die Beine hängen **bewusst nicht** unter dem Körper: beim Atmen bewegt sich der Rumpf, die
Füße bleiben stehen. Das ist der Unterschied zwischen „schwebt" und „steht".

---

## Drehgrenzen

| Knochen | Achse | Ruhelage | Bereich | Anmerkung |
|---|---|---|---|---|
| `kopf_pivot` | Nicken (x) | 0 | −0.25 … +0.30 | negativ = hochschauen |
| `kopf_pivot` | Drehen (y) | 0 | −0.55 … +0.55 | darüber bricht die Silhouette |
| `kopf_pivot` | Neigen (z) | 0 | −0.30 … +0.30 | die Nachdenk-Geste lebt hiervon |
| `antenne_lang` | Nachlauf | 0 | ±0.06 | nie direkt keyframen, immer federn lassen |
| `arm_*` | Ausschwenken (z) | 0.07 | −0.10 … +2.40 | ab ~2.4 taucht der Arm in den Kopf |
| `arm_*` | Vorschwingen (x) | 0.17 | −0.20 … +0.60 | positiv = nach vorn |
| `koerper` | Heben (y) | 0 | ±0.010 | Atembewegung |
| `koerper` | Stauchen | 0 | ±0.020 | volumenerhaltend gegengerechnet |
| `koerper` | Seitlich (x) | 0 | ±0.015 | Gewicht verlagern, seitliches Lehnen |
| `koerper` | Tiefe (z) | 0 | −0.020 … +0.050 | Heranlehnen, Zurückweichen |
| `koerper.sprung` | Heben (y) | 0 | 0 … +0.050 | Hüpfen — **eigener Kanal**, addiert sich auf den Atemkanal |
| `koerper` | Rollen (z) | 0 | ±0.050 | Gewichtsverlagerung, Verlegenheit |
| `koerper` | Rumpfneigung (x) | 0 | −0.10 … +0.10 | Sitzhaltung |
| `bein_*` | Beinwinkel (x) | 0 | 0 … −1.583 | Sitzstellung. **Dreht das Bein bei fester Länge `0.083`** — Stützpunkte einzeln zu interpolieren würde es dehnen |
| `glimm` | Helligkeit | 1.0 | 0 … 2.0 | **eigener Kanal**, unabhängig von der Augenhelligkeit |

Alle Kanäle dieser Tabelle sind in `noki.html` umgesetzt und werden vom eingebauten
Selbsttest (`noki.html#selftest=1`) über 1200 simulierte Sekunden gegen genau diese Grenzen
geprüft. Zusätzlich prüft er über den gesamten Sitzübergang, dass **Beinlänge und Fußversatz
konstant bleiben** und der Fuß nicht in den Boden sinkt.

> **Gliedmaßen werden gedreht, nie gestreckt.** Interpoliert man Gelenkpunkte einzeln
> zwischen zwei Posen, bleibt die Länge dazwischen nicht erhalten. Beim Sitzen wüchse das
> Bein so auf das Doppelte und der Fußansatz auf das Dreifache. Jede Pose wird deshalb über
> feste Beträge und Winkel gebaut.

**Die harte Grenze:** Kein Wert darf die Silhouettenregel aus
[02](02-formensprache-material.md) verletzen. Ein über 2.4 rad ausgeschwenkter Arm
verschwindet hinter dem Kopf — deshalb winkt Noki nach außen statt gerade nach oben.

---

## Bewegungsprinzipien

### 1 · Atmen ist die Grundfrequenz

Alles andere hängt daran. Der Körper hebt und senkt sich um `0.0062`, die Stauchung läuft
**gegenphasig** (`+π`) mit `0.013`. Grundtempo `1.15` — der Ausdruckszustand skaliert es
(`breath`), von `0.45` bei Müdigkeit bis `2.4` bei Überraschung.

Das Armschwingen läuft auf derselben Frequenz, seitenverkehrt. So wirkt die Figur als ein
Körper und nicht als Sammlung von Teilen.

### 2 · Nachlauf statt Keyframes

Die Antenne wird **nie animiert**. Sie hängt an einer Feder, die von der
Kopfdrehgeschwindigkeit angetrieben wird:

```
antrieb        = −kopf_dreh_geschwindigkeit · 0.020
beschleunigung = (antrieb − auslenkung) · 190 − geschwindigkeit · 15
```

Ausschlag begrenzt auf `±0.06`. Dieser eine Mechanismus erledigt den größten Teil des
Eindrucks „lebendig": Der Kopf bewegt sich, die Antenne kommt hinterher und schwingt aus.

### 3 · Blinzeln muss unregelmäßig sein

- Abstand: **1.9 s + Zufall bis 3.6 s** — ein fester Takt wirkt sofort mechanisch
- Dauer: **0.15 s** einfach, **0.34 s** doppelt
- Doppelblinzeln in **28 %** der Fälle
- Die Lidbewegung ist eine Sinuskurve, kein linearer Verlauf

Technisch schließt sich das Auge nicht — der Ringradius wird vertikal gestaucht, bis er zu
einem waagerechten Strich zusammenfällt. Dieselbe Mechanik erzeugt auch die müden Halblider.

### 4 · Der Kopf steht nie still

Auch im Ruhezustand: Nicken `±0.020` bei Tempo `0.83`, Neigen `±0.018` bei `0.47`, und ein
langsames Umsehen über eine überlagerte Dreifach-Sinuskurve mit `±0.22`. Drei nicht
harmonische Frequenzen — dadurch wiederholt sich das Muster nie hörbar.

Die Pupillen folgen dem Umsehen mit **90 %** — die Augen laufen dem Kopf leicht voraus, wie
bei einem Lebewesen.

### 5 · Übergänge, keine Schnitte

Zwischen zwei Ausdrücken wird jeder Wert weich überblendet (`k = 1 − 0.001^Δt`, entspricht
etwa **0.35 s**). Noki springt nie von Pose zu Pose. Das ist der Unterschied zwischen einer
Figur und einer Zustandsmaschine.

---

## Gefühlssystem

Neun Zustände. Jeder verändert **Augenform, Leuchtfarbe, Kopfhaltung, Mundlinie, Armhaltung
und Atemtempo** — niemals nur die Farbe.

| Zustand | Augenring | Bogen | Leuchten | Kopf (Nick/Neig) | Mund | Arme | Atem |
|---|---|---|---|---|---|---|---|
| **Neutral** | 0.062 / 0.0092 | — | 1.00 | 0 / 0 | 0 | 0.07 | 1.0 |
| **Neugierig** | 0.067 / 0.0100 | — | 1.12 | −0.06 / **+0.22** | +0.22 | 0.11 / 0.03 | 1.0 |
| **Glücklich** | 0.070 / 0.0120 | **voll** | 1.30 | −0.09 / 0 | **+1.0** | 0.34 | 1.9 |
| **Überrascht** | **0.080** / 0.0132 | — | **1.55** | −0.13 / 0 | −0.45 | 0.52 | **2.4** |
| **Denkend** | 0.055 / 0.0084 | — | 0.82 | +0.08 / **−0.21** | +0.06 | 0.07 | 0.55 |
| **Müde** | 0.059, **Lid 0.42** | — | **0.55** | +0.17 / 0 | −0.20 | 0.02 | 0.45 |
| **Traurig** | 0.058 / 0.0088 | — | 0.62 | **+0.21** / 0 | **−1.0** | −0.02 | 0.50 |
| **Stolz** | 0.061 | 0.55 | 1.22 | **−0.17** / 0 | +0.65 | 0.15 | 0.80 |
| **Winken** | 0.069 / 0.0116 | 0.85 | 1.28 | −0.07 / +0.10 | +0.85 | rechts **2.0 ± 0.28** | 1.6 |

**Blickrichtung** ergänzt den Ausdruck: neugierig `(+0.30, +0.12)`, denkend `(−0.38, −0.22)`,
traurig `(0, −0.38)`. Die Augen schauen dorthin, wohin die Figur denkt.

### Die Bogen-Mechanik

`Bogen = 1` blendet die untere Hälfte des Augenrings aus. Übrig bleibt der obere Bogen — die
klassische ⌒-Form fröhlich geschlossener Augen. Bei `0.55` (stolz) ist der Ring nur
angeschnitten: zufrieden, aber nicht überschwänglich. Ein Parameter, zwei sehr
unterschiedliche Gefühle.

### Winken als Geste, nicht als Stimmung

Winken ist der Beleg, dass das System auch **Gesten** trägt: Der rechte Arm schwenkt auf
`2.0 rad` aus und pendelt mit `±0.28` bei Tempo `6.4`. Nach außen statt gerade hoch — sonst
verschwindet die Hand hinter dem Kopf. Nach demselben Muster lassen sich Nicken, Zeigen,
Achselzucken und Erschrecken ergänzen.

---

## Hinweise für die Mesh-Umsetzung

Falls Noki als klassisches 3D-Modell gebaut wird:

- **Topologie:** Alle Bauteile sind Rotations- oder Kapselkörper. Ein Quad-Grid mit
  Kantenschleifen an den Gelenken reicht; nirgends entstehen Dreieckssterne.
- **Kopfschale und Visier bleiben getrennte Objekte.** Das Glas sitzt 0.016 versenkt und
  folgt der Wölbung der Schale — als eigenes, leicht verkleinertes Duplikat.
- **Augen als Textur oder Shader-Maske**, nicht als Geometrie. Ringradius, Ringstärke,
  vertikale Stauchung und Bogenmaske werden zu vier Parametern.
- **Blendshapes werden kaum gebraucht.** Nur eine für die Mundlinienkrümmung, alles andere
  läuft über Knochen und die Augenparameter.
- **Die Antenne braucht mindestens zwei Knochen** plus einen Federmodifikator — sonst geht
  der Nachlauf verloren, und mit ihm die halbe Lebendigkeit.

---

## Kamera im Viewer

| | Wert |
|---|---|
| Blickpunkt | `(0, 0.565, 0)` |
| Brennweite | `1.85` |
| Abstand | `2.05` (Bereich `1.25 … 4.2`) |
| Neigung | begrenzt auf **±1.40 rad (±80°)** |
| Drehung | unbegrenzt, volle 360° |
| Ruhedrehung | `0.085 rad/s` nach 4.5 s ohne Eingabe |

**Steuerung:** Ziehen mit Maus oder Finger dreht frei. Horizontales Scrollen dreht um die
Hochachse, vertikales Scrollen neigt. Zwei-Finger-Pinch und Strg+Scroll zoomen. Die Kamera
läuft mit Trägheit und schwingt weich aus.

**Adressparameter** für reproduzierbare Ansichten:

```
noki.html#yaw=90&pitch=20&dist=2.0&e=denkend&still=1&t=0&ui=0&theme=dark
```

| Parameter | Bedeutung |
|---|---|
| `yaw`, `pitch` | Kamerawinkel in Grad |
| `dist` | Kameraabstand |
| `e` | Ausdruckszustand |
| `still=1` | friert jede Bewegung ein (für Standbilder) |
| `t` | Zeitpunkt im eingefrorenen Zustand |
| `ui=0` | blendet die Bedienoberfläche aus |
| `theme` | `dark` oder `light` erzwingen |

---

## Nächster Schritt

[04 · Bewegungssprache](04-bewegungssprache.md) — der Animationsstil, der auf dieser
Grundlage aufsetzt: Leitsätze, Ruheverhalten und die acht Kanäle, die dem Rig dafür noch
fehlen.
