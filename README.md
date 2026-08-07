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
| Ausdrucks-Schaltflächen | neun Gefühlszustände und die Winkgeste |

Noki steht dabei nie still: Er atmet, blinzelt in unregelmäßigem Rhythmus, sieht sich um, und
seine Stimmungsantenne schwingt jeder Kopfbewegung nach.

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
Die vollständige Liste steht in [03 · Rig und Animation](design/03-rig-und-animation.md).

---

## Stand

- [x] Charakter-Identität
- [x] Formensprache, Maßkanon, Materialien
- [x] Frei drehbares 3D-Modell, aus allen Winkeln geprüft
- [x] Rig, Gefühlssystem, Idle-Animation
- [x] Animations- und Verhaltenskonzept
- [ ] Umsetzung: die fünf fehlenden Rig-Kanäle und Stufe 1 der Produktionsreihenfolge
- [ ] Anbindung als interaktiver Begleiter

Was im Viewer heute schon läuft, ist die Grundlage; das Konzept in
[04](design/04-bewegungssprache.md)–[07](design/07-animationsliste.md) beschreibt den
vollständigen Ausbau. [07 · Animationsliste](design/07-animationsliste.md) markiert mit ✚,
welche Animationen noch fehlende Rig-Kanäle brauchen, und schlägt eine Produktionsreihenfolge
in drei Stufen vor.
