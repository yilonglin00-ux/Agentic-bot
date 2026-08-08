# 02 · Formensprache, Maßkanon und Material

> Schritt 2 der visuellen Grundlage. Die verbindliche Bauvorlage.
>
> **Alle Werte hier stimmen exakt mit `noki.html` überein.** Das Distanzfeld im Shader ist
> nicht die Illustration der Doku — es *ist* das Modell. Wer Noki als Mesh nachbaut, kann
> jede Zeile dieser Tabellen direkt übernehmen.

---

## Einheit

Alle Maße in **Einheiten der Gesamthöhe**:

```
1.00  =  Scheitel der weißen Kopfschale
0.00  =  Bodenlinie (Fußunterkante)
```

Die Spitze der langen Antenne liegt bei **1.295** — sie ragt über den Scheitel hinaus und
zählt nicht zur Grundhöhe. Ursprung ist `(0, 0, 0)` in Bodenmitte, **+y** nach oben,
**+z** nach vorn (Blickrichtung), **+x** ist Nokis rechte Seite.

---

## Silhouettenregel

> **Kopfbreite (0.58) > Körperbreite (0.42).**

Noki liest immer als *großer Kopf auf kleinem Ei*. Diese Regel gilt in jeder Ansicht und in
jeder Pose. Keine Animation und keine Kameraeinstellung darf sie brechen — sie ist der
Grund, warum die Figur aus der Ferne noch als Noki erkennbar ist.

**Zweite Regel: keine einzige scharfe Kante.** Jedes Bauteil ist eine Kugel, ein Ellipsoid,
eine Kapsel oder eine Box mit großem Eckradius. Der kleinste Rundungsradius an der ganzen
Figur ist 0.042 (Fuß).

---

## Bauteilkanon

### Kopfgruppe

Drehpunkt der gesamten Gruppe: **`(0, 0.500, 0)`** (Halsansatz). Alle folgenden Werte sind
*lokal zu diesem Drehpunkt*.

| Bauteil | Form | Position (lokal) | Maße | Material |
|---|---|---|---|---|
| Kopfschale | Box mit Rundung | `(0, 0.240, 0)` | Halbmaße `(0.115, 0.085, 0.095)`, Radius `0.175` → **B 0.58 · H 0.52 · T 0.54** | Perlweiß |
| Visierausschnitt | Box mit Rundung | `(0, 0.255, 0.200)` | Halbmaße `(0.155, 0.075, 0.250)`, Radius `0.070` → Öffnung **B 0.45 · H 0.29** | — (Subtraktion) |
| Visierglas | Schnittmenge | wie Ausschnitt | **0.016 hinter der Schale versenkt**, folgt deren Wölbung | Visierglas |
| Ohrkapsel ×2 | Kapsel | `(±0.238…±0.278, 0.216, −0.018)` | Radius `0.047` | Graphit |
| **Antenne lang** | 2 Kapseln | `(−0.075, 0.455, −0.020)` → `(−0.116, 0.660, −0.050)` → `(−0.101, 0.752, −0.038)` | Radius `0.0165` / `0.0145` | Graphit |
| **Glimm** | Kugel | Spitze der langen Antenne | Radius `0.043` | Emissiv |
| Sensorstummel | Kapsel + Kappe | `(0.086, 0.462, −0.020)` → `(0.096, 0.542, −0.030)` | Radius `0.0155`, Kappe `0.027` | Graphit |

Der Visierausschnitt reicht **absichtlich nur teilweise** in den Kopf (bis `z = −0.05`).
Ginge er durch, entstünde von hinten ein Durchbruch.

### Rumpf und Gliedmaßen

Weltkoordinaten.

| Bauteil | Form | Position | Maße | Material |
|---|---|---|---|---|
| Körper | Ellipsoid | `(0, 0.315, 0)` | Radien `(0.210, 0.165, 0.190)` → **B 0.42 · H 0.33 · T 0.38** | Perlweiß |
| Hals | Kapsel | `(0, 0.420, 0)` → `(0, 0.520, 0)` | Radius `0.072` | Graphit |
| Schultergelenk ×2 | Kugel | `(±0.205, 0.400, 0.018)` | Radius `0.042` | Graphit |
| Oberarm ×2 | Kapsel | lokal `(0.006, −0.018, 0)` → `(0.032, −0.112, 0)` | Radius `0.050` | Perlweiß |
| Gelenkband ×2 | Kapsel | lokal `(0.033, −0.116, 0)` → `(0.037, −0.134, 0)` | Radius `0.046` | Graphit |
| Unterarm ×2 | Kapsel | lokal `(0.038, −0.140, 0)` → `(0.048, −0.206, 0.004)` | Radius `0.043` | Perlweiß |
| **Handballen ×2** | Ellipsoid | lokal `(0.052, −0.238, 0.006)`, Mitte `(0, 0.004, 0.002)` | Radien `(0.0290, 0.0300, 0.0235)` | Graphit |
| **Handteller ×2** | Ellipsoid | lokal in der Innenfläche, `(0, −0.0020, 0.0113)` | Radien `(0.0230, 0.0245, 0.0140)` — bündig, deshalb ein Farbfeld und keine Beule | Kontaktfläche |
| Bein ×2 | Kapsel | `(±0.085, 0.158, 0)` → `(±0.098, 0.076, 0)` | Radius `0.044` | Graphit |
| Fuß ×2 | Box mit Rundung | `(±0.105, 0.047, 0.020)` | Halbmaße `(0.032, 0.004, 0.050)`, Radius `0.042` → **L 0.184 · B 0.148** | Perlweiß |

### Finger

Aus der frühen Handkugel sind vier Finger und ein Daumen geworden — dieselbe Formensprache,
keine Kante, keine Fuge. Jeder Finger besteht aus zwei Kapseln und einer Kuppe und krümmt
sich um zwei Gelenke: das Grundglied um `c · 0.95`, das Mittelglied zusätzlich um `c · 1.15`.
Der stärker gekrümmte zweite Abschnitt ist der Grund, warum die Hand einen Haken bildet und
nicht nur eine Schaufel.

Alle Werte lokal zur Handmitte. `Spreizung` ist der Faktor, mit dem der Fingerabstands-Kanal
auf diesen Finger wirkt.

| Finger | Wurzel | Grundglied | Mittelglied | Radius | Spreizung |
|---|---|---|---|---|---|
| Zeigefinger | `(−0.0165, −0.0195, 0.0075)` | `0.0228` | `0.0176` | `0.0075` | `+1.5` |
| Mittelfinger | `(−0.0055, −0.0215, 0.0080)` | `0.0247` | `0.0189` | `0.0077` | `+0.5` |
| Ringfinger | `(0.0055, −0.0205, 0.0075)` | `0.0234` | `0.0179` | `0.0074` | `−0.5` |
| kleiner Finger | `(0.0160, −0.0180, 0.0065)` | `0.0195` | `0.0150` | `0.0069` | `−1.5` |
| **Daumen** | `(−0.0230, −0.0020, 0.0130)` | `0.0202` | `0.0163` | `0.0083` | eigener Oppositionswinkel |

Die Fingerlänge ist **nicht frei gewählt**. Die gekrümmten Glieder beschreiben einen
Kreisbogen, und dessen freier Innenradius ist alles, was die Hand umschließen kann. Bei
kürzeren Fingern läge er bei `0.0071` — Noki könnte einen Bleistift halten und sonst nichts.
Mit den Werten oben sind es `0.0108`, und die Finger sind etwa so lang, wie der Ballen hoch
ist. Das ist zugleich menschliche Proportion.

Der Daumen hat einen eigenen Kanal, den **Oppositionswinkel**: eine Drehung um `z` um seine
Wurzel. Er unterscheidet den Zangengriff vom Faustgriff deutlicher als jede Krümmung.

Die Armteile sind lokal zum jeweiligen Schultergelenk und werden für die linke Seite an der
`x`-Achse gespiegelt. Der eingebaute Versatz nach außen (`x` wächst nach unten) gibt den
Armen ihren leicht abgespreizten Stand — auch ohne jede Animation.

---

## Aufgemalte Details

Diese Elemente sind **bewusst keine Geometrie**. Sie liegen als Muster auf der Oberfläche und
lassen sich dadurch frei animieren, ohne ein einziges Polygon zu verformen. Wer Noki als Mesh
baut, setzt sie als Textur oder als Shader-Maske um — nicht als Modellierung.

| Detail | Wo | Beschreibung |
|---|---|---|
| **Augenringe** | Visierglas, `(±0.104, 0.255)` lokal | Ringradius `0.062`, Ringstärke `0.0092`. Darin eine dunkle, glänzende Pupillenkugel mit zwei Reflexen |
| Displayraster | Visierglas | Horizontale Linien, nur in Augennähe zu ahnen |
| Brustscheibe | Rumpf, `(0, 0.367, vorn)` | Vertiefte Scheibe Radius `0.072` mit dunklem Rand |
| **Brustring** | in der Brustscheibe | Radius `0.040`, leuchtet in Bernstein — das Echo der Augen |
| Mundlinie | Rumpf, `y ≈ 0.251` | Breite `0.156`, krümmt sich mit der Stimmung |
| Helmnähte | Kopfschale, lokal `y = 0.098` und `0.386` | Zwei feine Fugen rund um den Kopf |
| **Nackenprägung** | Kopfrückseite, lokal `(0, 0.205, hinten)` | Plakette mit Strichmarken — die Kennung N‑01 |
| **Schulterdelle** | rechte Schulterplatte, `(0.232, 0.352, 0.040)` | Abgeriebene Stelle mit feinen Schleifspuren |

Das Visierlicht **verlischt bei streifendem Blick** (Grenzwinkel `0.13…0.46` von `n·v`). So
verhält es sich wie ein echtes Display und steht nicht als harte Leuchtkante auf der
Silhouette, wenn Noki im Profil steht.

---

## Materialien

Fünf Materialien an der Figur, mehr nicht. Jede weitere Oberfläche verwässert sie.

| Material | Grundfarbe | Rauheit | Metallanteil | Wo |
|---|---|---|---|---|
| **Perlweiß** | `#EEEFF3` | 0.29 | 0.00 | Kopfschale, Körper, Ober-/Unterarme, Füße |
| **Graphit-Chrom** | `#151719` | 0.18 | 0.90 | Hals, Ohrkapseln, Antennen, Gelenke, Hände, Beine |
| **Visierglas** | `#060708` | 0.07 | 0.10 | Visier. Umgebungsspiegelung auf 35 % gedämpft |
| **Glimm** | `#6B4F2B` | 0.32 | 0.00 | Antennenperle. Emission = Stimmungsfarbe × 1.35 |
| **Kontaktfläche** | `#2F3135` | 0.62 | 0.05 | Handteller und Fingerkuppen. Matt statt glänzend — dort sieht man den Druckpunkt und kein Spiegelbild |

Die Beleuchtung ist ein Studio-Setup: warmes Hauptlicht von links oben vorn, kühles
Aufhelllicht von rechts, Kantenlicht von hinten, dazu zwei weiche Flächenlichter in der
Umgebung, damit das Chrom etwas zu spiegeln hat.

---

## Farbpalette

### Der Grundton

| | Hex | Verwendung |
|---|---|---|
| **Bernstein** | `#FFB65C` | Augenringe, Glimm, Brustring — Nokis Identitätsfarbe |

### Stimmungsverschiebungen

Die Leuchtfarbe verlässt die warme Familie nur an einer Stelle: bei Traurigkeit. Das ist
Absicht — der Bruch ist das Signal.

| Zustand | Hex | Warum |
|---|---|---|
| Grundzustand | `#FFB65C` | warm, wach |
| Überrascht | `#FFD18C` | heller, entsättigt — als würde er kurz überstrahlen |
| Müde | `#F29E4F` | tiefer, gesättigter, deutlich gedimmt |
| Traurig | `#94B8EB` | kühl — der einzige Kaltton, den Noki je zeigt |

### Bühne

| | Hell | Dunkel |
|---|---|---|
| Hintergrund oben | `#D8DCE2` | `#0E1014` |
| Hintergrund unten | `#B3B8C0` | `#050609` |

Kühle Neutrale mit leichter Blaubeimischung — damit das warme Augenlicht der einzige
Farbreiz im Bild bleibt.

---

## Do und Don't

**Ja**

- Runde Grundkörper, große Eckradien, weiche Übergänge
- Weiß als Hauptfläche, Graphit nur an Gelenken und Funktionsteilen
- Bernstein ausschließlich für Licht — nie als Lackfläche
- Asymmetrie oben (Antennen), Symmetrie unten (Körper, Beine)

**Nein**

- Keine scharfen Kanten, keine Fasen, keine Facetten
- Keine weitere Farbfläche, keine Muster, keine Aufkleber
- Keine sichtbaren Schrauben, Nieten, Kabel oder Lüftungsgitter
- Keine zweite lange Antenne — die Asymmetrie ist der Charakter
- Kein kaltes Cyan, auch nicht als Akzent

---

## Nächster Schritt

[03 · Rig und Animation](03-rig-und-animation.md) — Hierarchie, Drehpunkte, Grenzen und das
Gefühlssystem.
