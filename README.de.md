<p align="center">
  <b>Deutsch</b> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.md">English</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.tr.md">Türkçe</a>
</p>

<p align="center"><sub>Dies ist eine Übersetzung; maßgeblich ist die englische <a href="README.md">README</a>.</sub></p>

<img alt="rustyfiles: ein Dateimanager für Windows und macOS, der nie einfriert. In Rust geschrieben. Dargestellt als rustyfiles-Fenster mit einem Ordner mit 100,000 Dateien, geöffnet in 281 ms" src="docs/readme/cover.png" width="100%">

<p align="center">
  <b>Ein Dateimanager, der nie auf die Festplatte wartet.</b><br>
  Tabs, zwei Fensterbereiche, Archive, die sich wie Ordner öffnen, und eine Vorschau mit <kbd>Space</kbd>.<br>
  Von Grund auf in Rust geschrieben, für Windows und jetzt auch macOS.
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Neueste Version" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="Downloads" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11, x64 und ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <img alt="macOS 26 auf Apple Silicon und Intel, Beta" src="https://img.shields.io/badge/macOS_26-Apple_silicon_and_Intel_(beta)-3d3d44?labelColor=232326">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Jede Version wird auf VirusTotal geprüft: Die Berichte sind am Ende ihrer Versionshinweise verlinkt" src="https://img.shields.io/badge/VirusTotal-every_release_scanned-3d3d44?labelColor=232326"></a>
  <a href="LICENSE"><img alt="MIT-Lizenz" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Download für Windows" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg"><img alt="Download für macOS (Beta)" src="https://img.shields.io/badge/Download_for_macOS_(beta)-232326?style=for-the-badge&logo=apple&logoColor=white"></a>
  <br>
  <a href="https://rustyfiles.com"><img alt="Website" src="https://img.shields.io/badge/rustyfiles.com-232326?style=flat-square&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Dem Discord beitreten" src="https://img.shields.io/badge/Discord-5865F2?style=flat-square&logo=discord&logoColor=white"></a>
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="Spendier mir einen Kaffee" src="https://img.shields.io/badge/Buy_me_a_coffee-FFDD00?style=flat-square&logo=buymeacoffee&logoColor=black"></a>
</p>

<table>
  <tr>
    <td width="33%" valign="top">
      <b>Wartet nie auf die Festplatte</b><br>
      Auflisten, Symbole, Vorschaubilder, das Kontextmenü und jedes Kopieren laufen abseits des Threads des Fensters.
      Eine langsame Netzwerkfreigabe oder ein Ordner mit 100,000 Dateien kann es nicht einfrieren.
    </td>
    <td width="33%" valign="top">
      <b>Die Arbeit des Explorers, schneller erledigt</b><br>
      Tabs, zwei Fensterbereiche, Archive als Ordner, ein Namensindex für sofortige Suche, Massenumbenennung, Rückgängig,
      und eine eigene Kopier-Engine: 14-mal schneller bei kleinen Dateien.
    </td>
    <td width="33%" valign="top">
      <b>Kostenlos und deins</b><br>
      Kein Konto, keine Werbung, nichts wird irgendwohin gesendet. MIT-lizenziert, jede Zeile hier zum Nachlesen.
      Updates werden im Hintergrund still installiert.
    </td>
  </tr>
</table>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles zeigt einen Projects-Ordner in der Detailansicht, mit Tabs, einer Pfadleiste und angehefteten Ordnern in der Seitenleiste; das Fenster ist Glas über dem Hintergrundbild des Desktops" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Windows, mit dem experimentellen HyperGlass-Hintergrund (Einstellungen › Darstellung). Standard ist Mica, wie im Explorer.</sub>

## Wie schnell

<img alt="Ein Geschwindigkeitstest, dargestellt als rustyfiles-Liste: Das Öffnen eines Ordners mit 100,000 Dateien dauert 281 ms in rustyfiles und 2,318 ms im File Explorer (8-mal schneller); ein Ordner mit 2,000 Bildern 278 ms gegen 2,177 ms (8-mal); Kopieren von 10,000 kleinen Dateien 1.8 s gegen 25.6 s (14-mal); Entpacken eines ZIP mit 10,000 Dateien 3.6 s gegen 32.4 s (9-mal)" src="docs/readme/speed.png" width="100%">

Ein Ordner mit 100,000 Dateien öffnet sich in 281 ms (File Explorer: 2.3 s). Die Zahlen stammen aus dem Geschwindigkeitstest,
der in die App eingebaut ist, Einstellungen › Geschwindigkeitstest, damit Sie sie auf Ihrem eigenen PC prüfen können.

<details>
<summary><b>Jede Messung und ihre Methode</b></summary>

#### Öffnen eines Ordners

Zeit vom Start der App, bis die Liste des Ordners stillsteht, in einem neuen Fenster, wobei der Ordner bereits im Dateicache des Betriebssystems liegt. Median aus drei Läufen nach einem Aufwärmlauf.

| Ordner | rustyfiles | File Explorer | [Files](https://files.community) |
|---|--:|--:|--:|
| Leerer Ordner | 281 ms | 1,006 ms | 813 ms |
| 1,000 Dateien | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 Dateien | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 Dateien | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 Bilder (Vorschaubilder) | 278 ms | 2,177 ms | 1,075 ms |
| Speicher nach dem Auflisten von 10,000 Dateien | 39 MB | 69 MB | 204 MB |

Gemessen auf einem Rechner (Ryzen 7 9800X3D, NVMe-SSD, Windows 11) am 2026-11-16, mit rustyfiles
0.8.6 und Files 4.3. rustyfiles und der Explorer starteten jeweils einen neuen Prozess. Files lief bereits im Hintergrund,
daher sind seine Zeiten keine Kaltstarts; außerdem baut es seine Liste schrittweise auf, und die Zeit zählt, bis sich das Bild nicht mehr ändert.
Der Speicher ist der private Working Set, die Zahl, die der Task-Manager anzeigt. Mit eingeschaltetem „Im Hintergrund weiterlaufen“
startet ein neues rustyfiles-Fenster keinen Prozess und zeigte einen aufgelisteten Ordner in 93–168 ms (2026-11-06, eine frühere Version).

#### Ein Ordner mit 100,000 Dateien, der sich ständig ändert

Der Ordner ist geöffnet, eine Datei ist ausgewählt, und im Hintergrund werden 5 Sekunden lang etwa 1,000 Dateien pro Sekunde umbenannt:

| | |
|---|--:|
| Start bis zu einem aufgelisteten Ordner (100,001 Dateien) | 315 ms |
| Die Liste folgt den Änderungen | alle ~750 ms |
| Neu einlesen des Ordners (Hintergrund-Thread) | ~70 ms |
| Arbeit im UI-Thread pro Aktualisierung | 7–8 ms |
| Auswahl und Cursor | bleiben bei der ausgewählten Datei |

Bildzeiten, während derselbe Ordner 7.7 s lang nach unten und wieder nach oben gescrollt wird, wobei sich die Auswahl (und damit der Vorschaubereich) alle halbe Sekunde bewegt und im Hintergrund etwa 680 Dateien pro Sekunde umbenannt werden, auf einem 200-Hz-Display:

| | Median | p95 | p99 | Schlechtester |
|---|--:|--:|--:|--:|
| Bildkosten im UI-Thread | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| Zeit zwischen Bildern | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

2,182 Bilder in 7.7 s; eine Lücke, die länger ist als ein 60-Hz-Bild. Messen Sie es selbst mit
`RUSTYFILES_TRACE_LIVE=1` und `RUST_LOG=warn,rf_app=info`, und protokollieren Sie Bilder mit
`GPUI_FRAME_LOG=frames.tsv`.

#### Neuer Tab in einem offenen Fenster

| Ordner | Zeit bis zu einer stabilen Liste |
|---|--:|
| 1,000 Dateien | ~48 ms |
| 10,000 Dateien | ~46 ms |
| 100,000 Dateien | ~129 ms |

Jeder zusätzliche Tab kostet etwa 3 MB Speicher (1 Tab: ~32 MB, 25 Tabs: ~103 MB).

#### Suche nach Dateiname

rustyfiles führt einen Namensindex der lokalen Laufwerke und beantwortet Suchen daraus. Suche in allen Unterordnern eines Benutzerordners
mit 3.6 Millionen Dateien und Ordnern nach „rapor“, gemessen bis die Ergebnisse auf dem Bildschirm sich nicht mehr änderten:

| | Zeit |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| File Explorer | 1.8 s (seine schnellste von 4 Läufen; die anderen dauerten 11–32 s) |

Der Index speichert jeden unterschiedlichen Namen nur einmal: Ein Laufwerk mit 4.8 Millionen Einträgen, aber nur 490 Tausend
verschiedenen Namen, belegt 61 MB. Ein Ordner-Watcher hält ihn aktuell, solange rustyfiles läuft; zwischen den Läufen sagt
das Änderungsjournal von NTFS (gelesen ohne Administratorrechte), welche Ordner sich geändert haben, sodass ein Start nur diese
erneut auflistet: 0.2 s für C: hier, wo ein Durchlauf 30 s dauert. App-Daten, Caches,
`node_modules` und `.git` bleiben außen vor; eine Suche, die innerhalb davon gestartet wird, durchläuft die Festplatte:

| `*.dll` unter `C:\Windows` (342,492 Elemente, 116,851 Ordner, Cache warm) | Zeit |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

Sie sucht auf sechs Threads mit Hintergrundpriorität, damit der Rest des Rechners reaktionsfähig bleibt, und folgt keinen
Junctions, weshalb sie 454 Duplikate weniger findet.

#### Vorschau mit Space (DirectLook)

| Datei | Inhalt bereit, ab dem Tastendruck |
|---|--:|
| JPEG, 3840 × 2160 | 24–86 ms |
| Quellcode, syntaxhervorgehoben | 23–44 ms |
| 7z-Archiv (Inhalt aufgelistet) | 19–31 ms |
| MP3 (Tags, Liedtexte) | 27–32 ms |
| H.264-Video, 1080p | erstes Bild in 146–185 ms |

#### Archive

RAR lesen mit [rarchive](https://github.com/gelenmedya/rarchive), unserer eigenen Engine, im Vergleich zum offiziellen UnRAR
(jede CRC dekodieren und prüfen, Archive im Dateicache):

| Archiv | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB, 32 MB Wörterbuch, 195 Dateien | 2.79 s | **0.28 s** (10×) |
| 400 MB solid | 0.95 s | **0.21 s** (4.5×) |
| 400 MB, 520 Dateien | 1.03 s | **0.063 s** (16×) |

#### Kopieren und Entpacken

Unsere eigenen Engines gegen die von Windows (rustyfiles 0.8.6, 2026-11-16):

| | Windows | rustyfiles |
|---|--:|--:|
| Kopieren von 10,000 kleinen Dateien in 100 Ordnern (41 MB) | 25.6 s | **1.8 s** (14×) |
| Kopieren einer 1 GB großen Datei | 0.61 s | 0.44 s |
| Entpacken eines ZIP mit 10,000 Dateien | 32.4 s | **3.6 s** (9×) |

Das Kopieren verteilt kleine Dateien auf mehrere Ordner, weil Threads, die in denselben Ordner schreiben, aufeinander warten;
eine einzelne große Datei wird von der Festplatte begrenzt, nicht von der Engine. Namenskonflikte behandelt sie selbst (ersetzen,
überspringen oder beide behalten), sie kann pausieren und fortsetzen, jede Kopie Byte für Byte prüfen und ihre Geschwindigkeit begrenzen.
Das Testskript ist `crates/rf-win/examples/copybench.rs`.

Alle Zahlen: ein Rechner (Ryzen 7 9800X3D, NVMe-SSD, Windows 11), November 2026.

</details>

## Was es kann

- **Durchsuchen.** Tabs, die Sie in ein eigenes Fenster ziehen können, gespeicherte Sitzungen, zwei Fensterbereiche, Ansichten Details, Raster, Liste und Spalten, Gruppierung, ein Vorschaubereich, Farbmarkierungen, die Shelf, Netzwerkfreigaben und FTP.
- **Dateien.** Rückgängig und Wiederholen, Massenumbenennung, Ordnergrößen auf Anfrage, Speicherbelegung mit einer Treemap, das vollständige Kontextmenü des Explorers, Drag & Drop mit jeder anderen App.
- **Archive.** ZIP, 7z, RAR, tar und ISO öffnen sich wie Ordner; ZIP, 7z und tar können darin bearbeitet werden.
- **Suche.** Ein Namensindex Ihrer Laufwerke und die Schnellsuche (<kbd>Alt</kbd>+<kbd>Space</kbd>) für Apps, Dateien, Lesezeichen und die Einstellungen von Windows.
- **Windows.** Optional der Standard-Dateimanager, einschließlich <kbd>Win</kbd>+<kbd>E</kbd> und der Öffnen- und Speichern-Dialoge anderer Apps.
- **49 Sprachen**, die der Systemsprache folgen.

<details>
<summary><b>Jede Funktion im Detail</b></summary>

**Durchsuchen.** Tabs, die Sie zum Umordnen ziehen oder in ein neues Fenster herausziehen können. Speichern Sie die Tabs eines Fensters, geteilte Bereiche eingeschlossen, als benannte Sitzung und öffnen Sie sie später wieder. Ein neuer Tab öffnet die Startseite oder den Ordner, in dem Sie sich gerade befinden (Einstellungen › Allgemein). Zwei Fensterbereiche. Ansichten Details, Raster, Liste und Spalten (die Liste kann sich spaltenweise füllen, wie im Explorer), kompakte Einträge wie die kompakte Ansicht des Explorers, Gruppierung, ein Vorschaubereich, Suche, Farbmarkierungen, die Shelf, Bibliotheken, Netzwerkfreigaben und FTP. Eine Spalte für den Git-Status. Ordnergrößen auf Anfrage: Klicken Sie auf „Größe berechnen“; sie bleiben über Neustarts hinweg erhalten und können die Größe auf dem Datenträger anzeigen. „Speicherbelegung anzeigen“ bei einem Ordner oder Laufwerk listet auf, was den meisten Platz belegt, das Größte zuerst, neben einer Treemap, in die Sie hineinklicken können. Virtuelle Ordner wie Bibliotheken und Galerie öffnen sich in rustyfiles, und Geräte, die Windows unter Dieser PC auflistet (Telefone über USB oder Phone Link), erscheinen auf der Startseite unter Geräte. Cloud-Ordner, die Windows kennt (OneDrive, Proton Drive, Dropbox, MEGA und andere), stehen in der Seitenleiste.

**Dateien.** Rückgängig und Wiederholen für Umbenennen, Kopieren, Verschieben, Löschen in den Papierkorb und das Abflachen von Ordnern. Massenumbenennung. Duplizieren, Kopieren oder Verschieben in den anderen Bereich, und kopierten Text oder ein Bild als neue Datei einfügen. Das vollständige Kontextmenü des Explorers mit „Senden an“, Drag & Drop mit dem Explorer, dem Desktop, Browsern und Outlook sowie Ablageziele auf Tabs, angehefteten Ordnern, Ordnern und Programmen. Dateien an Ihre anderen Computer senden mit [Ferry](#ferry) und herunterladen mit [HyperDownload](#hyperdownload).

**Archive.** ZIP, 7z, RAR, die tar-Familie und ISO öffnen sich wie Ordner, passwortgeschützte ZIP-, 7z- und RAR-Dateien eingeschlossen. Hinzufügen, Löschen und Umbenennen innerhalb von ZIP, 7z und tar. Erstellen von ZIP, 7z und tar.zst auf jedem Kern. Das Lesen übernimmt [rarchive](https://github.com/gelenmedya/rarchive), unsere eigene reine Rust-Engine (auch [auf crates.io](https://crates.io/crates/rarchive)): Sie dekodiert ein 1.3 GB großes WinRAR-Archiv zehnmal schneller als UnRAR.

**Windows.** Optional der Standard-Dateimanager: Ordner, <kbd>Win</kbd>+<kbd>E</kbd>, Dieser PC und der Papierkorb öffnen sich in rustyfiles. Optional öffnen sich auch die Dialoge „Öffnen“, „Speichern“ und „Ordner auswählen“ anderer Apps in rustyfiles (Einstellungen › Erweitert), und eine Anheften-Schaltfläche dort merkt sich einen Startordner für jede App. Bilder und Videos, die in Fotos oder einer anderen Store-App geöffnet werden, kommen mit ihrem Ordner, sodass Zurück und Weiter funktionieren. Die Pfade `/select,` und `shell:` des Explorers funktionieren. Mica, dunkler Modus und die Systemakzentfarbe. Wenn ein Spiel oder ein Rendering die GPU auslastet, wechselt das Zeichnen ohne Flackern auf die CPU und wieder zurück.

**Befehle in der Adressleiste.** Wie im Explorer: Drücken Sie <kbd>Ctrl</kbd>+<kbd>L</kbd>, tippen Sie `cmd`, `powershell`, `wt` oder `code .` und drücken Sie <kbd>Enter</kbd>. Der Text wird zuerst als Ort verstanden (ein Pfad, ein Laufwerk, `%VAR%`, `shell:`); ein Ordner oder eine Datei mit diesem Namen im geöffneten Ordner hat immer Vorrang. Nur wenn es keinen solchen Ort gibt, wird das erste Wort wie im Ausführen-Feld als Programm gesucht und im geöffneten Ordner gestartet. Aus einer Vervollständigung wird nie etwas gestartet, und es werden keine Administratorrechte angefordert. Ein Schalter unter Einstellungen › Dateien und Ordner › Verhalten schaltet es aus.

**Ordnen.** Regeln für einen Ordner: Bedingungen zu Name, Erweiterung, Art, Größe und Alter sowie eine Aktion (verschieben, in einen aus einer Vorlage gebauten Ordner verschieben, umbenennen oder in den Papierkorb legen). Ein Lauf von Hand zeigt jeden Schritt in einer Vorschau und verschiebt vor „Anwenden“ nichts; ein Lauf lässt sich rückgängig machen. Nichts wird endgültig gelöscht und nichts überschrieben. Regeln, die von selbst laufen, sind standardmäßig aus.

**Suche.** Die Schnellsuche (<kbd>Alt</kbd>+<kbd>Space</kbd>) findet Apps, Dateien und Ordner, Browser-Lesezeichen und die Einstellungsseiten von Windows. Was Sie oft öffnen, steht zuerst, und was Sie bei denselben Buchstaben zuvor gewählt haben, steht als erste Zeile.

**Sprachen.** 49 davon, der Anzeigesprache des Systems folgend (oder wählen Sie eine unter Einstellungen › Allgemein). Die Oberfläche ist auf Türkisch und Englisch geschrieben; die anderen Sprachen begannen mit den Formulierungen der Files-Community und werden von dort maschinell übersetzt. Namen und Text in Rechts-nach-links-Schrift werden von rechts nach links gelesen (Dank an @ialhabbal); die Oberfläche selbst ist noch nicht gespiegelt. Korrekturen sind willkommen: Jede Sprache ist eine JSON-Datei in `crates/rf-app/locales/`.

</details>

<img alt="rustyfiles zeigt einen Ordner mit Windows-Hintergrundbildern in der Rasteransicht, eines davon ist ausgewählt und rechts im Vorschaubereich zu sehen, auf dem HyperGlass-Hintergrund" src="docs/readme/grid.png" width="100%">

## DirectLook

Wählen Sie eine Datei und drücken Sie <kbd>Space</kbd>: Video und Audio mit eigenem Player, Bilder, Text und Code, der Inhalt von Archiven und eine Detailkarte für alles andere. Mit den Pfeiltasten wechseln Sie durch den Ordner, während er geöffnet bleibt. Es läuft als eigenes Programm, daher belastet ein schweres Video nie den Dateimanager, und wenn Sie es einschalten, funktioniert es auch im Explorer und auf dem Desktop (Einstellungen › DirectLook).

## Ferry

Senden Sie Dateien und Ordner an rustyfiles auf Ihren anderen Computern im selben Netzwerk, ähnlich wie AirDrop: Rechtsklick › **Mit Ferry senden**. Aus, bis Sie es unter Einstellungen › Ferry einschalten.

<img alt="Einstellungen › Ferry mit zwei Geräten in der Nähe, einem gekoppelten Windows-PC mit seiner gemessenen Geschwindigkeit und einem MacBook mit einer geschätzten Geschwindigkeit; eine Windows-Benachrichtigung fragt, ob Dateien vom MacBook angenommen werden sollen, und die Statusleiste zeigt eine Übertragung bei 61 Prozent" src="docs/readme/ferry.png" width="100%">

<sub>Eine Zeichnung von Einstellungen › Ferry mit erfundenen Geräten, in den Worten und dem Layout der App.</sub>

<details>
<summary><b>So funktioniert Ferry</b></summary>

- Durchgehend verschlüsselt (QUIC mit TLS 1.3), direkt von Gerät zu Gerät: kein Server, kein Konto, nichts verlässt Ihr Netzwerk.
- Koppeln Sie Ihre Geräte wie Bluetooth: Beide zeigen denselben sechsstelligen Code. Dateien eines gekoppelten Geräts können ohne Nachfrage ankommen; alles andere wird vorher in einer Benachrichtigung abgefragt.
- Sie bestimmen, wer Ihren Computer sehen kann: niemand, Ihre gekoppelten Geräte (der Standard) oder alle für zehn Minuten. Solange Einstellungen › Ferry geöffnet ist, kann jeder ihn sehen, damit ein neues Gerät ihn finden kann.
- Eine unterbrochene Übertragung wird an der Stelle fortgesetzt, an der sie stehen blieb, und jede Datei wird geprüft, bevor sie ihren Namen erhält. Empfangene Dateien werden als heruntergeladen markiert, sodass SmartScreen nachfragt, bevor ein Programm unter ihnen läuft.
- Jedes Gerät zeigt eine geschätzte Geschwindigkeit, und **Geschwindigkeit testen** misst sie in beide Richtungen in wenigen Sekunden.
- Die macOS-App spricht dasselbe Protokoll, beschrieben in [docs/nearby-protocol.md](docs/nearby-protocol.md). Die Windows-Firewall fragt womöglich nach Ferry: Erlauben Sie es für private Netzwerke.

</details>

## HyperDownload

Der eigene Download-Manager von rustyfiles, ein Add-on: Einstellungen › Add-ons › HyperDownload › **Installieren**. Eine Datei über viele Verbindungen gleichzeitig, pausiert und fortgesetzt, dort wo sie aufgehört hat, Torrents und Magnet-Links, eine Downloads-Seite und Videos, die in DirectLook abgespielt werden, bevor sie fertig sind. Es ist ein eigenes Programm ([Quellcode](https://github.com/gelenmedya/hyperdownload), MIT), wird nur heruntergeladen, wenn Sie auf Installieren klicken, vorher gegen seine SHA-256-Prüfsumme geprüft und lässt sich jederzeit entfernen.

## Experimentell unter Windows: HyperGlass, das Dock und der Desktop

Standardmäßig aus und noch früh: gebaut und auf einem PC mit zwei Monitoren getestet. Erwarten Sie raue Kanten und melden Sie bitte, was Ihnen auffällt. Der Explorer läuft im Hintergrund weiter; nichts von Windows wird auf der Festplatte entfernt oder ersetzt.

<img alt="Ein Desktop mit dem rustyfiles-Dock unten (drei Inseln: Start und Suche, die angehefteten und laufenden Apps, der Infobereich und die Uhr), links das geöffnete Start-Panel des Docks mit angehefteten Apps und zuletzt verwendeten Dateien, in der Mitte HyperSearch mit den gefundenen Apps für adobe, und rechts das Kontrollzentrum mit WLAN, Bluetooth, Ethernet, dem gerade laufenden Lied, Lautstärke und Helligkeit" src="docs/readme/shell.png" width="100%">

<sub>Start, HyperSearch und das Kontrollzentrum, zusammengesetzt aus drei Screenshots desselben Desktops (sie öffnen sich nicht alle gleichzeitig).</sub>

<details>
<summary><b>Was jeder Teil tut</b></summary>

- **HyperGlass** (Einstellungen › Darstellung) ist ein fünfter Hintergrund: Der Hintergrund des Fensters ist Glas, das verzerrt und trübt, was dahinter liegt.
- **Das Dock** (Einstellungen › Dock, ein Schalter) setzt drei Glasinseln unten auf jeden Monitor anstelle der Taskleiste von Windows: ein eigenes Start-Panel und eine Suche, die laufenden und angehefteten Apps mit Fenstervorschauen, die Symbole des Infobereichs, ein Kontrollzentrum und ein Benachrichtigungspanel. Solange es eingeschaltet ist, öffnet ein Antippen der Windows-Taste das Start-Panel des Docks, und <kbd>Alt</kbd>+<kbd>Tab</kbd> öffnet dessen Umschalter. Schalten Sie das Dock aus, und die von Windows sind wieder da.
- **Der Desktop**, den rustyfiles zeichnet, kommt mit dem Dock: Ihr Hintergrundbild und Ihre Symbole, mit einer Option für abgerundete Ecken.
- **Helligkeit für jeden Monitor.** Windows hat einen Regler nur für den eigenen Bildschirm eines Laptops; das Kontrollzentrum des Docks hat einen für jeden Monitor. Externe Monitore werden über DDC/CI eingestellt, was fast jeder Desktop-Monitor der letzten fünfzehn Jahre über DisplayPort, HDMI und DVI spricht. Hat ein Monitor keinen Regler, prüfen Sie, ob DDC/CI in seinem eigenen Menü eingeschaltet ist; manche Docks, KVM-Umschalter und Adapter leiten es nicht durch.

</details>

## macOS (Beta)

Derselbe Dateimanager, nativ mit AppKit und Liquid Glass gezeichnet, über demselben Rust-Kern: Tabs, zwei Fensterbereiche, jede Ansicht, Quick Look mit <kbd>Space</kbd>, Archive als Ordner, Massenumbenennung, Speicherbelegung, die Shelf, Farbmarkierungen, Git-Status, Mit Server verbinden, Ferry und dieselben 49 Sprachen. Das Dock, der Desktop, HyperSearch und DirectLook bleiben vorerst unter Windows, und die Mac-Version von HyperDownload folgt in einer eigenen Version. Apple Silicon und Intel, mit macOS 26 (Tahoe). Sie aktualisiert sich selbst wie die Windows-App.

## Installation

**Windows.** [rustyfiles-win-Setup.exe herunterladen](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)
(etwa 50 MB) und öffnen. Wenn Windows meldet „Windows hat Ihren PC geschützt“, klicken Sie auf **Weitere Informationen** und dann auf
**Trotzdem ausführen**: Das Installationsprogramm ist noch nicht code-signiert, und jede Version wird auf
[VirusTotal](https://www.virustotal.com) geprüft (die Berichte sind am Ende ihrer Versionshinweise verlinkt).
rustyfiles öffnet sich von selbst und steht in Ihrem Startmenü. Kein Administratorkennwort; Updates werden im Hintergrund still installiert.

**macOS (Beta).** [rustyfiles-mac.dmg herunterladen](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg),
öffnen und rustyfiles in Programme ziehen. Es ist von Apple signiert und notariell beglaubigt, daher öffnet es sich wie jede andere Mac-App.

<details>
<summary><b>Scoop, portabel, ARM64, Standard-Dateimanager, Deinstallation</b></summary>

**Mit [Scoop](https://scoop.sh):**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**Portabel oder ARM64:** Ein portables ZIP und ein ARM64-Installationsprogramm (`rustyfiles-win-arm64-Setup.exe`) liegen auf der Seite [Releases](https://github.com/gelenmedya/rustyfiles/releases). Windows 11 ist erforderlich (Windows 10 ist nicht getestet). Linux ist geplant.

**Ordner und <kbd>Win</kbd>+<kbd>E</kbd> in rustyfiles:** Schalten Sie unter Einstellungen › Erweitert › Als Standard-Dateimanager festlegen ein. Sie können es jederzeit wieder ausschalten.

**Deinstallation** wie bei jeder App unter Windows-Einstellungen › Apps. Alles, was rustyfiles geändert hat, geht zurück an den Datei-Explorer.

**Öffnen- und Speichern-Dialoge in anderen Apps schlagen fehl oder stürzen ab**, nachdem die Dateien von rustyfiles ohne Deinstallation entfernt wurden (durch einen Virenscanner, ein kaputtes Update)? Ein erneuter Start von rustyfiles behebt es. Startet es nicht mehr, gibt dieser PowerShell-Befehl jeder App den Windows-eigenen Dialog zurück:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

</details>

<details>
<summary><b>Aus dem Quellcode bauen</b></summary>

Sie brauchen [Rust](https://rustup.rs) 1.95 oder neuer und die Visual Studio Build Tools mit der Arbeitslast „Desktopentwicklung mit C++“ (eine Abhängigkeit, zstd, kompiliert C-Code).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # baut, dann Installation für Ihren Benutzer
.\scripts\install.ps1 -Default   # macht es außerdem zum Standard-Dateimanager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` startet es ohne Installation. Die macOS-App liegt in `macos/` und
`crates/rf-mac`, außerhalb des Windows-Workspace.

</details>

**Fragen oder Ideen?** Schauen Sie auf [Discord](https://discord.gg/K2banZ36zJ) vorbei oder besuchen Sie
[rustyfiles.com](https://rustyfiles.com).

## Unter der Haube

<details>
<summary><b>Tastatur</b></summary>

| Tasten | Aktion |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | Neuer Tab, Tab schließen, geschlossenen Tab wieder öffnen |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> oder <kbd>Alt</kbd>+<kbd>D</kbd> | Adressleiste |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Suche (<kbd>Enter</kbd> durchsucht auch Unterordner) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> oder <kbd>F1</kbd> | Befehlspalette |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | Einen besuchten Ordner finden, einen Tab finden |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | Details, Raster, Spalten, Karten, Liste |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>, <kbd>F6</kbd> | Zwei Fensterbereiche, Bereich wechseln |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | Kopieren, in den anderen Bereich verschieben |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | Duplizieren |
| <kbd>Alt</kbd>+<kbd>P</kbd>, <kbd>Ctrl</kbd>+<kbd>B</kbd> | Vorschaubereich, Seitenleiste |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | Umbenennen (Massenumbenennung bei mehreren ausgewählten Elementen) |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Rückgängig, Wiederholen |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | Versteckte Elemente anzeigen |

Jede Tastenkombination lässt sich unter Einstellungen › Aktionen ändern: Klicken Sie auf einen Befehl und drücken Sie dann die neuen Tasten.

</details>

<details>
<summary><b>Projektaufbau</b></summary>

| Crate | Inhalt |
|---|---|
| `crates/rf-app` | Die Windows-Oberfläche und ihre zwei Programme: `rustyfiles` und `directlook` (`src/look/`) |
| `crates/rf-win` | Windows- und Shell-Integration, die Engine für Dateioperationen |
| `crates/rf-core` | Das Modell, das beide Plattformen teilen: Orte, Verlauf, Einträge, Formate |
| `crates/rf-archive` | Archivoperationen: durchsuchen, entpacken, erstellen, bearbeiten |
| `crates/rf-nearby` | Ferry, mit der macOS-App geteilt |
| `crates/rf-player` | Der Video- und Audioplayer von DirectLook |
| `crates/rustdecoder` | Unsere eigene Video-Engine, in Arbeit: MP4/MOV und MKV/WebM; H.264, HEVC, VP9 und AV1 werden auf der Grafikkarte über D3D11 Video dekodiert, in den eigenen Tests bitgenau mit ffmpeg / dav1d; ProRes und DNxHR auf der CPU; Audio und A/V-Synchronisation ([Entwurf](docs/design/rustdecoder.md)) |
| `macos/`, `crates/rf-mac` | Die macOS-App (AppKit) und ihre Brücke zum Rust-Kern |

Die Windows-Oberfläche wird mit [GPUI](https://gpui.rs) gebaut, dem Framework hinter dem Editor Zed. Ihre Windows-Schicht ist ein Fork, der den Wechsel zwischen GPU und CPU ergänzt:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

</details>

<details>
<summary><b>Umgebungsvariablen</b></summary>

| Variable | Wirkung |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` oder `auto` | Erzwingt, wo das Fenster gezeichnet wird |
| `RUSTYFILES_CONFIG_DIR` = Ordner | Speichert Einstellungen in einem anderen Ordner (portable Nutzung) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Erlaubt mehrere Kopien gleichzeitig (Tests) |
| `RUSTYFILES_FRAME_LOG=1` | Protokolliert Bildzeiten |
| `RUSTYFILES_LOOK_TRACE=1` | Schreibt die Zeiten von DirectLook nach stderr |
| `RUSTYFILES_MUTE=1` | DirectLook spielt ohne Ton ab (Tests) |

Einstellungen werden in `%APPDATA%\rustyfiles\settings.toml` gespeichert, die von DirectLook daneben in `directlook.toml`.

</details>

<details>
<summary><b>So entsteht es</b></summary>

rustyfiles wird mit KI-Programmierwerkzeugen geschrieben, und das ist kein Geheimnis. Eine Person entscheidet, was gebaut wird und wie es sich verhalten soll, testet jede Änderung auf einem echten PC (zwei Monitore, Alltagsarbeit) und verwirft, was nicht standhält. Die KI schreibt einen großen Teil des Codes; sie entscheidet nicht, was ausgeliefert wird.

Was das ehrlich hält:

- **Tests.** Mehr als 4,000 automatisierte Tests laufen, bevor jede Änderung hochgeladen wird.
- **Messungen.** Die Geschwindigkeitszahlen stammen von einem Test, der in die App eingebaut ist (Einstellungen › Geschwindigkeitstest), sodass jeder sie auf dem eigenen Rechner prüfen kann.
- **Rückmeldungen.** Fehler kommen von den Menschen, die es benutzen. Jedes Issue bekommt eine Antwort, die Korrektur entsteht öffentlich, und die meldende Person wird gebeten, sie zu testen, bevor es geschlossen wird.
- **Der Code.** Alles liegt hier unter der MIT-Lizenz, zum Lesen und Prüfen.

Warum Rust: Ein Dateimanager muss mit 100,000 Dateien in einem Ordner schnell bleiben und darf nie die Daten von jemandem verlieren. Rust bietet Geschwindigkeit ohne Garbage Collector und schließt ganze Klassen von Abstürzen aus, bevor sie ausgeliefert werden. rustyfiles ist eine neue, von Grund auf geschriebene App und keine Neufassung des Explorers; sie läuft neben dem Explorer und verändert nichts an Windows auf der Festplatte.

</details>

## Status

rustyfiles ist jung, und sein Autor benutzt es jeden Tag anstelle des Explorers. Die macOS-App ist eine erste Beta auf demselben Kern, und Linux kommt danach. Fehlerberichte und Pull Requests sind willkommen.

## Gelistet bei

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/), als Alternative zum File Explorer
- [Awesome GPUI](https://github.com/zed-industries/awesome-gpui#productivity), im Abschnitt Productivity
- [Awesome Rust](https://github.com/rust-unofficial/awesome-rust#file-manager), im Abschnitt File manager
- [Scoop](https://scoop.sh), aus dem eigenen Bucket (siehe [Installation](#installation))

## Danksagung

rustyfiles baut auf der Arbeit anderer Menschen auf und hat unterwegs viele Ideen übernommen.

[Files](https://github.com/files-community/Files) zeigte, wie ein moderner Dateimanager für Windows aussehen kann. Sein Funktionsumfang war unsere Checkliste, seine Einstellungen und sein Layout haben unsere geprägt, und sein Ansatz, den Explorer als Standard-Dateimanager zu ersetzen, hat unseren geleitet. Danke an die Files-Community.

[GPUI](https://gpui.rs), das UI-Framework des Editors [Zed](https://github.com/zed-industries/zed), zeichnet unter Windows jeden Pixel von rustyfiles auf der GPU und macht ein so schnelles Fenster möglich. Danke an das Zed-Team für den Bau und das Teilen, und an [Longbridge](https://github.com/longbridge/gpui-kit) für gpui-kit, die Komponenten, auf denen wir aufbauen.

DirectLook folgt der Idee von macOS Quick Look und von [QuickLook](https://github.com/QL-Win/QuickLook) von Paddy Xu, das sie als Erstes unter Windows brachte. rarchive hat das RAR-Format aus dem UnRAR-Quellcode von Alexander Roshal gelernt.

Gebaut mit [windows-rs](https://github.com/microsoft/windows-rs), [Velopack](https://velopack.io) (Installation und Updates), [Lucide](https://lucide.dev)-Symbolen, [zip](https://github.com/zip-rs/zip2),
[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust), [tar](https://github.com/alexcrichton/tar-rs),
[notify](https://github.com/notify-rs/notify) und vielen weiteren Crates. Die Bilder verwenden
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage) von Atelier Triay,
[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) und
[Doto](https://fonts.google.com/specimen/Doto).

## Lizenz

MIT, siehe [LICENSE](LICENSE). Der GPUI-Fork behält Zeds Apache-2.0-Lizenz in seinem eigenen Repository.
