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

## Installation

Unter [Releases](../../releases) die neueste `Laermampel-Setup-x.y.z.exe` herunterladen und ausführen.
Adminrechte sind nicht nötig, installiert wird nach `%LOCALAPPDATA%\Programs\Laermampel`.
Beim Installieren lässt sich „Mit Windows starten“ ankreuzen, das geht später auch in den Einstellungen.

Weil die Datei nicht signiert ist, warnt Windows beim ersten Mal: „Weitere Informationen“ → „Trotzdem ausführen“.

Deinstallieren über Windows-Einstellungen → Apps → Lärmampel.

## Bedienung

- **Einstellungen:** Klick auf das Ampel-Symbol im Infobereich (neben der Uhr) oder Rechtsklick → Einstellungen.
  Die Lärmampel nochmal starten öffnet ebenfalls die Einstellungen.
- **Beenden:** Rechtsklick auf das Symbol → Beenden.
- **Updates:** Das Programm sucht beim Start nach einer neuen Version. Unter Einstellungen → Version lässt sie sich installieren.
- Im Spiel „randloses Fenster“ verwenden, im exklusiven Vollbild wird die Anzeige verdeckt (dann hilft der Warnton).

Die Einstellungen liegen unter `%APPDATA%\LucyWolf\Laermampel\config\settings.json`.

## Bauen

```
cargo build --release
```

Der Installer wird mit [Inno Setup 6](https://jrsoftware.org/isinfo.php) gebaut:
`iscc /DAppVersion=0.2.0 installer\laermampel.iss`

Unter Linux wird zusätzlich `libasound2-dev` (bzw. `alsa-lib`) benötigt.

## Neue Version veröffentlichen

1. `version` in `Cargo.toml` erhöhen (z.B. `0.1.0` → `0.1.1`)
2. Committen und pushen
3. Tag setzen: `git tag v0.1.1 && git push origin v0.1.1`

GitHub Actions baut dann die `laermampel.exe` und legt das Release an.
Der Tag muss zur Version in `Cargo.toml` passen, sonst bricht der Build ab.
