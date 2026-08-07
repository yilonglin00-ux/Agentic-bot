# 04 · Animationsreferenz — Analyseraster

> **Status: vorbereitet, noch nicht gefüllt.**
>
> Schritt 3 der Aufgabenstellung beginnt, sobald das Referenzvideo vorliegt. Dieses Dokument
> ist das Raster, nach dem die Analyse dann abläuft — damit sie strukturiert passiert und
> nicht in „sieht nett aus" endet.

---

## Wie die Übertragung ablaufen wird

Die Vorgabe lautet ausdrücklich: **den Bewegungs*stil* übernehmen, aber einen eigenen
entwickeln.** Deshalb ist die Analyse zweistufig — erst beschreiben, was das Referenzvideo
tut, dann entscheiden, was davon zu Noki passt.

```
1  Beobachten      →  Was macht die Referenz? Wertfrei protokollieren.
2  Prinzip ableiten→  Welche Regel steckt dahinter?
3  Übersetzen      →  Wie sieht diese Regel an Nokis Körper aus?
4  Abgrenzen       →  Was wird bewusst NICHT übernommen und warum?
```

Schritt 4 ist der wichtigste. Er ist der Unterschied zwischen einer Kopie und einem eigenen
Stil.

---

## Analyseraster

### 1 · Bewegungsstil (Gesamteindruck)

| Frage | Beobachtung | Ableitung für Noki |
|---|---|---|
| Grundtempo — hektisch, ruhig, schwerfällig? | | |
| Beschleunigungskurven — hart oder weich? | | |
| Gewicht — schwebt die Figur oder hat sie Masse? | | |
| Wie stark wird überzogen (Anticipation, Overshoot)? | | |
| Ruhezustand — steht sie still oder lebt sie? | | |
| Wie viel Squash & Stretch? | | |

### 2 · Körperbewegung

| Frage | Beobachtung | Ableitung für Noki |
|---|---|---|
| Woher startet eine Bewegung — Hüfte, Brust, Kopf? | | |
| Läuft eine Welle durch den Körper oder bewegt sich alles gleichzeitig? | | |
| Wie stark neigt/verlagert sich der Rumpf? | | |
| Gibt es Gewichtsverlagerung beim Stehen? | | |

### 3 · Gesichtsausdruck

| Frage | Beobachtung | Ableitung für Noki |
|---|---|---|
| Wie schnell wechseln Ausdrücke? | | |
| Gibt es Zwischenstufen oder Sprünge? | | |
| Welche Ausdrücke wiederholen sich am häufigsten? | | |
| Wird der Ausdruck vor oder nach der Körperbewegung gesetzt? | | |

### 4 · Augenbewegung

| Frage | Beobachtung | Ableitung für Noki |
|---|---|---|
| Blinzelrhythmus — Frequenz, Dauer, Doppelblinzeln? | | |
| Führen die Augen den Kopf oder folgen sie? | | |
| Sakkaden (Sprünge) oder weiches Gleiten? | | |
| Wie wird Aufmerksamkeit gezeigt? | | |

### 5 · Gestik

| Frage | Beobachtung | Ableitung für Noki |
|---|---|---|
| Wie groß sind die Gesten im Verhältnis zum Körper? | | |
| Symmetrisch oder einseitig? | | |
| Wie kehren die Arme in die Ruhelage zurück? | | |
| Gibt es wiederkehrende Handformen? | | |

### 6 · Signature-Bewegungen

Die zwei bis drei Bewegungen, an denen man die Referenzfigur sofort erkennt.

| # | Beschreibung | Übernehmen? | Nokis Entsprechung |
|---|---|---|---|
| 1 | | | |
| 2 | | | |
| 3 | | | |

---

## Was schon feststeht

Diese Punkte gelten unabhängig vom Referenzvideo und sind aus dem Charakterkonzept
abgeleitet. Sie sind der Rahmen, in den die Analyse hineinpassen muss:

- **Noki ist nie hektisch.** Auch übernommene schnelle Bewegungen werden in seinem Tempo
  gedacht — Freude zeigt sich in der Amplitude, nicht in der Frequenz.
- **Die Antenne wird nie direkt animiert.** Was auch immer sich in der Referenz mitbewegt —
  bei Noki entsteht es aus der Feder, nie aus Keyframes.
- **Kein Sprung von Pose zu Pose.** Jeder Wechsel läuft über eine Überblendung.
- **Die Silhouettenregel bricht nie.** Keine übernommene Geste darf den Kopf verdecken oder
  den Arm hinter dem Kopf verschwinden lassen.
- **Es gibt keine Mimik im Gesicht** — nur Augen, Mundlinie und Kopfhaltung. Was die Referenz
  über Augenbrauen oder Mundformen löst, muss über diese drei Kanäle neu erfunden werden.

---

## Vorhandene Bausteine

Was für die Analyse schon bereitsteht und nicht neu gebaut werden muss:

| Baustein | Wo | Zustand |
|---|---|---|
| Neun Ausdruckszustände | `noki.html`, `EMO` | läuft |
| Blinzelautomatik | `updateRig()` | läuft |
| Atem- und Armschwingung | `updateRig()` | läuft |
| Antennenfeder | `updateRig()` | läuft |
| Winken als Geste | `EMO.winken` | läuft |
| Drehgrenzen aller Knochen | [03 · Rig](03-rig-und-animation.md) | dokumentiert |

Noch nicht vorhanden und vermutlich aus der Referenz abzuleiten: **Laufen**, **Drehen im
Stand**, **Erschrecken**, **Zeigen**, **Nicken/Kopfschütteln**, **Einschlafen und Aufwachen**.
