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

| Dokument | Inhalt |
|---|---|
| [01 · Charakter-Konzept](design/01-charakter-konzept.md) | Wer Noki ist: Name, Herkunft, Persönlichkeit, Ausstrahlung, Wiedererkennbarkeit |
| [02 · Formensprache und Material](design/02-formensprache-material.md) | Der verbindliche Maßkanon: jedes Bauteil mit Position, Maß und Material |
| [03 · Rig und Animation](design/03-rig-und-animation.md) | Hierarchie, Drehpunkte, Grenzen, Bewegungsprinzipien, Gefühlssystem |
| [04 · Animationsreferenz](design/04-animationsreferenz.md) | Vorbereitetes Analyseraster — wartet auf das Referenzvideo |

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
- [ ] Analyse des Referenzvideos und Übertragung des Bewegungsstils
- [ ] Entscheidung über die weitere Umsetzung (Mesh in Blender oder Echtzeit-Renderer)
