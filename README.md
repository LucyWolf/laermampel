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
- **Comp.** (0–10): automatische Lautstärke, hebt leise Sprache an und regelt laute runter. 0 = aus.
- **Gate** (0–10): unter der Schwelle wird das Mikrofon leiser, z.B. Tastatur und Lüfter in Sprechpausen. 0 = aus, höher = höhere Schwelle. Die Lampe am Knopf zeigt, ob es gerade offen ist.
- **Gain-Fader:** Grundlautstärke −60 bis +12 dB.
- **Pegelanzeige:** ein Balken mit deiner Stimme (ein Mikrofon ist mono).
  - Ist **Ton** an, gibt es eine **gelbe** und eine **rote** Linie mit Pfeil am Rand für die Ampel-Schwellen, zum Ziehen.
  - **Limiter:** über der ganzen Anzeige, Linie runterziehen (−40 bis 0 dB). Auf 0 ist er aus und nur beim Überfahren zu sehen, Doppelklick schaltet ihn aus.
  - Gegriffen wird die Linie, die der Maus am nächsten ist; am Rand nur Gelb und Rot.
- **Ton:** Warnton, sobald deine Stimme über den roten Pfeil kommt.
- **Mute:** Mikrofon für andere stumm. Die Pegelanzeige wird grau, schlägt aber weiter aus. Auch im Rechtsklick-Menü des Symbols; Punkt und Symbol bekommen dann einen roten Ring.
- Comp., Gate, Fader, Limiter und Mute brauchen den Filter („Ohne VB-Cable“) oder VB-Cable. Ohne beides zeigt der Kanalzug den Knopf „Filter einrichten“.
- Knöpfe und Fader: ziehen oder Mausrad, Doppelklick setzt zurück. Feineinstellungen und Ausgabe unter dem Zahnrad oben rechts.

Einrichten:

1. [VB-Cable](https://vb-audio.com/Cable/) installieren (kostenlos, braucht Adminrechte und einen Neustart). Die Lärmampel findet es von selbst.
2. In Discord, Spielen, OBS usw. als Mikrofon **„CABLE Output“** auswählen.

Die Lärmampel muss dafür laufen, sonst kommt bei „CABLE Output“ nichts an.

### Mikrofon-Filter (ohne VB-Cable)

Im Kanalzug über „Filter einrichten“ (oder ⚙ → „Mikrofon-Filter“) lässt sich ein Audio-Filter (APO) direkt beim Mikrofon eintragen,
so wie Equalizer APO es macht. Dann wirken Fader und Mute in allen Programmen mit dem normalen Mikrofon.

- Braucht einmal Adminrechte; der Windows-Audiodienst startet dabei neu (Ton ein paar Sekunden weg).
- Die Originalwerte des Mikrofons werden gesichert, „Filter entfernen“ und die Deinstallation stellen sie wieder her.
- Gate, Comp., Fader, Limiter und Mute laufen dann im Filter, derselbe Code wie beim Weg über VB-Cable.
- Nicht jeder Treiber lädt solche Filter; dann zeigt die Lärmampel „Eingetragen, aber Windows nutzt den Filter nicht“.
- Notfalls von Hand austragen: `laermampel.exe --apo uninstall-all` als Administrator ausführen.
Die Ampel misst weiterhin vor dem Kanalzug, zeigt also, wie laut du wirklich sprichst.

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
