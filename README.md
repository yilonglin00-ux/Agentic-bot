# Noki

<img src="design/renders/noki-3-4.png" alt="Noki in der Dreiviertelansicht" width="380">

**Noki** ist die visuelle Grundlage einer virtuellen Figur, die später als KI-Begleiter
auftreten soll. Dieses Repository enthält bislang **ausschließlich das Design** — Identität,
Formensprache, Material und Bewegungsgrundlage. Noch keine Agenten-Logik, keine KI-Anbindung,
keine Integration.

---

## Noki ansehen

**[`noki.html`](noki.html)** im Browser öffnen. Die Datei ist vollständig eigenständig — keine
Installation, kein Server, keine externen Abhängigkeiten.

Noki wird in Echtzeit als dreidimensionales Distanzfeld berechnet. Er lässt sich frei um alle
360° drehen:

| Eingabe | Wirkung |
|---|---|
| Ziehen mit Maus oder Finger | frei drehen |
| **Horizontal scrollen** | um die Hochachse drehen |
| **Vertikal scrollen** | neigen (bis ±80°) |
| Zwei-Finger-Pinch, Strg+Scroll | zoomen |
| Ansichts-Schaltflächen | Vorne · Rechts · Hinten · Links · Oben · 3/4 |
| Ereignis-Schaltflächen | die acht Ereignisse, die später die KI-Seite meldet |
| Ausdrucks-Schaltflächen | neun Gefühlszustände und die Winkgeste |
| Zeitraffer ×10 | rafft die Zeitkaskade, damit Dösen und Schlaf in zwei Minuten sichtbar werden |

Noki steht dabei nie still: Er atmet, blinzelt in unregelmäßigem Rhythmus, sieht sich um, und
seine Stimmungsantenne schwingt jeder Kopfbewegung nach.

**Er verhält sich auch, wenn du nichts tust.** Nach 20 s lässt seine Aufmerksamkeit nach, nach
90 s setzt er sich hin und beschäftigt sich selbst, nach 4 Minuten döst er, nach 15 Minuten
schläft er — und der Glimm pulsiert dabei weiter. Sprichst du ihn an, dreht er dir den Kopf zu.
Lobst du ihn, hebt das seine Stimmung für Minuten, nicht für Sekunden.

Die Ereignis-Schaltflächen sind bewusst genau die Schnittstelle, die später ein KI-Agent
bedient: Sie melden **was passiert ist**, nicht **welche Animation laufen soll**. Was Noki
daraus macht, entscheidet er selbst.

---

## Dokumentation

**Wer Noki ist und wie er gebaut ist**

| Dokument | Inhalt |
|---|---|
| [01 · Charakter-Konzept](design/01-charakter-konzept.md) | Name, Herkunft, Persönlichkeit, Ausstrahlung, Wiedererkennbarkeit |
| [02 · Formensprache und Material](design/02-formensprache-material.md) | Der verbindliche Maßkanon: jedes Bauteil mit Position, Maß und Material |
| [03 · Rig und Animation](design/03-rig-und-animation.md) | Hierarchie, Drehpunkte, Grenzen, Bewegungsprinzipien, Gefühlssystem |

**Wie er sich bewegt und verhält**

| Dokument | Inhalt |
|---|---|
| [04 · Bewegungssprache](design/04-bewegungssprache.md) | Die sieben Leitsätze des Animationsstils, das geschichtete Ruheverhalten, Stehen und Sitzen |
| [05 · Gesicht und Emotionen](design/05-gesicht-und-emotionen.md) | Sieben Gefühle mit Augen, Mund, Kopf, Körpersprache und Glimm |
| [06 · Interaktion und Verhalten](design/06-interaktion-und-verhalten.md) | Die fünf Interaktionsfälle, das Verhaltensmodell und die Schnittstelle zur späteren KI-Seite |
| [07 · Animationsliste](design/07-animationsliste.md) | 36 Animationen mit Auslöser, Gefühl und Spezifikation |
| [08 · Anhang](design/08-anhang-referenzvideo.md) | Analyseraster, falls später ein Referenzvideo einfließen soll |

Die Maße in [02](design/02-formensprache-material.md) stimmen exakt mit `noki.html` überein.
Das Distanzfeld im Viewer ist nicht die Illustration der Dokumentation — es *ist* das Modell.

---

## Ansichten reproduzieren

Jede Kameraeinstellung lässt sich über die Adresse festlegen, etwa für Standbilder:

```
noki.html#yaw=90&pitch=20&dist=2.0&e=denkend&still=1&t=0&ui=0
```

`yaw`/`pitch` in Grad, `dist` Kameraabstand, `e` Ausdruckszustand, `still=1` friert jede
Bewegung ein, `ui=0` blendet die Bedienoberfläche aus, `theme` erzwingt `dark` oder `light`.
Dazu `pose=` (stehen · sitzen · doesen · schlaf), `clip=` mit `cu=` für eine Einlage an einem
bestimmten Zeitpunkt und `achtung=1` für die Zuhör-Haltung.

**Selbsttest:** `noki.html#selftest=1` fährt das Rig über 1200 simulierte Sekunden und prüft
alle 16 Kanäle gegen ihre Grenzen, die Reihenfolge der Zeitkaskade, die Anti-Wiederholung der
Idle-Einlagen, die Sprungfreiheit jedes Kanals und den Verlauf der Stimmung.

---

## Stand

- [x] Charakter-Identität
- [x] Formensprache, Maßkanon, Materialien
- [x] Frei drehbares 3D-Modell, aus allen Winkeln geprüft
- [x] Rig, Gefühlssystem, Idle-Animation
- [x] Animations- und Verhaltenskonzept
- [x] **Stufe 1 umgesetzt** — 12 Animationen, Verhaltensmodell, Zeitkaskade, Stimmung
- [ ] Stufe 2: 13 weitere Animationen (die Rig-Kanäle dafür stehen bereits)
- [ ] Stufe 3: die seltenen Momente
- [ ] Anbindung als interaktiver Begleiter

[07 · Animationsliste](design/07-animationsliste.md) markiert mit ▶, was bereits läuft, und
schlägt die Produktionsreihenfolge in drei Stufen vor.
