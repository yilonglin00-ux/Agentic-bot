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

Nokis Normalzustand. Er **wechselt den Ort nicht** — die Bühne bleibt seine. Was er kann:
sich drehen, lehnen, das Gewicht verlagern, hüpfen, sich hinsetzen — und gehen.

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

### Gehen — ein Watschelgang, kein Menschengang

Noki hat **kein Kniegelenk**: sein Bein ist eine starre Strecke am Hüftgelenk. Einen
nachgeahmten Menschengang gäbe das nicht her — und er wäre auch falsch. Was stattdessen
herauskommt, ist der Gang eines Aufziehspielzeugs, und der passt zu dieser Figur besser als
jede Nachahmung.

| | |
|---|---|
| Schrittzyklen je Sekunde | `0.72` — ein Schritt dauert `0.69 s` |
| Ausschlag je Bein | `±0.42`, gegenphasig |
| Hüftabsenkung | `−0.082 · (1 − cos α)` — genau so viel, wie die gespreizten Beine kürzer werden |
| Rumpf rollt | `±0.030` zur Standseite |
| Rumpf wiegt seitlich | `±0.007` |
| Vorlage | `+0.045` |
| Arme | `∓0.30` gegenläufig zum Bein derselben Seite |
| Kopf | `∓0.045` gegen die Arme |
| Atem | `×1.25` |

**Das Schrittprofil ist eine zum Dreieck hin verzogene Sinuswelle** (65 % Dreiecksanteil).
Eine reine Sinusform beschleunigt ununterbrochen und liest sich als Schleichen; mit
Dreiecksanteil läuft der Abstoß mit gleichmäßiger Geschwindigkeit und die Umkehr wird zügig —
so sieht ein Schritt aus.

**Die Hüfte sinkt, statt dass die Füße rutschen.** Sie folgt exakt der Beinspreizung — dadurch
bleiben beide Füße auf dem Boden, und der Rumpf wippt zweimal je Schritt, weil die Beine
zweimal je Schritt zusammenkommen. Das ist keine Zutat, sondern fällt aus der Geometrie
heraus, sobald man die Beinlänge respektiert.

**Er geht auf der Stelle.** Der Boden trägt feine Querbänder, die unter ihm durchziehen —
ohne sie wäre ein Gang auf der Stelle von Stillstand nicht zu unterscheiden. Die Kamera
bleibt dadurch immer auf ihm, aus jedem Blickwinkel.

**Der Boden rückt um genau den Weg vor, den der abstoßende Fuß zurücklegt.** Nicht um einen
frei gewählten Betrag — das ist der Unterschied zwischen Treten und Rutschen. Rechnerisch:
`Boden += |Δ(0.083 · sin θ)|`. Damit steht in jedem Augenblick **genau ein Fuß still**
relativ zum Boden, während der andere nach vorn schwingt; welcher, wechselt mit jedem
Halbzyklus. Daraus ergibt sich die Ganggeschwindigkeit von `0.097` Einheiten je Sekunde — sie
ist eine Folge der Beinlänge, kein eingestellter Wert.

Der Selbsttest prüft das über zwei volle Zyklen: In über 90 % der Bilder muss ein Fuß auf
`10⁻⁹` genau stillstehen.

**Wann er geht:** auf Knopfdruck, und von selbst als Leerlauf-Einlage, sobald er länger als
60 s allein ist. Die selbstständigen Schritte setzen die Zeitkaskade **nicht** zurück — er
wird trotzdem müde und schläft am Ende ein.

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

---

## Sitzen und Aufstehen

Sitzen war lange ein **Zustand**: eine einzige Exponentialkurve zog alle Kanäle
gleichzeitig ans Ziel. Genau daher kam der Eindruck, die Figur wechsele zwischen zwei
Posen statt sich zu bewegen. Dazu kam ein Fehler, den niemand sehen konnte, ohne den
Code zu lesen: Die Rumpfabsenkung `posY = −0.150` lief als `u_pose.z` in den Shader und
**wurde dort nie gelesen**. Beim Sitzen drehte deshalb nur das Bein nach vorn, während
der Rumpf auf Stehhöhe blieb — Noki schwebte, und die Beine verschwanden im Bauch.

Beides ist behoben. Der Übergang hat jetzt eine eigene Zeitachse, und **jeder Kanal hat
seine eigene Kurve darüber**. Erst die versetzten Zeiten machen aus einer Überblendung
eine Bewegung.

### Hinsetzen — 1.5 s

| Abschnitt | Anteil | Was geschieht |
|---|---|---|
| Vorbereitung | 0.00 – 0.16 | Das Gewicht geht kurz nach hinten, die Knie lösen sich |
| Absenken | 0.16 – 0.68 | Knie beugen, Hüfte nach hinten und unten, Rumpf beugt zum Ausgleich **vor**, Arme heben mit |
| Aufsetzen | 0.68 – 0.80 | Das Gesäß setzt auf: kurzes Nachgeben, die Arme reagieren nach |
| Setzen | 0.80 – 1.00 | Der Rumpf richtet sich wieder auf, alles läuft weich aus |

Der Kopf läuft dem Rumpf hinterher — dieselbe Verzögerung, die auch den Blick zum
Gegenstand trägt.

### Aufstehen — 1.25 s

Es ist **nicht** die rückwärts gespielte Kurve. Zuerst neigt sich der Rumpf vor und der
Schwerpunkt wandert über die Füße, dann strecken die Knie, dann richtet sich der Rumpf
auf. Man steht anders auf, als man sich hinsetzt.

### Zwei Regeln, die der Selbsttest erzwingt

**Die Kurvensätze müssen an ihren gemeinsamen Endpunkten übereinstimmen.** Sonst springt
die Figur genau dann, wenn ein Ereignis sie mitten im Hinsetzen unterbricht und sie wieder
aufsteht. Der erste Entwurf ließ beim Hinsetzen einen Rest von `−0.016` in der Tiefe
stehen, den der Aufsteh-Satz nicht kannte — ein Sprung von 18 mm in einem Bild.

**Die Kurvenfunktion antwortet allein aus ihren Parametern.** Das Aufsetzen ist ein
Ereignis, kein Kurvenwert, und steht deshalb außerhalb. Solange es drinsteckte, hing das
Ergebnis am Zustand des Aufrufers, und dieselbe Eingabe lieferte zwei verschiedene
Antworten.

### Die Taste

Im Reiter *Bewegung* steht eine eigene Taste neben *Gehen*. Sie schaltet nur um; die
Bewegung ist dieselbe wie die selbsttätige. Drei Regeln:

- Der Klick setzt einen **Wunsch**, der der Zeitkaskade vorgeht — aber nur, solange
  diese nicht ohnehin tiefer will. Wer sich hinsetzt, darf danach von selbst dösen.
- Der Aufsteh-Wunsch ist **einmalig**: Sobald Noki steht, entscheidet wieder die
  Kaskade. Sonst bliebe sie für immer blockiert.
- Während der Bewegung ist die Taste **gesperrt**. Ein zweiter Klick mittendrin würde
  die Richtung kippen und den Ablauf zerreißen.

Wer losgeht, nimmt den Sitzwunsch zurück — sonst zögen beide gegeneinander.

### Sitz-Leerlauf

Sehr klein gehalten: eine langsame Gewichtsverlagerung (Periode ~11 s), ein minimales
Nachsacken im Atemrhythmus, dazu der ohnehin ruhigere Atem und das Blinzeln. Wer sitzt,
wackelt nicht.

### Was die Beinlänge nicht hergibt

Das Knie kann höchstens `0.037` über der Hüfte stehen — mehr lässt eine Beinlänge von
`0.083` bei aufsitzendem Rumpf nicht zu. Noki sitzt deshalb mit **leicht angewinkelten**
Beinen und flach aufliegenden Sohlen, nicht mit hochgezogenen Knien.

---

## Hinlegen und Liegen

Noki **fällt nicht um**. Er legt sich hin: mit sichtbarer Vorbereitung, mit
Gewichtsverlagerung, mit Armen, die mitstützen, und mit einem Rücken, der abrollt statt
aufzuschlagen. Der Ablauf dauert **2.2 s** — länger als das Hinsetzen, weil der Weg weiter
ist und die letzte Handbreit vor dem Boden langsam sein muss.

### Hinlegen — 2.2 s

| Abschnitt | Anteil | Was geschieht |
|---|---|---|
| Orientierung | 0.00 – 0.14 | Das Gewicht geht nach hinten, die Arme lösen sich vom Körper. Ein kurzer Moment — er soll bewusst wirken, nicht zögerlich |
| Absenken | 0.14 – 0.58 | Knie beugen bis in die **Hocke** — dieselben Beinwinkel wie beim Sitzen. Die Hüfte sinkt, die Hände gehen nach hinten und stützen mit |
| Ablegen | 0.58 – 0.86 | Der Rumpf rollt über Gesäß und Rücken ab, die Beine strecken sich nach vorn, das Kinn geht zur Brust |
| Auslegen | 0.86 – 1.00 | Der Kopf setzt auf und gibt kurz nach, die Glieder finden ihre Ruhelage |

Dass Noki durch die Hocke geht, ist keine Verzierung: Ohne sie kippte er aus dem Stand nach
hinten, und genau das soll die Bewegung nicht sein.

Der **Kopf** bekommt besondere Behandlung, weil er hier am meisten schaden kann. Er neigt
sich beim Zurücklegen nach vorn — Kinn zur Brust, wie jeder, der sich nicht den Hinterkopf
anschlagen will — und richtet sich erst nach dem Aufsetzen in die Ruhelage.

### Der Boden entscheidet, nicht die Kurve

Die Kurve legt fest, wie sich Noki **dreht**. Wie hoch er dabei liegt, entscheidet die
Bedingung *nichts unter `y = 0`* — je Bild neu, analytisch, aus der Drehkette des Shaders.
Das Abrollen entsteht dadurch von selbst: Der zurückgelegte Rücken ist dicker als das
Gesäß, also hebt die Bedingung den Rumpf beim Zurückneigen wieder an. Und der Kontaktpunkt
wandert ohne Zutun vom Gesäß über den Rücken zum Kopf. Die Rechnung dazu steht in
[03 · Rig und Animation](03-rig-und-animation.md).

Dasselbe gilt für die Hände: Die Kurve drückt sie gegen den Boden, die Bedingung schneidet
ab. Sie gleiten deshalb beim Abrollen am Boden entlang, statt einer eingetragenen Bahn zu
folgen — und können weder hindurchgreifen noch darüber schweben.

### Warum die Arme zur Seite gehen und erst spät sinken

Eine Bedingung, die abschneidet, ist nur so gut wie das, was sie abschneiden soll. Der
erste Entwurf schickte die Arme von Anfang an nach unten, und in der Hocke wurde das zur
Falle: Dort liegt die Hand bei der Abspreizung `0.16` für **jeden** sinnvollen Vorschwung
unter dem Boden — von `−2 mm` bei `armFw = −0.60` bis `−43 mm` bei `armFw = 0`. Es gab
keine nahe zulässige Stellung, also musste die Bodenbedingung über die ganze unzulässige
Zone hinwegspringen: bis zu `0.33 rad` in einem Bild, acht Zentimeter Handweg.

Zwei Dinge lösen das gemeinsam, und **nur** gemeinsam:

| Variante | größter Sprung je Bild | Dauerkorrektur |
|---|---|---|
| Arme früh nach unten (erster Entwurf) | 0.610 | 1.115 |
| nur stärker abspreizen | 0.655 | 0.662 |
| nur später sinken lassen | 0.089 | 0.514 |
| **beides** | **0.061** | **0.135** |

Die Abspreizung allein verschiebt den Sprung nur an eine andere Stelle. Das späte Sinken
allein lässt den Arm über weite Strecken der Zwangsbedingung statt der Kurve folgen.

Noki nimmt die Arme beim Absenken deshalb **zur Seite** — dieselbe Lösung, die das Sitzen
mit `armSw = 0.44` längst benutzt — und greift erst zum Boden, wenn der Rumpf schon fast
flach liegt. Im Rig gemessen bleiben davon `0.068` bis `0.077 rad` je Bild, in derselben
Klasse wie Hüfte und Knie.

> **Die Lehre:** Eine Zwangsbedingung kann Durchdringung verhindern, aber sie kann keine
> Bewegung erfinden. Wenn die Animation etwas Unmögliches verlangt, wird die Bedingung
> unstetig — und keine bessere Suche behebt das. Zwei Versuche an der Suche sind hier
> gescheitert, bevor die Ursache in der Kurve gefunden war.

### Aufrichten

Ein eigener Kurvensatz, kein Rückwärtsspielen: erst der Rumpf hoch, dann über die Hocke,
dann stehen. Die **ausgearbeitete** Aufstehbewegung aus dem Liegen kommt später; dieser Satz
sorgt vorerst dafür, dass das Liegen keine Sackgasse ist. Für die beiden Sätze gelten
dieselben zwei Regeln wie beim Sitzen — Endpunktgleichheit und weicher Richtungswechsel —,
und der Selbsttest erzwingt sie hier wie dort.

### Liege-Leerlauf

Noch kleiner als im Sitzen: Der Kopf sackt im Atemrhythmus minimal nach, die Arme atmen mit,
Blinzeln und Blick laufen unverändert weiter. Die Endlage der Hinlegebewegung **ist** der
Startzustand des Leerlaufs — kein Übergang, keine zweite Haltung, damit dazwischen nichts
springen kann. Damit ist sie zugleich ein sauberer Anfang für eine spätere Schlafanimation.

### Die Taste

Dritte Taste im Reiter *Bewegung*, nach demselben Muster wie *Sitzen*: Umschalter, Symbol
und Text wechseln mit dem Zustand, während der Bewegung gesperrt. Sitzt Noki gerade, **steht
er erst vollständig auf** — die vorhandene Aufstehbewegung läuft dabei ganz durch, sie wird
nicht abgekürzt. Solange er liegt, ist die Sitzen-Taste gesperrt: Aus dem Liegen setzt man
sich nicht hin, man richtet sich auf.

### Was die Proportionen nicht hergeben

Noki liegt nicht flach. Sein Kopf ist mit `0.270` dicker als sein Rumpf mit `0.190`; bei
`−90°` läge der Rücken auf und der Kopf hinge in der Luft. Die Figur liegt deshalb bei
**`−78°`** leicht angestellt — dann berühren Kopf und Rumpf gleichzeitig den Boden. Und weil
seine Beine mit `0.083` kurz sind gegenüber einem `0.190` dicken Rumpf, liegen sie nicht
flach ausgestreckt, sondern kommen vorn aus dem Rumpf heraus und ruhen mit den Sohlen auf
dem Boden. Beides ist Folge der Formensprache, nicht der Animation.

---

## Einschlafen, Schlafen und Aufwachen

Der Schlaf setzt genau dort an, wo das Hinlegen aufhört. Nicht ungefähr, sondern
buchstäblich: `schlafU` darf nur steigen, wenn `liegeU` bei `1` steht, und `liegeU` darf
nur fallen, wenn `schlafU` bei `0` steht. Die Kette **Stehen → Hinlegen → Schlafen →
Aufwachen → Aufstehen** ist damit erzwungen und nicht verdrahtet — es gibt keinen zweiten
Ort, an dem der Schlaf beginnen oder das Aufstehen einsetzen könnte.

### Einschlafen — 6.0 s

Was du zuerst siehst, ist gar keine neue Animation: Nach dem Hinlegen liegt Noki **wach**
im vorhandenen Liege-Leerlauf und sieht sich um. Erst nach etwa 20 s Ruhe wird er müde.

| Anteil | Abschnitt | Was geschieht |
|---|---|---|
| 0.00 – 0.30 | **Müdigkeit** | Das Blinzeln wird gedehnt, die Lider sinken auf etwa `0.75`, Blick und Kopf beruhigen sich |
| 0.30 – 0.62 | **Schwere Lider** | Auf etwa `0.35`. Der Atem wird langsamer, Arme und Finger lösen ihre Spannung |
| 0.62 – 0.88 | **Augen schließen** | Ganz zu. Der Glimm dimmt und pulst langsamer, der Kopf sackt nach |
| 0.88 – 1.00 | **Ankommen** | Der Körper entspannt sichtbar, alles läuft weich aus |

Das Blinzeln wird **nicht abgeschaltet, sondern gedehnt**: Der Abstand wächst auf das
Siebenfache, die Dauer auf das Dreifache, und ein Faktor im Blinzelprofil kappt den Sinus
oben ab — aus der Spitze wird ein Plateau bei „ganz zu". Genau das ist der Unterschied
zwischen *die Augen werden schwer* und *die Augen gehen aus*.

### Schlafen

Sehr klein und bewusst **deterministisch** statt zufällig: langsame Sinus mit unteilbaren
Perioden, wie `drift()` es im Haus schon macht. Nur dadurch kann der Selbsttest jede
Amplitude nach oben begrenzen — bei Zufall bliebe „nichts zuckt" eine Behauptung.

| Was | Größenordnung | Periode |
|---|---|---|
| Atem | langsamer **und tiefer** als im Wachliegen | ~10 s |
| Kopf drehen / neigen / nicken | ±0.020 / ±0.018 / ±0.009 | 23 s · 31 s · 37 s |
| Arme | ±0.010 | 19 s |
| Finger | ±0.030 | 43 s |
| Lidzittern | ±0.012 | 13 s |
| **Umlagern** | Gewicht, Schulter, Beinstellung — ein gedämpfter Ausschlag | selten, aus zwei unteilbaren Perioden überlagert |

Der Glimm pulst weiter, nur gedimmt und langsam. Das ist der Unterschied zwischen
*schlafend* und *ausgeschaltet*.

**Der Kopf darf sich dabei bewegen, ohne dass etwas abhebt.** Dreht er sich, wird einer von
Kopf und Rumpf zum tiefsten Punkt und der andere schwebt — bei `0.03` rad schon um gut fünf
Millimeter. Statt die Kopfbewegung deshalb winzig zu halten, wird die **Rumpfneigung
nachgeführt**, bis beide gleich tief liegen. Dieselbe Idee wie die abgeleitete Bodenhöhe,
eine Ebene höher — und erst sie macht die Kopfbewegung im Schlaf überhaupt möglich.

### Aufwachen — 5.5 s

Ein eigener Kurvensatz, kein Rückwärtsspielen. Das ist hier nicht Feinschliff, sondern der
Kern: Aufwachen hat eigene Schläge, die es beim Einschlafen nicht gibt.

| Anteil | Abschnitt | Was geschieht |
|---|---|---|
| 0.00 – 0.14 | **Erste Reaktion** | Ein tieferer Atemzug, die Finger regen sich, die Lider zittern — die Augen sind noch zu |
| 0.14 – 0.30 | **Erster Versuch** | Die Lider öffnen sich auf etwa `0.35` |
| 0.30 – 0.40 | **Rückfall** | Sie sinken wieder auf `0.12`, eine kurze Pause |
| 0.40 – 0.58 | **Zweiter Versuch** | Jetzt ganz auf, der Glimm kommt zurück |
| 0.52 – 0.80 | **Orientierung** | Der Blick geht nach links, nach rechts, dann nach vorn; der Kopf folgt verzögert |
| 0.62 – 0.92 | **Strecken** | Arme, Finger, Beine — ein Ausschlag, gedämpft, leicht verspielt |
| 0.92 – 1.00 | **Ankommen** | Alles läuft in die Wach-Liegehaltung aus |

Der **Rückfall** ist der Beat, der den ganzen Moment glaubwürdig macht. Ohne ihn ist es ein
Aufspringen, und der Selbsttest lässt ihn deshalb nicht weg: Er misst, dass sich die Augen
nach dem ersten Versuch wieder um mindestens `0.15` schließen.

Der verschlafene Ausdruck läuft über **Versätze**, nicht über einen Ausdruckswechsel: Lid,
Glimm und Mund wandern kurz in Richtung der Werte von `EMO.muede` und kehren zurück. Das
Menü und die Agentenlogik bleiben dabei unberührt.

Am Ende steht **wach + liegend**. Aufwachen führt ausdrücklich nicht zum Aufstehen — das
ist eine eigene Entscheidung, und der Selbsttest prüft, dass Noki sie nicht von selbst
trifft.

### Auslösung

Noki schläft **von selbst** ein, wenn er etwa 20 s ruhig liegt — über dieselbe Ruhe-Uhr,
die schon die Zeitkaskade trägt. Und weil jede Eingabe diese Uhr zurücksetzt, ist das
Wecken keine eigene Mechanik: Anfassen, Drehen, ein Ereignis oder ein Menüklick weckt ihn.

Dazu eine vierte Taste im Reiter *Bewegung*. Steht Noki noch, legt sie ihn erst hin und
lässt ihn dann einschlafen — die vorhandene Hinlegebewegung läuft dabei vollständig durch.

Eine einzige Verbindung zum Verhaltenstreiber war nötig: Ein Schlafender darf nicht
„B1 Umsehen" spielen. Das Ziehintervall der Idle-Einlagen wird im Schlaf deshalb gestreckt
und der Versatz einer noch laufenden Einlage ausgeblendet — rein mengenmäßig, ohne neue
Verzweigung. Die Zeitkaskade selbst bleibt unangetastet; der neue Schlaf gilt ausschließlich
im Liegen.

### Warum der Atem den Körper nicht anhebt

Weil die Bodenhöhe abgeleitet wird, hält die Bedingung den tiefsten Punkt am Boden — der
Atem kann den liegenden Körper also gar nicht heben. Sichtbar wird er über die **Stauchung**:
Die Brust weitet sich, der Rücken bleibt liegen. Für einen liegenden Körper ist genau das
richtig, und deshalb wird die Amplitude im Schlaf sogar größer, während die Frequenz sinkt.
