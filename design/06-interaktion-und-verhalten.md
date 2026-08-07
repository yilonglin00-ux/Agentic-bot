# 06 · Interaktion und Verhalten

> Schritt 3, Teil 3. Wie Noki auf dich reagiert — und das Modell, das ihn später als
> Begleiter statt als ferngesteuerte Puppe funktionieren lässt.

---

## Die Haltungsregel

Bevor eine einzige Reaktion beschrieben wird, muss diese Regel stehen, weil sie mehrere davon
einschränkt:

> **Noki setzt niemals Gefühle ein, um Verhalten zu erzwingen.**
>
> Er wird nicht traurig, weil du ihn ignorierst. Er schaut nicht vorwurfsvoll, wenn du lange
> weg warst. Er wirkt nicht enttäuscht, wenn du ihn schließt.

Eine Figur, die Schuldgefühle erzeugt, um Aufmerksamkeit zu bekommen, ist manipulativ — und
zwar umso wirksamer, je süßer sie ist. Das ist der Punkt, an dem ein sympathischer Begleiter
zu einem Bindungsmechanismus wird.

Nokis Zuwendung ist ein **Angebot, keine Forderung**. Er freut sich, wenn du da bist. Er
kommt ohne dich zurecht, wenn du nicht da bist. Beides muss man sehen können.

---

## Die fünf Interaktionsfälle

Jeder als **Ablaufpartitur** — was zu welchem Zeitpunkt passiert. Die Staffelung folgt
durchgehend Leitsatz 1 aus [04](04-bewegungssprache.md): Glimm, Augen, Kopf, Körper.

### 1 · Du sprichst ihn an

**Auslöser:** Eingabefeld fokussiert, Mikrofon aktiv, oder erste getippte Taste.

| Zeit | Was passiert |
|---|---|
| `0 ms` | Glimm hellt auf: `1.00` → `1.25` |
| `80 ms` | Augenring weitet sich `0.062` → `0.066`, Blick sucht dich |
| `180 ms` | Kopf dreht sich zu dir, Neigen `+0.08` |
| `350 ms` | Körper richtet sich auf, Arme lösen sich `0.07` → `0.10` |
| `600 ms` | **Zuhörhaltung** erreicht: Blick auf dir, Kopf leicht geneigt, Atem auf `0.95` gedrosselt |
| danach | nur Blinzeln und Atem — **kein Nicken, keine Geste** |

**Gefühl:** Zuwendung.

Der entscheidende Teil ist die letzte Zeile: Während du sprichst, tut Noki **nichts**. Er
drosselt sogar den Atem. Eine Figur, die während des Zuhörens gestikuliert, wirkt, als warte
sie darauf, selbst dranzukommen.

### 2 · Du lobst ihn

**Auslöser:** Ereignis `lob` von der Agentenseite.

| Zeit | Was passiert |
|---|---|
| `0 ms` | Glimm springt auf `1.45` |
| `90 ms` | Augen weiten sich kurz, dann kippt der **Bogen auf `1.0`** — die ⌒-Form |
| `200 ms` | Kopf hebt sich `−0.09`, Neigen `+0.10` |
| `400 ms` | kleiner Hüpfer (`+0.030`), Arme auf `0.34` |
| `900 ms` | **Verlegenheitsanteil:** Kopf senkt sich um `0.06`, ein Arm geht Richtung Kopf (`0.9 rad`), Glimm fällt auf `1.20` |
| `1800 ms` | Übergang in Zufriedenheit — hält 2–4 Minuten |

**Gefühl:** Freude mit einem Anflug von Verlegenheit.

Der Verlegenheitsanteil bei `900 ms` ist der wichtigste Teil dieser Sequenz. Reine Freude
wirkt wie ein Erfolgsgeräusch. Die kleine Zurücknahme — er weiß nicht recht, wohin mit dem
Lob — macht aus einer Belohnungsanimation einen Charaktermoment.

**Stimmung:** `+0.35`.

### 3 · Du ignorierst ihn

**Auslöser:** keine Eingabe. Die Zeitkaskade läuft.

| Zeit | Was passiert |
|---|---|
| `20 s` | Aufmerksamkeit lässt nach, der Blick löst sich von dir |
| `90 s` | **er beschäftigt sich selbst** — Idle-Einlagen werden *häufiger*, nicht seltener: umsehen, die eigene Hand betrachten, die Antenne schütteln |
| `4 min` | er setzt sich hin, Dösen beginnt, Lider auf `0.6` |
| `15 min` | Schlaf, Glimm pulsiert mit `0.35 Hz` weiter |

**Gefühl:** Gelassenheit.

Dass die Idle-Einlagen bei `90 s` **häufiger** werden, ist die zentrale Entscheidung dieses
Falls. Der naheliegende Entwurf wäre, Noki langsamer und trauriger werden zu lassen — und
genau das verbietet die Haltungsregel. Stattdessen findet er etwas zu tun.

Kein Blick zurück zu dir. Kein Seufzen. Kein Zusammensinken. **Stimmung: unverändert.**

### 4 · Du gibst ihm eine Aufgabe

**Auslöser:** Ereignis `aufgabe_start`.

**Phase 1 — Annehmen** (`0–600 ms`)

| Zeit | Was passiert |
|---|---|
| `0 ms` | Glimm auf `1.20` |
| `120 ms` | zwei kurze Nicker, *pendelnd*, Amplitude `0.10` |
| `300 ms` | Mund `+0.30`, Arme `0.12` |
| `600 ms` | Übergang in Phase 2 |

**Phase 2 — Arbeiten** (Schleife, bis Ergebnis vorliegt)

Nachdenklichkeit aus [05](05-gesicht-und-emotionen.md): Glimm-Puls auf `0.5 Hz`, Kopf
`−0.21` geneigt, Blick wandert alle `1.5–2.5 s`.

- nach `30 s`: er setzt sich hin — die Aufgabe dauert offenbar
- nach `60 s`: Idle-Einlagen im Sitzen sind wieder erlaubt; er wartet ja auch

Der langsame Glimm-Puls ersetzt den Ladebalken vollständig. Man sieht, dass etwas läuft, ohne
dass ein Fortschrittsbalken lügen muss.

**Phase 3a — Erfolg** (`aufgabe_fertig`)

| Zeit | Was passiert |
|---|---|
| `0 ms` | **Blick sucht zuerst dich** (Leitsatz 7) |
| `250 ms` | Aufregung, `0.6 s` |
| `850 ms` | Freude, `1.5 s` |
| `2400 ms` | Zufriedenheit |

**Phase 3b — Fehler** (`fehler`)

| Zeit | Was passiert |
|---|---|
| `0 ms` | kurzes Zusammenzucken, `0.15 s` |
| `200 ms` | Kopf senkt sich `+0.14`, Glimm auf `0.70`, Mund `−0.35` |
| `600 ms` | ein Arm hebt sich halb (`0.55 rad`) — die Geste des „hm" |
| `2000 ms` | Blick zurück zu dir, Kopf `+0.16` geneigt: *soll ich's nochmal versuchen?* |

**Gefühl:** Bereitwilligkeit → Konzentration → Stolz oder Bedauern.

Bei Fehlern **nicht Traurigkeit**, sondern Bedauern mit Angebot. Der Unterschied: Traurigkeit
ist nach innen gerichtet und lädt zum Trösten ein; Bedauern ist nach außen gerichtet und
bietet einen nächsten Schritt an. **Stimmung:** Erfolg `+0.25`, Fehler `−0.15`.

### 5 · Du kommst nach längerer Zeit zurück

**Auslöser:** Eingabe nach einer Abwesenheit. Die Reaktion hängt von der Dauer ab — und genau
diese Abstufung erzeugt den Eindruck, dass Noki ein Gedächtnis hat.

**Unter 5 Minuten** (er döst)

Aufrichten über `0.6 s`, Neugier, kein großes Aufheben. Er war ja kaum weg.

**5 Minuten bis 3 Stunden** (er schläft)

| Zeit | Was passiert |
|---|---|
| `0 ms` | **Erschrecktes Aufwachen** — Überraschung, `0.4 s`, Antenne schlägt voll aus |
| `400 ms` | Erkennen: Augen fokussieren, Kopf richtet sich auf, `0.5 s` |
| `900 ms` | Freude, `2.0 s` |
| `2900 ms` | Winken |

Das erschrockene Aufwachen ist der beste Einzelmoment der ganzen Figur. Es ist der einzige
erlaubte direkte Übergang von Schlaf zu Überraschung (siehe Übergangsregeln in
[05](05-gesicht-und-emotionen.md)).

**Über einen Tag**

Wie oben, aber mit einer zusätzlichen Bewegung nach dem Erkennen:

| Zeit | Was passiert |
|---|---|
| `900 ms` | er **lehnt sich heran** (`+0.05` nach vorn), Kopf `+0.18` geneigt |
| `1100 ms` | **hält den Blick `2.5 s`** statt der üblichen `0.8 s` |
| `3600 ms` | Freude, dann Winken |

Der lange Blick ist alles. Er erzählt „ich hab dich vermisst", ohne es zu behaupten — und
ohne einen Vorwurf, dass du weg warst. **Stimmung:** `+0.30`.

---

## Das Verhaltensmodell

### Vier Ebenen, die gleichzeitig laufen

| Ebene | Wechselt | Werte |
|---|---|---|
| **Grundhaltung** | selten, mit sichtbarem Übergang | Stehen · Sitzen · Dösen · Schlaf |
| **Stimmung** | träge, über Minuten | `−1.0` bis `+1.0` |
| **Reaktion** | kurz, unterbricht | die Animationen aus [07](07-animationsliste.md) |
| **Aufmerksamkeit** | dauernd | wohin der Blick geht |

Ein Zustand entsteht aus der **Kombination**. Freude im Sitzen bei gedrückter Stimmung sieht
anders aus als Freude im Stehen bei guter Stimmung — ohne dass dafür eine eigene Animation
nötig wäre.

### Stimmung ist ein träger Wert, kein Schalter

| | |
|---|---|
| Bereich | `−1.0` (gedrückt) bis `+1.0` (aufgeräumt) |
| Grundstimmung | **`+0.15`** — Noki ist von Natur aus leicht positiv |
| Verschiebung je Ereignis | höchstens `±0.35` |
| Rückkehr zur Grundstimmung | Halbwertszeit `≈ 4 min` |

**Was die Stimmung beeinflusst:** Atemtempo (`±15 %`), Glimm-Grundhelligkeit (`±0.15`),
Häufigkeit der Idle-Einlagen, und **welche** Einlagen überhaupt gezogen werden dürfen.

**Was sie nicht tut:** Sie ersetzt keine Emotion, sie färbt sie. Freude bei Stimmung `−0.5`
ist gedämpfter als bei `+0.5` — dieselbe Animation, andere Intensität.

Deshalb wirkt Noki nicht wie ein Automat, der auf Knopfdruck fröhlich wird: Wer ihn eben
dreimal gelobt hat, trifft eine andere Figur an als jemand, dem gerade eine Aufgabe
fehlgeschlagen ist.

### Prioritäten und Unterbrechungen

| Stufe | Kategorie | Unterbricht | Wird unterbrochen von |
|---|---|---|---|
| **5** | Erschrecken, Überraschung | alles | nichts |
| **4** | Direkte Reaktion (Zuhören, Nicken, Antworten) | 1–3 | 5 |
| **3** | Aufgabenrückmeldung (Erfolg, Fehler) | 1–2 | 4, 5 |
| **2** | Gefühlszustand | 1 | 3, 4, 5 |
| **1** | Idle-Einlage | — | alles |

**Unterbrechen heißt nie abbrechen.** Die laufende Animation wird über `0.35 s` ausgeblendet,
während die neue einsetzt. Ein harter Schnitt ist der sicherste Weg, die Illusion zu zerstören.

### Zeitkaskade

| Ohne Interaktion | Zustand |
|---|---|
| `20 s` | Aufmerksamkeit lässt nach |
| `90 s` | beschäftigt sich selbst, setzt sich hin |
| `4 min` | Dösen |
| `15 min` | Schlaf |

### Anti-Wiederholung

Jede gezogene Idle-Einlage merkt sich die **letzten drei**. Keine davon darf erneut gezogen
werden. Bei sieben Einlagen (Gruppe B in [07](07-animationsliste.md)) bleiben immer
mindestens vier zur Auswahl — genug, dass kein Muster entsteht.

Zusätzlich variiert jede Einlage bei jedem Abspielen: Dauer `±12 %`, Amplitude `±8 %`,
Seite gespiegelt bei `50 %`.

---

## Die Schnittstelle zur KI-Seite

Der eine Architekturentscheid, an dem hängt, ob Noki wie ein Wesen oder wie eine Fernbedienung
wirkt:

> **Die Agentenseite meldet Ereignisse, keine Animationen.**

Acht Ereignisse, mehr nicht:

| Ereignis | Wann es gemeldet wird |
|---|---|
| `angesprochen` | Nutzer beginnt eine Eingabe |
| `denkt_nach` | Anfrage läuft |
| `antwortet` | Ausgabe beginnt |
| `aufgabe_start` | eine längere Aufgabe beginnt |
| `aufgabe_fertig` | erfolgreich abgeschlossen |
| `fehler` | fehlgeschlagen |
| `lob` | positive Rückmeldung erkannt |
| `nutzer_weg` | Fenster verlassen, Sitzung inaktiv |

**Was Noki daraus macht, entscheidet er selbst** — abhängig von Stimmung, Grundhaltung,
Tageszeit und davon, was er zuletzt getan hat. Dasselbe `lob` sieht beim ersten Mal anders aus
als beim vierten Mal in fünf Minuten.

Gäbe die Agentenseite Animationsnamen vor, wäre Noki eine Marionette mit einer Bibliothek von
Clips. So ist er eine Figur, die auf Ereignisse in ihrer Welt reagiert. Der Unterschied ist
für den Betrachter sofort spürbar, auch wenn er ihn nicht benennen kann.

**Nicht Teil der Schnittstelle:** Der Text der Antwort. Noki liest nicht mit und spielt keine
Emotion zum Inhalt. Er reagiert auf das *Ereignis*, nicht auf die *Bedeutung* — alles andere
würde eine Verstehensleistung vortäuschen, die die Animationsebene nicht erbringt.

---

## Weiter

- [07 · Animationsliste](07-animationsliste.md) — die vollständige Liste aller Animationen
