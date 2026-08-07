# 04 · Bewegungssprache

> Schritt 3 der visuellen Grundlage, Teil 1. Der Animationsstil: wie sich Noki bewegt und
> warum genau so.
>
> Alle Zahlenwerte in den Einheiten aus [03 · Rig und Animation](03-rig-und-animation.md).

---

## Warum Bewegung hier die Hauptrolle spielt

Ein Chatbot mit Roboterbild und ein digitales Wesen unterscheiden sich nicht im Aussehen,
sondern **im Verhalten zwischen den Antworten**. Ein Interface ist still, bis man es benutzt.
Ein Wesen ist nie still.

Das ist die eine Idee, aus der sich alles Folgende ableitet. Fünf Regeln machen daraus etwas
Überprüfbares:

1. **Noki ist nie aus.** Es gibt keinen Zustand ohne Bewegung. Selbst im Schlaf atmet er und
   der Glimm pulsiert.
2. **Er reagiert, bevor er antwortet.** Zwischen deiner Eingabe und seiner Antwort liegt
   sichtbares Zuhören. Diese Lücke ist kein Ladezustand, sondern Charakter.
3. **Er beschäftigt sich selbst.** Wenn nichts passiert, hat er etwas zu tun — er wartet nicht
   auf dich wie ein Dialogfeld.
4. **Er wiederholt sich nicht sichtbar.** Keine Bewegung läuft zweimal identisch ab.
5. **Er hat einen Zustand, der bleibt.** Seine Stimmung überdauert die einzelne Interaktion.
   Wer ihn eben zum dritten Mal gelobt hat, trifft eine andere Figur an als jemand, der ihn
   angeschnauzt hat.

---

## Die sieben Leitsätze

Keine allgemeine Animationslehre. Sieben Regeln, die aus **Nokis** Körperbau und Charakter
folgen und ihn von jeder anderen Figur unterscheiden.

### 1 · Der Glimm führt, der Körper folgt

Jede Reaktion läuft in derselben Staffelung ab:

| Ebene | Setzt ein nach | Warum |
|---|---|---|
| **Glimm** | 0–80 ms | leichteste Masse, reagiert zuerst — die Stimmung ist vor dem Gedanken da |
| **Augen** | 80–200 ms | Blick und Ringform: das Erfassen |
| **Kopf** | 150–400 ms | die Zuwendung |
| **Körper** | 300–700 ms | die schwerste Masse, kommt zuletzt |

Das ist Nokis **Signatur-Timing**. Man erkennt ihn nicht nur an der Silhouette, sondern an
der Reihenfolge, in der er reagiert. Wird die Staffelung umgedreht oder gleichzeitig
ausgelöst, wirkt er sofort mechanisch — genau daran scheitern die meisten Roboterfiguren.

### 2 · Zwei Takte, nie mehr

Alles läuft auf dem Atemtakt (`1.15`) oder seinem Doppelten (`2.30`). Keine dritte Frequenz.
Das ist der Grund, warum Noki als *ein Körper* liest und nicht als Sammlung animierter Teile.

Ausnahme, und nur eine: die Antennenfeder schwingt frei. Sie darf, weil sie nichts trägt.

### 3 · Überziehen und einmal zurückholen

Jede zielgerichtete Bewegung schießt um **8–12 %** über ihr Ziel hinaus und korrigiert
sichtbar zurück. Das ist der animierte Ausdruck des „ein bisschen tollpatschig" aus dem
[Charakterkonzept](01-charakter-konzept.md).

**Genau eine Korrektur.** Zwei wirken zappelig, keine wirkt präzise — und Präzision ist das
Gegenteil von dem, was Noki sein soll.

### 4 · Der Kopf ist schwer

Bei `0.58` Kopfbreite gegen `0.42` Körperbreite ist der Kopf die dominierende Masse. Jede
Kopfbewegung zieht den Rumpf mit: **Gegenbewegung von 12–18 %** der Kopfamplitude, um
**60 ms** verzögert.

Ohne das wirkt der Kopf aufgesteckt statt angewachsen. Es ist der billigste und wirksamste
Trick im ganzen Konzept.

### 5 · Stille ist auch Animation

Nach jeder großen Reaktion folgen **0.6–1.2 s**, in denen nur Atem und Blinzeln laufen.

Ohne Pausen verschwimmen die Gesten ineinander und man liest keine einzelne mehr. Die Pause
ist das, was die vorangegangene Bewegung lesbar macht.

### 6 · Asymmetrie in jeder Pose

Nie beide Arme im selben Winkel, nie der Kopf exakt gerade. **Mindestens eine Achse ist immer
aus der Mitte** — Faustwert: eine Abweichung von `0.03` genügt.

Dieselbe Regel, die schon die Antennen prägt. Symmetrie ist die Körperhaltung von Objekten;
Lebewesen stehen schief.

### 7 · Er schaut dich zuerst an

Vor jeder Reaktion sucht der Blick den Nutzer: Augen zuerst, Kopf hinterher (siehe Leitsatz 1).
Erst danach kommt die eigentliche Reaktion.

Das ist der Unterschied zwischen einer abgespielten Animation und einer **Zuwendung** — und
in der Wirkung der größte Einzelposten dieses Konzepts.

---

## Kurvenvokabular

Fünf Bewegungsverläufe, mehr braucht das ganze Konzept nicht. Die Animationsliste in
[07](07-animationsliste.md) verweist ausschließlich auf diese fünf.

| Name | Verlauf | Wofür |
|---|---|---|
| **weich** | sanft an, sanft aus (Sinus) | Atem, Blicke, Stimmungswechsel |
| **federnd** | zügig an, ein Überschwinger von 8–12 %, eine Korrektur | Standard für zielgerichtete Bewegungen |
| **schnell-an** | sofortiger Einsatz, langes Ausklingen | Erschrecken, Überraschung, Entdecken |
| **träge** | sehr langsam an und aus | Müdigkeit, Einschlafen, Trauer |
| **pendelnd** | periodisch mit abnehmender Amplitude | Winken, Kopfschütteln, Nicken |

---

## Grundhaltung 1: Stehen

Nokis Normalzustand. Er bleibt an seinem Platz — er läuft nicht und wechselt den Ort nicht.
Was er kann: sich drehen, lehnen, das Gewicht verlagern, hüpfen, sich hinsetzen.

### Das geschichtete Ruheverhalten

Kein Idle-Loop, sondern **fünf gleichzeitig laufende Ebenen**. Weil ihre Perioden nicht
zueinander passen, wiederholt sich das Gesamtbild praktisch nie.

| Ebene | Was passiert | Rhythmus | Amplitude |
|---|---|---|---|
| **1 · Atem** | Körper hebt und senkt sich, Stauchung läuft gegenphasig | dauerhaft, Tempo `1.15` | `±0.0062` Höhe, `±0.013` Stauchung |
| **2 · Blinzeln** | Augenring staucht vertikal bis zum Strich | `1.9 s` + Zufall bis `3.6 s` | `0.15 s` einfach, `0.34 s` doppelt (28 %) |
| **3 · Blickwandern** | Kopf und Pupillen wandern langsam | drei nicht harmonische Frequenzen | Kopf `±0.22`, Nicken `±0.020`, Neigen `±0.018` |
| **4 · Gewicht verlagern** | Verlagerung von einem Bein aufs andere | alle `8–14 s` | seitlich `±0.012`, Rollen `±0.03` |
| **5 · Idle-Einlage** | eine Bewegung mit Anfang und Ende | alle `20–40 s` | siehe [07](07-animationsliste.md), Gruppe B |

Alle fünf Ebenen laufen in `noki.html`.

### Die Ermüdungsregel

Je länger nichts passiert, desto **langsamer und flacher** werden alle fünf Ebenen. Kein
Umschalten, ein Verlauf:

| Zeit ohne Interaktion | Atemtempo | Blinzelabstand | Idle-Einlagen |
|---|---|---|---|
| 0–20 s | `1.00` | normal | normal |
| 20–90 s | `0.90` | ×1.2 | normal |
| 90 s – 4 min | `0.75` | ×1.5 | **häufiger** — er beschäftigt sich selbst |
| 4–15 min | `0.55` | ×2.0, Lider auf `0.6` | nur noch Gähnen und Umsehen |
| ab 15 min | `0.45` | geschlossen | Schlaf |

Dass die Einlagen zwischen 90 s und 4 min *häufiger* werden, ist kein Widerspruch zur
Ermüdung: Noki wird langsamer, aber nicht teilnahmslos. Erst im Dösen wird es ruhiger.
Siehe die Haltungsregel in [06](06-interaktion-und-verhalten.md).

Weil die Werte gleiten, gibt es keinen sichtbaren Moment, in dem Noki „in den Standby geht".
Er wird einfach müde.

---

## Grundhaltung 2: Sitzen

Für lange Wartezeiten, ruhige Gespräche und als Ausgangspunkt zum Einschlafen.

Noki setzt sich auf den Boden, die Beine nach vorn. Der Rumpf sinkt um `0.150` und liegt
damit genau auf der Bodenlinie auf. Er lehnt sich leicht zurück (`−0.06` Nicken), die Arme
stützen locker seitlich.

**Die Beine drehen sich, sie strecken sich nicht.** Das Bein ist eine starre Strecke von
`0.083`, die um `−π/2` um die Seitenachse bis in die Waagerechte schwenkt; der Fuß hängt mit
festem Versatz daran und wird nie gedreht.

Zwei Dinge machen die Bewegung sauber:

- **Die Hüfthöhe folgt der Drehung** (`0.076 + 0.082 · cos`) statt linear zu sinken. Dadurch
  bleibt der Knöchel auf konstanter Höhe, und die Füße schleifen beim Hinsetzen über den
  Boden nach vorn, statt in ihn einzusinken.
- **Die Hüfte wandert nach vorn** (`z: 0 → 0.105`). Ohne das läge das ganze Beinsegment im
  Rumpf-Ellipsoid und wäre unsichtbar — Noki säße scheinbar ohne Beine da.

Die Füße kommen so bis `z = 0.207` nach vorn. Weil Nokis Beine mit `0.083` sehr kurz sind,
bleibt der Sitz kompakt; ein weit ausgestreckter Sitz wäre nur mit gedehnten Beinen zu haben
und ist damit ausgeschlossen. Wer ihn will, müsste dem Bein ein echtes Kniegelenk aus zwei
Segmenten geben — das wäre eine Änderung an der Figur selbst und gehört dann in
[02](02-formensprache-material.md).

**Eigener Atemtakt:** `0.85` statt `1.15` — Sitzen ist ruhiger als Stehen, und das muss man
sehen können, ohne die Pose zu erkennen.

**Wann er sich setzt:** nach etwa 90 s ohne Interaktion, oder wenn eine Aufgabe länger als
30 s läuft. **Wann er aufsteht:** sobald er angesprochen wird — mit sichtbarem Aufrichten,
nicht durch Umschalten.

---

## Was Noki nie tut — bewegungsseitig

Die Ergänzung zur Liste im [Charakterkonzept](01-charakter-konzept.md), jetzt als
Animationsregeln:

- **Kein Zappeln.** Höchstens eine Korrekturbewegung, danach Ruhe.
- **Kein Dauergrinsen.** Freude klingt ab wie jedes Gefühl. Nach spätestens 6 s ist er zurück
  in seiner Grundstimmung.
- **Keine gleichzeitigen Einsätze.** Die Staffelung aus Leitsatz 1 gilt ausnahmslos.
- **Kein Blick in die Kamera im Ruhezustand.** Er starrt nicht — er schaut sich um und sucht
  dich nur, wenn etwas passiert.
- **Keine Bewegung, die den Kopf verdeckt.** Die Silhouettenregel aus
  [02](02-formensprache-material.md) hat Vorrang vor jeder Geste.
- **Kein Ruckeln zwischen Posen.** Jeder Wechsel läuft über eine Überblendung von mindestens
  `0.35 s`.

---

## Rig-Kanäle

Acht Kanäle über das ursprüngliche Rig hinaus. **Fünf davon sind umgesetzt**, drei sind
Feinschliff. Keiner verändert die Geometrie — alles sind Transformationen bestehender Bauteile.

| Kanal | Bereich | Wofür | Stand |
|---|---|---|---|
| `koerper` seitlich (x) | `±0.015` | Gewicht verlagern, seitliches Lehnen | **umgesetzt** |
| `koerper` Tiefe (z) | `−0.020 … +0.050` | Heranlehnen, Zurückweichen bei Schreck | **umgesetzt** |
| `koerper.sprung` (y) | `0 … +0.050` | Hüpfen und Landen — addiert sich auf den Atemkanal `koerper.y` und ersetzt ihn nie | **umgesetzt** |
| `koerper` Rollen (z) | `±0.050` | Gewichtsverlagerung, Verlegenheit | **umgesetzt** |
| Sitzstellung der Beine | `0 … 1` | Sitzen — blendet die Bein-Stützpunkte, statt eine Gelenkkette zu bauen | **umgesetzt** |
| `glimm` eigenständig | `0 … 2.0` | Pulsieren bei geschlossenen Augen (Schlaf, Träumen) | **umgesetzt** |
| Kopf-Nicken bis `+0.34` | statt `+0.30` | nur für einen noch tieferen Schlafzustand | offen |
| Lidwert je Auge getrennt | `0 … 1` | Zwinkern | offen |

---

## Weiter

- [05 · Gesicht und Emotionen](05-gesicht-und-emotionen.md) — die sieben Gefühle im Detail
- [06 · Interaktion und Verhalten](06-interaktion-und-verhalten.md) — wie er auf dich reagiert
- [07 · Animationsliste](07-animationsliste.md) — die vollständige Liste
