# Lärmampel

Kleines Programm, das dein Mikrofon mithört und dir als Ampel zeigt, wenn du zu laut sprichst.
Gedacht für geschlossene Kopfhörer ohne Monitoring, wo man die eigene Stimme kaum hört.

- Kleiner Punkt oder Leiste mit Pegel, immer im Vordergrund, Mausklicks gehen durch
- Frei platzierbar: Monitor 1, 2, 3 …, oben oder unten, jeweils links, Mitte oder rechts
- Grün, Gelb, Rot, reagiert sofort und klingt langsam ab
- Einlernen: ein paar Sekunden normal reden, Gelb und Rot werden relativ dazu gesetzt
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

- **Einstellungen:** Klick auf das Ampel-Symbol im Infobereich (neben der Uhr) oder Rechtsklick → Einstellungen.
  Die Lärmampel nochmal starten öffnet ebenfalls die Einstellungen.
- **Beenden:** Rechtsklick auf das Symbol → Beenden.
- **Updates:** Das Programm sucht beim Start nach einer neuen Version. Unter Einstellungen → Version auf „Update installieren“ klicken:
  der Installer läuft ohne Rückfragen durch und startet die Lärmampel danach neu.
- Im Spiel „randloses Fenster“ verwenden, im exklusiven Vollbild wird die Anzeige verdeckt (dann hilft der Warnton).

### Kanalzug: Comp., Gate, Gain, Mute

Bearbeitet nur dein Mikrofon, nicht das, was du hörst.

- **Comp.** (0–10): automatische Lautstärke, hebt leise Sprache an und regelt laute runter. 0 = aus.
- **Gate** (0–10): unter der Schwelle wird das Mikrofon leiser, z.B. Tastatur und Lüfter in Sprechpausen. 0 = aus, höher = höhere Schwelle. Die Lampe am Knopf zeigt, ob es gerade offen ist.
- **Gain-Fader:** Grundlautstärke −60 bis +12 dB.
- **Pegelanzeige:** links deine Stimme, rechts was rausgeht.
  - Ist **Ton** an, gibt es eine **gelbe** und eine **rote** Linie mit Pfeil am Rand für die Ampel-Schwellen, zum Ziehen.
  - **Limiter:** über der ganzen Anzeige, Linie runterziehen (−40 bis 0 dB). Auf 0 ist er aus und nur beim Überfahren zu sehen, Doppelklick schaltet ihn aus.
  - Gegriffen wird die Linie, die der Maus am nächsten ist; am Rand nur Gelb und Rot.
- **Ton:** Warnton, sobald deine Stimme über den roten Pfeil kommt.
- **Mute:** Mikrofon für andere stumm. Auch im Rechtsklick-Menü des Symbols; Punkt und Symbol bekommen dann einen roten Ring.
- Knöpfe und Fader: ziehen oder Mausrad, Doppelklick setzt zurück. Feineinstellungen unter „Details“.

Einrichten:

1. [VB-Cable](https://vb-audio.com/Cable/) installieren (kostenlos, braucht Adminrechte und einen Neustart). Die Lärmampel findet es von selbst.
2. In Discord, Spielen, OBS usw. als Mikrofon **„CABLE Output“** auswählen.

Die Lärmampel muss dafür laufen, sonst kommt bei „CABLE Output“ nichts an.
Die Ampel misst weiterhin vor dem Kanalzug, zeigt also, wie laut du wirklich sprichst.

Bei Problemen hilft die Log-Datei `%LOCALAPPDATA%\\LucyWolf\\Laermampel\\data\\laermampel.log`
(Einstellungen → Version → „Log-Datei zeigen“). Sie hält Start, Mikrofon, Fenster, Updates und Abstürze fest.

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
