# 07 · Animationsliste

> Schritt 3, Teil 4. **38 Animationen** in sieben Gruppen — die vollständige Liste für die
> spätere Umsetzung.
>
> Jeder Eintrag trägt Name, Beschreibung, Auslöser, Gefühl und eine Spezifikationszeile:
> `Dauer · Kurvenform · betroffene Kanäle · Priorität · unterbrechbar`
>
> Kurvennamen aus [04](04-bewegungssprache.md), Kanalnamen und Grenzen aus
> [03](03-rig-und-animation.md), Prioritätsstufen aus [06](06-interaktion-und-verhalten.md).
> Ein **▶** markiert Animationen, die in `noki.html` bereits laufen (Stufe 1).
> Ein **✚** markiert Animationen, die einen noch fehlenden Rig-Kanal brauchen.

---

## A · Grundzustände

Immer genau einer davon aktiv. Sie laufen unter allem anderen weiter.

**A1 · Atem-Idle** ▶
Der Normalzustand im Stehen. Körper hebt und senkt sich, die Stauchung läuft gegenphasig, die
Arme schwingen seitenverkehrt mit. Darüber liegen Blinzeln, Blickwandern und Gewichtsverlagerung.
*Auslöser:* dauerhaft, sobald Noki steht — *Gefühl:* Anwesenheit, Ruhe
`Zyklus 5.5 s · weich · koerper.y ±0.0062, koerper.stauchung ±0.013, arme ±0.016 gegenläufig · Prio 0 · läuft immer`

**A2 · Sitz-Idle** ▶
Noki sitzt, Beine nach vorn, Arme stützen locker seitlich. Deutlich ruhiger als A1 — man muss
den Unterschied hören können, ohne die Pose zu sehen.
*Auslöser:* nach 90 s ohne Interaktion, oder wenn eine Aufgabe länger als 30 s läuft — *Gefühl:* Gelassenheit
`Zyklus 7.4 s · weich · wie A1 mit halber Amplitude, beinwinkel −π/2, hüfte nach vorn 0.105, koerper −0.150, rumpfneigung −0.06 · Prio 0 · läuft immer`

**A3 · Dösen** ▶
Die Lider sinken, der Glimm dimmt, alles verlangsamt sich. Kein Umschalten — die Ermüdungsregel
lässt die Werte gleiten.
*Auslöser:* 4 min ohne Interaktion — *Gefühl:* Müdigkeit, Geborgenheit
`Übergang 2.0 s · träge · lid 0.60, glimm 0.55, atem 0.55, kopf.nicken +0.12 · Prio 0 · läuft immer`

**A4 · Schlaf** ▶
Die Augen sind zu einem waagerechten Strich gestaucht, der Glimm pulsiert langsam weiter. Noki
ist nicht ausgeschaltet — er schläft.
*Auslöser:* 15 min ohne Interaktion — *Gefühl:* Zufriedenheit, Vertrauen
`Übergang 3.5 s · träge · lid 0, glimm 0.25 @ 0.35 Hz, atem 0.45, kopf.nicken +0.30 · Prio 0 · läuft immer`

---

## B · Idle-Einlagen

Bewegungen mit Anfang und Ende, alle 20–40 s gezogen. Die Anti-Wiederholungsregel aus
[06](06-interaktion-und-verhalten.md) merkt sich die letzten drei.

**B1 · Umsehen** ▶
Der Kopf dreht sich langsam zur Seite, der Blick läuft voraus, der Körper folgt minimal
gegenläufig. Die häufigste Einlage.
*Auslöser:* Idle-Ziehung — *Gefühl:* Wachheit, Interesse an der Umgebung
`2.4 s · weich · kopf.drehen ±0.28, blick führt 90 %, koerper Gegenbewegung 15 % · Prio 1 · ja`

**B2 · Gewicht verlagern** ▶
Verlagerung von einem Bein aufs andere, mit leichtem Rollen des Rumpfs. Winzig, aber die
wirksamste Einzelbewegung gegen den Eindruck einer aufgestellten Figur.
*Auslöser:* alle 8–14 s automatisch, nicht Teil der Ziehung — *Gefühl:* Körperlichkeit
`1.8 s · weich · koerper.x ±0.012, koerper.rollen ±0.03 · Prio 1 · ja`

**B3 · Antenne schütteln** ▶
Ein kurzes, schnelles Kopfschütteln, nur damit die Antenne ausschwingt — als wolle er sie
zurechtrücken. Sein persönlichster Tick.
*Auslöser:* Idle-Ziehung, erhöhte Wahrscheinlichkeit bei guter Stimmung — *Gefühl:* Eigenart, Verspieltheit
`1.1 s · pendelnd · kopf.drehen ±0.14 in 3 Schwingungen, Antennenfeder folgt frei · Prio 1 · ja`

**B4 · Strecken**
Beide Arme nach oben-außen, der Körper hebt sich kurz, der Kopf legt sich zurück. Danach
sackt alles wohlig zusammen.
*Auslöser:* Idle-Ziehung, nur ab Ermüdungsstufe 2 — *Gefühl:* Behaglichkeit
`2.6 s · federnd · arme auf 1.85, koerper.sprung +0.020, kopf.nicken −0.16, dann zurück · Prio 1 · ja`

**B5 · Etwas entdecken** ▶
Der Kopf fährt herum, die Augen weiten sich, der Körper geht ein Stück nach vorn — als hätte
er am Rand etwas bemerkt. Löst sich nach kurzem Schauen wieder auf.
*Auslöser:* Idle-Ziehung, erhöhte Wahrscheinlichkeit nach 90 s ohne Interaktion — *Gefühl:* Neugier
`1.6 s · schnell-an · kopf.drehen 0.34, augen 0.070, glimm 1.25, koerper.z +0.03 · Prio 1 · ja`

**B6 · Gähnen** ▶
Die Augen kneifen sich zu, die Mundlinie zieht sich nach unten, der Kopf legt sich zurück,
danach ein langes Blinzeln.
*Auslöser:* Idle-Ziehung, nur ab Ermüdungsstufe 3 — *Gefühl:* Müdigkeit
`2.2 s · träge · lid auf 0.15, mund −0.60, kopf.nicken −0.10, arme 0.30 · Prio 1 · ja`

**B7 · Hand betrachten** ▶
Er hebt eine Hand vor sich und schaut sie an, den Kopf leicht gedreht. Die Bewegung, die am
deutlichsten sagt: *er beschäftigt sich selbst.*
*Auslöser:* Idle-Ziehung, erhöhte Wahrscheinlichkeit ab 90 s ohne Interaktion — *Gefühl:* Selbstgenügsamkeit
`3.0 s · weich · arm_r 1.35 nach vorn, kopf.nicken −0.10 und drehen 0.18, Blick auf die Hand · Prio 1 · ja`

**B8 · Ein paar Schritte** ▶
Ein Watschelgang auf der Stelle, während der Boden unter ihm durchzieht. Die Beine schwingen
gegenphasig, die Hüfte sinkt mit der Spreizung, Arme und Kopf schwingen gegen. Ohne Kniegelenk
ist es der Gang eines Aufziehspielzeugs — und genau der passt zu Noki.
*Auslöser:* Knopf, oder Idle-Ziehung ab 60 s ohne Interaktion — *Gefühl:* Eigenständigkeit, Tatendrang
`5–9 s · pendelnd (65 % Dreieck) · beinschwung ±0.42 @ 0.72 Zyklen/s, hüfte −0.082·(1−cos α), boden += |Δfuß|, koerper.rollen ±0.030, arme ∓0.30 · Prio 1 · ja`

**B9 · Hüpfen** ▶
Ein kleiner Hopser aus dem Stand: leicht in die Knie, abspringen, kurz fliegen, landen,
abfedern, stehen. Die einzige Bewegung, bei der Noki den Boden verlässt — und die einzige,
die keinen Zustand hinterlässt: sie endet exakt in der Haltung, in der sie begonnen hat.
*Auslöser:* Knopf im Reiter *Bewegung*. Bewusst **nicht** in der Idle-Ziehung: ein Hopser aus
dem Nichts wirkt nervös, nicht verspielt — *Gefühl:* Verspieltheit, Übermut
`1.5 s · weich (Landung schnell-an) · hüfte −0.55 / knie 1.10 in der Hocke, koerper.sprung +0.060 als Wurfparabel, arm_l/r +0.34, arm_vor +0.22, kopf.nicken −0.070, stauchung ∓0.016 · Prio 1 · nein`

---

## C · Begrüßung und Abschied

**C1 · Winken** ▶
Der rechte Arm schwenkt nach außen-oben und pendelt. Nach außen, nicht gerade hoch — sonst
verschwindet die Hand hinter dem Kopf.
*Auslöser:* erste Begrüßung, Ende von C2/C3, oder Rückkehr nach kurzer Abwesenheit — *Gefühl:* Zuwendung, Freundlichkeit
`2.8 s · pendelnd · arm_r 2.00 ±0.28 @ 6.4, kopf.neigen +0.10, bogen 0.85, mund +0.85 · Prio 4 · nein`

**C2 · Aufwachen und Erkennen** ▶
Schreck, dann Fokus, dann Aufrichten. Der beste Einzelmoment der Figur — der einzige erlaubte
direkte Übergang von Schlaf zu Überraschung.
*Auslöser:* Eingabe nach 5 min bis 3 h Abwesenheit — *Gefühl:* Überraschung, die in Freude kippt
`1.4 s · schnell-an → weich · Überraschung 0.4 s → Fokus 0.5 s → Aufrichten 0.5 s · Prio 5 · nein`

**C3 · Wiedersehen** ▶
Wie C2, danach lehnt er sich heran und **hält den Blick 2.5 s** statt der üblichen 0.8 s. Der
lange Blick erzählt „ich hab dich vermisst", ohne es zu behaupten — und ohne Vorwurf.
*Auslöser:* Eingabe nach über einem Tag Abwesenheit — *Gefühl:* Wiedersehensfreude
`5.4 s · weich · C2 + koerper.z +0.05, kopf.neigen +0.18, Blick 2.5 s halten, dann D1 und C1 · Prio 5 · nein`

**C4 · Verabschieden**
Ein kleineres, langsameres Winken als C1, der Glimm dimmt dabei bereits. Kein Bedauern, keine
Bitte zu bleiben.
*Auslöser:* Ereignis `nutzer_weg` bei bewusstem Schließen — *Gefühl:* freundliche Gelassenheit
`1.9 s · pendelnd · arm_r 1.60 ±0.20, kopf.neigen +0.12, bogen 0.60, glimm sinkt auf 0.80 · Prio 4 · nein`

---

## D · Gefühle

Vollständig beschrieben in [05 · Gesicht und Emotionen](05-gesicht-und-emotionen.md); hier
nur Auslöser und Zeitverhalten.

**D1 · Freude** ▶
Die Augen kippen in die ⌒-Form — Noki sieht in diesem Moment nicht, er ist ganz bei seinem Gefühl.
*Auslöser:* `lob`, `aufgabe_fertig`, gelungene Interaktion — *Gefühl:* warme, ruhige Freude
`Einsatz 0.25 s · federnd · halten 1.5–3.0 s · Abklingen 1.2 s, spätestens nach 6 s zurück · Prio 2 · ja`

**D2 · Neugier** ▶
Kopf zur Seite, Augen weit offen, Körper minimal vorgeschoben. Nokis wiedererkennbarste Pose.
*Auslöser:* Nachfrage, unbekannte Eingabe, B5 — *Gefühl:* Interesse
`Einsatz 0.30 s · federnd · hält offen · Abklingen 0.8 s · Prio 2 · ja`

**D3 · Überraschung** ▶
Größter Augenring, hellster Glimm, Körper weicht zurück. Die **einzige** Animation, bei der
die Staffelung aus Leitsatz 1 entfällt: alles setzt gemeinsam ein.
*Auslöser:* unerwartetes Ereignis, E6, C2 — *Gefühl:* Schreck
`Einsatz 0.09 s · schnell-an · halten 0.4 s · Abklingen 0.8 s, nie länger als 1.3 s gesamt · Prio 5 · nein`

**D4 · Nachdenklichkeit** ▶
Kleinster Augenring, Blick nach unten links und wandernd, Glimm-Puls auf 0.5 Hz. Ersetzt den
Ladebalken vollständig.
*Auslöser:* `denkt_nach`, F2 — *Gefühl:* Konzentration
`Einsatz 0.50 s · träge · hält offen · Abklingen 0.6 s · Prio 2 · ja`

**D5 · Traurigkeit**
Der einzige Kaltton der Figur. Klingt sehr langsam ab — Traurigkeit lässt sich nicht wegklicken.
*Auslöser:* nur bei echtem Anlass im Gespräch. **Nie**, weil der Nutzer abwesend ist — *Gefühl:* stille Betroffenheit
`Einsatz 0.90 s · träge · hält offen · Abklingen 4–8 s · Prio 2 · ja`

**D6 · Aufregung** ▶
Weit aufgerissene Augen, springender Blick, schnellster Glimm-Puls, kleine Hüpfer. Das
Gegenstück zu D1: Freude geht nach innen, Aufregung nach außen.
*Auslöser:* `aufgabe_fertig` mit gutem Ergebnis, überraschend Positives — *Gefühl:* ungerichtete Begeisterung
`Einsatz 0.18 s · federnd · halten 1.0–2.5 s · Abklingen 1.5 s · Prio 2 · ja`

**D7 · Zufriedenheit** ▶
Bogen nur angeschnitten, Puls gleichmäßig. Nokis zweite Ruhestellung — er bleibt hier
minutenlang, bevor er nach Neutral zurückfällt.
*Auslöser:* Abklingen von D1/D6, gelungene Interaktion — *Gefühl:* Ausgeglichenheit
`Einsatz 0.60 s · weich · hält sehr lange · Abklingen 1.5 s · Prio 2 · ja`

---

## E · Reaktionen

**E1 · Zuhören** ▶
Kopf leicht geneigt, Blick auf dem Nutzer, Atem gedrosselt — und **sonst nichts**. Eine Figur,
die während des Zuhörens gestikuliert, wirkt, als warte sie darauf, dranzukommen.
*Auslöser:* `angesprochen` — *Gefühl:* Aufmerksamkeit
`Einsatz 0.6 s, hält bis Eingabeende · weich · kopf.neigen +0.08, atem 0.95, arme 0.10 · Prio 4 · nein`

**E2 · Nicken** ▶
Zwei kurze Nicker mit abnehmender Amplitude, der Körper federt gegenläufig mit.
*Auslöser:* Zustimmung, Bestätigung im Gesprächsverlauf — *Gefühl:* Einverständnis
`0.9 s · pendelnd · kopf.nicken +0.10 ×2, koerper Gegenbewegung 15 % · Prio 4 · nein`

**E3 · Kopfschütteln**
Zweieinhalb Schwingungen, die Mundlinie zieht leicht nach unten. Bewusst kleiner als E2 —
Ablehnung soll nicht dominant wirken.
*Auslöser:* Verneinung, „das geht nicht" — *Gefühl:* freundliches Bedauern
`1.0 s · pendelnd · kopf.drehen ±0.16 ×2.5, mund −0.20 · Prio 4 · nein`

**E4 · Bestätigen** ▶
Ein Glimm-Aufblitzen, ein einzelner Nicker, kurzer Bogen in den Augen. Die kürzeste Reaktion
im ganzen Satz — für alles, was nur quittiert werden muss.
*Auslöser:* `antwortet` beginnt, Eingabe angenommen — *Gefühl:* „verstanden"
`0.7 s · federnd · glimm 1.30 kurz, kopf.nicken 0.12, mund +0.40, bogen 0.40 · Prio 4 · nein`

**E5 · Nachfragen** ▶
Stärkste Kopfneigung des ganzen Satzes, Augen leicht geweitet, Körper einen Hauch vor. Der
Blick bleibt auf dir stehen und wartet.
*Auslöser:* mehrdeutige Eingabe, fehlende Angabe — *Gefühl:* freundliche Ratlosigkeit
`1.2 s · federnd · kopf.neigen +0.24, augen 0.068, koerper.z +0.02, Blick hält · Prio 4 · nein` ✚

**E6 · Erschrecken**
Körper zuckt zurück und hoch, Arme fahren aus, Antenne schlägt voll aus. Höchste Priorität —
unterbricht ausnahmslos alles.
*Auslöser:* plötzliches lautes Ereignis, abrupte Rückkehr aus dem Schlaf — *Gefühl:* Schreck
`0.7 s · schnell-an · koerper.z −0.020, koerper.sprung +0.015, arme 0.52, augen 0.080, antenne voll · Prio 5 · nein`

---

## F · Aufgaben

**F1 · Aufgabe annehmen** ▶
Zwei kurze Nicker, der Glimm zieht an, die Mundlinie hebt sich. Signalisiert Bereitschaft,
bevor irgendetwas passiert.
*Auslöser:* `aufgabe_start` — *Gefühl:* Bereitwilligkeit
`0.6 s · pendelnd · kopf.nicken 0.10 ×2, glimm 1.20, mund +0.30, arme 0.12 · Prio 3 · nein`

**F2 · Arbeiten** ▶
Die Warteschleife: D4 mit langsamem Glimm-Puls und wanderndem Blick. Nach 30 s setzt er sich
hin, nach 60 s sind Idle-Einlagen im Sitzen wieder erlaubt — er wartet ja auch.
*Auslöser:* läuft zwischen `aufgabe_start` und dem Ergebnis — *Gefühl:* Konzentration, dann geduldiges Warten
`Schleife · träge · D4 + glimm 0.5 Hz, Blick wandert alle 1.5–2.5 s, ab 30 s A2 · Prio 2 · ja`

**F3 · Erfolg melden** ▶
Der Blick sucht **zuerst** dich, erst danach kommt die Freude. Diese Reihenfolge ist der
ganze Unterschied zwischen einer Erfolgsanimation und einem geteilten Moment.
*Auslöser:* `aufgabe_fertig` — *Gefühl:* Stolz, der geteilt werden will
`3.0 s · federnd · Blick zu dir 0.25 s → D6 0.6 s → D1 1.5 s → D7 · Prio 3 · nein`

**F4 · Fehler melden** ▶
Zusammenzucken, Kopf senken, halb gehobener Arm — die Geste des „hm". Nach 2 s der Blick
zurück zu dir mit geneigtem Kopf: *soll ich's nochmal versuchen?*
*Auslöser:* `fehler` — *Gefühl:* Bedauern mit Angebot, ausdrücklich **nicht** Traurigkeit
`2.6 s · schnell-an → träge · Zucken 0.15 s, kopf.nicken +0.14, glimm 0.70, mund −0.35, arm 0.55, dann kopf.neigen +0.16 · Prio 3 · nein`

---

## G · Besondere Momente

**G1 · Stolz** ▶
Kopf hoch, Brust vor, Augenbogen nur angeschnitten. Zufrieden, aber nicht überschwänglich —
derselbe Parameter wie bei Freude, nur halb so weit.
*Auslöser:* nach mehreren Erfolgen hintereinander, oder bei Stimmung über `+0.6` — *Gefühl:* leiser Stolz
`2.0 s · federnd · kopf.nicken −0.17, bogen 0.55, koerper.z +0.03, arme 0.15, glimm 1.22 · Prio 2 · ja`

**G2 · Verlegen**
Kopf senkt und dreht sich weg, ein Arm geht Richtung Kopf, der Rumpf rollt leicht. Der Teil,
der Lob erst sympathisch macht — er weiß nicht recht, wohin damit.
*Auslöser:* Bestandteil der Lob-Reaktion, oder nach einem eigenen Missgeschick — *Gefühl:* Verlegenheit
`1.8 s · weich · kopf.nicken +0.10 und drehen 0.20 weg, arm_r 0.90, koerper.rollen 0.04, glimm 1.15 · Prio 2 · ja`

**G3 · Heranlehnen** ▶
Der ganze Körper geht nach vorn, der Kopf neigt sich, der Blick bleibt lange. Nokis Art, Nähe
zu zeigen, ohne den Platz zu verlassen.
*Auslöser:* Bestandteil von C3; bei sehr guter Stimmung auch als Idle-Einlage — *Gefühl:* Zugewandtheit
`1.5 s · weich · koerper.z +0.05, kopf.neigen +0.18, augen 0.068, Blick 2.5 s halten · Prio 2 · ja`

**G4 · Träumen**
Im Schlaf schlägt der Glimm gelegentlich stärker aus und die Antenne zuckt minimal. Der
Beweis, dass da drin jemand ist.
*Auslöser:* läuft in A4, unregelmäßig alle 8–20 s — *Gefühl:* Lebendigkeit im Ruhezustand
`Schleife · träge · glimm 0.25 → 0.45 kurz, antenne ±0.01, kopf.nicken ±0.02 · Prio 0 · nur in A4`

---

## Abdeckung der Aufgabenstellung

| Geforderter Punkt | Abgedeckt durch |
|---|---|
| Idle-Animationen | A1–A4, B1–B7 |
| Begrüßung | C1, C2, C3 |
| Reaktionen | E1–E6, F1, F4 |
| **Freude** | D1 |
| **Neugier** | D2, B5 |
| **Überraschung** | D3, E6, C2 |
| **Nachdenklichkeit** | D4, F2 |
| **Traurigkeit** | D5 |
| **Aufregung** | D6 |
| **Schlaf / Zufriedenheit** | A4 und D7 — bewusst getrennt: eine schlafende und eine wache Ausprägung desselben warmen, langsamen Gefühls |
| Bewegungen | B2, B4, B7, G3, A2 (Sitzen), C1 |
| Besondere Momente | G1–G4, C3 |
| *angesprochen werden* | E1 |
| *gelobt werden* | D1 + G2 (Ablauf in [06](06-interaktion-und-verhalten.md)) |
| *ignoriert werden* | A3, A4, B5, B7 (häufigere Einlagen, keine Traurigkeit) |
| *Aufgabe bekommen* | F1–F4 |
| *Rückkehr nach langer Zeit* | C2, C3, G3 |

---

## Produktionsreihenfolge

Nicht alle 36 müssen gleichzeitig entstehen. Drei Stufen, jede für sich schon vorzeigbar:

**Stufe 1 — Noki lebt** ▶ **umgesetzt** (13 Animationen)
A1, **A2**, A3, A4, B1, B2, B3, **B8**, D1, D2, D4, E1, C1

Damit hat er ein Ruheverhalten, kann sitzen, gehen, einschlafen, zuhören, sich freuen,
nachdenken und winken. Das ist bereits ein Begleiter, mit dem sich sinnvoll interagieren lässt.

*A2 · Sitz-Idle rückte aus Stufe 2 vor: A4 · Schlaf setzt die Sitzhaltung voraus, sonst
schliefe Noki im Stehen.*

**Stufe 2 — Noki reagiert** ▶ **umgesetzt**
C2, C3, D3, D6, D7, E2, E5, **E7 Zeigen**, F1, F2, F3, F4, G1, G3, **G5 Dankbarkeit**

Dazu neu: das **Sequenzsystem** mit neun Abläufen und das **Greifsystem** mit vier
Griffarten, sieben Phasen und vier gebauten Gegenständen — beides beschrieben in
[09 · Stufe 2 und Greifsystem](09-stufe2-und-greifsystem.md).

Noch offen aus Gruppe B: B5 Etwas entdecken, B6 Gähnen, B7 Hand betrachten.

Sitzen, Aufwachen, die vollständige Aufgabenschleife und alle Gefühle bis auf Traurigkeit.

**Stufe 3 — Noki hat Tiefe**
B4, B5, B6, B7, C4, D5, E3, E4, E6, G2, G4

Die seltenen Momente. Man sieht sie nicht oft — aber sie sind der Grund, warum die Figur nach
Wochen noch lebendig wirkt.

---

## Rig-Bedarf im Überblick

Von den acht Kanälen aus der Lückenliste in [04 · Bewegungssprache](04-bewegungssprache.md)
sind **sechs umgesetzt**. Damit stehen alle Kanäle bereit, die die Stufen 1 und 2 brauchen.

| Kanal | Betrifft | Stand |
|---|---|---|
| `koerper` seitlich (x) | B2 | **umgesetzt** |
| `koerper` Tiefe (z) | B5, E5, E6, G1, G3, C3 | **umgesetzt** |
| `koerper.sprung` | B4, B9, D6, E6 | **umgesetzt** |
| `koerper` Rollen (z) | B2, G2 | **umgesetzt** |
| Beinwinkel (Sitzstellung) | A2, C2 | **umgesetzt** |
| `glimm` eigenständig | A4, G4 | **umgesetzt** |
| Kopf-Nicken bis `+0.34` | tieferer Schlaf | offen, nur Feinschliff |
| Lidwert je Auge | Zwinkern | offen, nur Feinschliff |

Keiner davon verändert die Geometrie — alle sind Transformationen bestehender Bauteile.

---

## Prüfbarkeit

Die umgesetzten Animationen lassen sich einzeln ansteuern:

```
noki.html#still=1&ui=0&pose=sitzen          Grundhaltung einfrieren
noki.html#still=1&ui=0&clip=B1&cu=0.5       Einlage an einem Zeitpunkt
noki.html#still=1&ui=0&achtung=1            Zuhör-Haltung
noki.html#still=1&ui=0&huepf=0.59           Hüpfen an einem Zeitpunkt (0 … 1)
noki.html#selftest=1                        Selbsttest über 1200 simulierte Sekunden
```

Der Selbsttest prüft alle 16 Rig-Kanäle gegen die Grenzen aus
[03](03-rig-und-animation.md), die Reihenfolge der Zeitkaskade, die Anti-Wiederholung,
die Sprungfreiheit jedes Kanals und den Verlauf der Stimmung.

---

## Weiter

Stufe 1 läuft. Der nächste Schritt wären die 13 Animationen der Stufe 2 — die Rig-Kanäle
dafür stehen bereits, es fehlen nur noch die Bewegungen selbst.
