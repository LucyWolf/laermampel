# Lärmampel

Kleines Programm, das dein Mikrofon mithört und dir als Ampel zeigt, wenn du zu laut sprichst.
Gedacht für geschlossene Kopfhörer ohne Monitoring, wo man die eigene Stimme kaum hört.

- Kleiner Punkt oder Leiste mit Pegel, immer im Vordergrund, Mausklicks gehen durch
- Frei platzierbar: Monitor 1, 2, 3 …, oben oder unten, jeweils links, Mitte oder rechts
- Grün, Gelb, Rot, reagiert sofort und klingt langsam ab
- Einstellbar: Schwellen, Anstieg, Abklingen, Haltezeit, Größe, Abstand zum Rand, Helligkeit (Grün extra), optionaler Warnton
- Symbol im Infobereich neben der Uhr, zeigt ebenfalls die Ampelfarbe
- Liest das Mikrofon parallel zu Discord und Co., nimmt nichts weg
- Updates mit einem Klick
- Kanalzug fürs Mikrofon wie im Mischpult (über VB-Cable): Comp., Gate, Gain-Fader, Pegelanzeige, Mute

## Installation

Unter [Releases](../../releases) die neueste `Laermampel-Setup-x.y.z.exe` herunterladen und ausführen.
Adminrechte sind nicht nötig, installiert wird nach `%LOCALAPPDATA%\Programs\Laermampel`.
Beim Installieren lassen sich eine Desktop-Verknüpfung und „Mit Windows starten“ ankreuzen (Autostart geht später auch in den Einstellungen).

Weil die Datei nicht signiert ist, warnt Windows beim ersten Mal: „Weitere Informationen“ → „Trotzdem ausführen“.

Deinstallieren über Windows-Einstellungen → Apps → Lärmampel.

## Bedienung

- **Fenster öffnen:** Klick auf das Ampel-Symbol im Infobereich (neben der Uhr), Rechtsklick → Einstellungen,
  oder die Lärmampel nochmal starten.
- **Fenster:** Titelzeile zum Verschieben, rechts ⚙ (Einstellungen), – (minimieren), X (schließen; läuft im Infobereich weiter).
- **Anzeige:** Knopf im Kanalzug → Punkt oder Leiste, Monitor, Position, Größe, Helligkeit.
- **Beenden:** Rechtsklick auf das Symbol → Beenden, oder ⚙ → „Lärmampel beenden“.
- **Updates:** Beim Start wird nach einer neuen Version gesucht (grüner Punkt am Zahnrad). ⚙ → „Update installieren“:
  der Installer läuft ohne Rückfragen durch und startet die Lärmampel danach neu.
- Im Spiel „randloses Fenster“ verwenden, im exklusiven Vollbild wird die Anzeige verdeckt (dann hilft der Warnton).

### Kanalzug: Comp., Gate, Gain, Mute

Bearbeitet nur dein Mikrofon, nicht das, was du hörst.

- **Mikrofon wählen:** auf den Gerätenamen unter „MIKROFON“ klicken. Ist keins ausgewählt oder das gewählte abgesteckt, blinkt dort ein roter Hinweis.
- **Empfindlichkeit:** Regler unter dem Gerätenamen, in Prozent. Das ist der Mikrofonpegel von Windows; das Gate senkt von diesem Wert aus ab.
- **Ausgang wählen:** unter der Pegelanzeige auf „Ausgang … ▾“ klicken: Aus, automatisch VB-Cable, oder ein virtuelles Gerät (VB-Cable, Voicemeeter). Kopfhörer und Lautsprecher stehen wegen Rückkopplung nicht zur Wahl.
- **Comp.** (0–10): automatische Lautstärke, hebt leise Sprache an und regelt laute runter. 0 = aus.
- **Gate** (−100 bis 0 dB): gleitend wie bei Voicemeeter. Über der Schwelle bleibt alles unverändert, darunter wird pro dB darunter um 3 dB zusätzlich abgesenkt (höchstens um die eingestellte Absenkung). Je höher das Gate, desto niedriger der Pegel; die Anzeige zeigt den Pegel nach dem Gate. Ganz links (−100) ist aus.
  Ohne VB-Cable zieht die Lärmampel dafür den Mikrofon-Regler von Windows gleitend herunter (gilt für alle Programme). Die Lärmampel senkt dabei nur so weit ab, dass sie dich noch hört, und regelt das selbst nach; hängt das Gate trotzdem, öffnet sie nach 4 s kurz den Regler und misst neu. Beim Beenden wird der Regler zurückgestellt, nach einem Absturz beim nächsten Start.
- **Gain-Fader:** Grundlautstärke −60 bis +12 dB. Ohne Ausgang stellt er den Mikrofonpegel von Windows (gilt für alle Programme); das Gate senkt von diesem Wert aus ab.
- **Pegelanzeige:** ein Balken (ein Mikrofon ist mono) mit dB-Skala; ein kleines Dreieck an der Skala zeigt die Gate-Schwelle (grün offen, grau zu). Zeigt, was nach Gate & Co. übrig bleibt; bei Mute grau deine Stimme.
  - Ist **Ton** an, gibt es eine **gelbe** und eine **rote** Linie mit Pfeil am Rand für die Ampel-Schwellen, zum Ziehen.
  - **Limiter:** über der ganzen Anzeige, Linie runterziehen (−40 bis 0 dB, Anzeige −100 bis 0 dB). Auf 0 ist er aus und nur beim Überfahren zu sehen, Doppelklick schaltet ihn aus.
  - Gegriffen wird die Linie, die der Maus am nächsten ist; am Rand nur Gelb und Rot.
- **Rausch:** öffnet das Fenster „Rauschen“ mit Filter-Schalter (RNNoise), Frequenz-Diagramm und Rauschprofil. Grüner Knopf heißt, der Filter läuft; er wirkt über den Ausgang und braucht ein Mikrofon mit 48 kHz.
- **Echo:** rechnet heraus, was aus deinen Kopfhörern wieder ins Mikrofon geht (wichtig, wenn das Headset
  neben dir liegt statt auf dem Kopf). Dafür wird das **Standard-Wiedergabegerät** von Windows mitgehört;
  überfahren zeigt, welches Gerät das ist, um wie viel dB das Echo gerade leiser wird und welchen Versatz der
  Filter gefunden hat. Steht dort „Noch kein Echo gefunden“, läuft entweder kein Ton oder das falsche Gerät ist
  das Standardgerät. Kostet 10 ms und wirkt – wie Comp. und Limiter – für andere Programme nur über den Ausgang.
  Was der Filter stehen lässt (Verzerrungen kleiner Kopfhörer-Treiber, Nachhall vom Tisch), wird danach noch um
  bis zu 8 dB abgesenkt – aber nur, solange es wirklich nur Restecho ist. Sobald du redest, geht die Absenkung
  sofort wieder auf.
- **Ton:** Warnton, sobald deine Stimme über den roten Pfeil kommt.
- **Mute:** Mikrofon für andere stumm, die Pegelanzeige wird grau. Auch im Rechtsklick-Menü des Symbols; Punkt und Symbol bekommen dann einen roten Ring.
- Gate und Mute wirken auch ohne VB-Cable (über den Mikrofon-Regler von Windows; bei Mute bekommt dann auch die Lärmampel nur Stille). Comp., Limiter, Rauschfilter und Echo wirken in anderen Programmen nur mit VB-Cable.
- Der **Fader** verstellt ohne Ausgang den Mikrofon-Regler von Windows. Der hat einen festen Bereich: steht er
  schon am Anschlag, kommt ein Aufdrehen nicht mehr an. Das Überfahren des Faders sagt, wie viele dB gerade
  verloren gehen – weiter hoch geht es dann nur mit VB-Cable als Ausgang.
- Knöpfe und Fader: ziehen oder Mausrad, Doppelklick setzt zurück. Feineinstellungen und Ausgabe unter dem Zahnrad oben rechts.

Einrichten:

1. [VB-Cable](https://vb-audio.com/Cable/) installieren (kostenlos, braucht Adminrechte und einen Neustart). Die Lärmampel findet es von selbst.
2. In Discord, Spielen, OBS usw. als Mikrofon **„CABLE Output“** auswählen.

Die Lärmampel muss dafür laufen, sonst kommt bei „CABLE Output“ nichts an.

Unter dem Kanalzug steht neben dem Pegel die **Verzögerung in ms** (nur mit Ausgang). Beim Überfahren
steht, woraus sie besteht: Mikrofon-Block + Puffer + Ausgabe-Block + Rauschfilter (10 ms). Den Puffer
stellst du unter ⚙ → Feineinstellungen ein (5–60 ms): kleiner heißt weniger Verzögerung, aber mehr
Risiko für Aussetzer.

Die Ampel misst weiterhin vor dem Kanalzug, zeigt also, wie laut du wirklich sprichst.

Im Fenster „Rauschen“ (Knopf im Kanalzug) zeigt ein Frequenz-Diagramm (wie in ReaFir), was das Mikrofon gerade hört.
„Rauschprofil messen“ nimmt zwei Sekunden Stille auf und legt sie als graue Linie darüber (bleibt gespeichert);
die gelbe Linie ist die Gate-Schwelle.

Bei Problemen hilft die Log-Datei `%LOCALAPPDATA%\\LucyWolf\\Laermampel\\data\\laermampel.log`
(Zahnrad oben rechts → „Log-Datei zeigen“). Sie hält Start, Mikrofon, Fenster, Updates und Abstürze fest.

Die Einstellungen liegen unter `%APPDATA%\LucyWolf\Laermampel\config\settings.json`.

## Bauen

```
cargo build --release
```

Das Icon erzeugt `scripts/make_icon.py` (schreibt nach `assets/`).

Der Installer wird mit [Inno Setup 6](https://jrsoftware.org/isinfo.php) gebaut:
`iscc /DAppVersion=0.2.0 installer\laermampel.iss`

Unter Linux wird zusätzlich `libasound2-dev` (bzw. `alsa-lib`) benötigt.

## Neue Version veröffentlichen

```
scripts/release.sh
```

Das Skript zählt die letzte Stelle der Version hoch, committet, setzt den Tag und pusht.
GitHub Actions baut dann Installer und `laermampel.exe` und legt das Release an.

Versionsschema: nur die letzte Stelle wird erhöht (`0.3.1`, `0.3.2` …).
Nach `.99` geht es mit der mittleren Stelle weiter: `0.3.99` → `0.4.0`.
Der Build prüft, dass der Tag zur Version in `Cargo.toml` passt und die letzte Stelle nicht über 99 liegt.

Frühere Versionen konnten einen Audio-Filter (APO) beim Mikrofon eintragen. Ist davon noch etwas übrig,
bietet ⚙ „Entfernen“ an; die Deinstallation trägt ihn ebenfalls aus. Von Hand: `laermampel.exe --apo uninstall-all` als Administrator.
