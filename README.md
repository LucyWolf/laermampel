# Lärmampel

Kleines Programm, das dein Mikrofon mithört und dir als Ampel zeigt, wenn du zu laut sprichst.
Gedacht für geschlossene Kopfhörer ohne Monitoring, wo man die eigene Stimme kaum hört.

- Kleines, verschiebbares Fenster, immer im Vordergrund
- Grün, Gelb, Rot plus Pegelbalken, reagiert sofort und klingt langsam ab
- Einlernen: ein paar Sekunden normal reden, Gelb und Rot werden relativ dazu gesetzt
- Einstellbar: Schwellen, Anstieg, Abklingen, Haltezeit, Helligkeit (Grün extra), optionaler Warnton
- Liest das Mikrofon parallel zu Discord und Co., nimmt nichts weg

## Download

Unter [Releases](../../releases) die `laermampel.exe` herunterladen und starten, keine Installation nötig.
Updates: Das Programm sucht beim Start nach einer neuen Version. Ist eine da, erscheint ⬆ in der Ampel,
und unter ⚙ → Version lässt sie sich mit einem Klick installieren.

Die Einstellungen liegen unter `%APPDATA%\LucyWolf\Laermampel\config\settings.json`.

## Bedienung

- Fenster an der farbigen Fläche verschieben
- ⚙ öffnet die Einstellungen, ✕ beendet
- Im Spiel „randloses Fenster“ verwenden, im exklusiven Vollbild wird das Fenster verdeckt (dann hilft der Warnton)

## Bauen

```
cargo build --release
```

Unter Linux wird zusätzlich `libasound2-dev` (bzw. `alsa-lib`) benötigt.

## Neue Version veröffentlichen

1. `version` in `Cargo.toml` erhöhen (z.B. `0.1.0` → `0.1.1`)
2. Committen und pushen
3. Tag setzen: `git tag v0.1.1 && git push origin v0.1.1`

GitHub Actions baut dann die `laermampel.exe` und legt das Release an.
Der Tag muss zur Version in `Cargo.toml` passen, sonst bricht der Build ab.
