import AppKit

/// The formats the Compress menu and "Create archive…" offer, in the Windows app's order
/// (ctx_menu.rs): ZIP, 7z, the tar family, and RAR when RARLAB's `rar` is installed.
enum ArchiveKinds {
    static let all: [(key: String, title: String, format: ArchiveFormat)] = [
        ("zip", "ZIP", .zip), ("7z", "7z", .sevenZ), ("tarzst", "tar.zst", .tarZst), ("targz", "tar.gz", .tarGz), ("tar", "tar", .tar), ("rar", "RAR", .rar),
    ]

    /// The ones this Mac can make.
    static var offered: [(key: String, title: String, format: ArchiveFormat)] { all.filter { $0.format != .rar || canMakeRar() } }

    static func format(_ key: String) -> (key: String, title: String, format: ArchiveFormat)? { offered.first { $0.key == key } }
}

extension AppSettings {
    /// Settings › Files › "A click on Compress": "menu" opens the submenu only; otherwise the
    /// format a click makes at once (settings.rs `quick_compress`, ZIP by default).
    static var quickCompress: String {
        get { UserDefaults.standard.string(forKey: "quickCompress") ?? "zip" }
        set {
            UserDefaults.standard.set(newValue, forKey: "quickCompress")
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "quickCompress"])
        }
    }
}

/// "Create archive…": name, format, level, password, parts and 7z's dictionary, as the Windows
/// dialog asks them (ctx_menu.rs `create_archive_dialog`).
@MainActor
enum CreateArchiveDialog {
    struct Choice {
        let dest: URL
        let format: ArchiveFormat
        let level: UInt32
        let password: String?
        let volumeSize: UInt64?
        let dictSize: UInt64?
    }

    static let splitSizes: [(String, UInt64)] = [("25 MB", 25 << 20), ("100 MB", 100 << 20), ("700 MB", 700 << 20), ("1 GB", 1 << 30), ("4 GB", (4 << 30) - (1 << 20))]
    static let dictSizes: [(String, UInt64)] = [("1 MB", 1 << 20), ("16 MB", 16 << 20), ("64 MB", 64 << 20), ("256 MB", 256 << 20)]

    static func ask(for urls: [URL]) -> Choice? {
        guard let first = urls.first else { return nil }
        let dir = first.deletingLastPathComponent()
        let stem = urls.count == 1 ? first.deletingPathExtension().lastPathComponent : dir.lastPathComponent
        let kinds = ArchiveKinds.offered

        let alert = NSAlert()
        alert.messageText = String(localized: "Create archive")
        alert.informativeText = urls.count == 1
            ? String(localized: "1 item — the extension follows the format")
            : String(localized: "\(urls.count) items — the extension follows the format")
        alert.addButton(withTitle: String(localized: "Create"))
        alert.addButton(withTitle: String(localized: "Cancel"))

        let name = NSTextField(string: stem.isEmpty ? String(localized: "Archive") : stem)
        let format = popup(kinds.map(\.title), selected: max(0, kinds.firstIndex { $0.key == AppSettings.quickCompress } ?? 0))
        let level = popup([String(localized: "None"), String(localized: "Fast"), String(localized: "Normal"), String(localized: "Maximum")], selected: 2)
        let password = PasswordField()
        password.placeholderString = String(localized: "No password")
        let again = PasswordField()
        again.placeholderString = String(localized: "Repeat the password")
        let split = popup([String(localized: "None")] + splitSizes.map(\.0), selected: 0)
        let dict = popup([String(localized: "Automatic")] + dictSizes.map(\.0), selected: 0)

        let rows: [(String, NSView)] = [
            (String(localized: "Archive name"), name), (String(localized: "Format"), format), (String(localized: "Compression level"), level),
            (String(localized: "Password"), password), ("", again), (String(localized: "Split into parts"), split), (String(localized: "Dictionary size (7z)"), dict),
        ]
        let grid = NSGridView(views: rows.map { [label($0.0), $0.1] })
        grid.column(at: 0).xPlacement = .trailing
        grid.rowSpacing = 8
        grid.columnSpacing = 10
        for (_, v) in rows { v.widthAnchor.constraint(equalToConstant: 240).isActive = true }
        grid.frame = NSRect(origin: .zero, size: grid.fittingSize)
        alert.accessoryView = grid
        alert.window.initialFirstResponder = name

        while true {
            guard alert.runModal() == .alertFirstButtonReturn else { return nil }
            let kind = kinds[max(0, format.indexOfSelectedItem)]
            let n = name.stringValue.trimmingCharacters(in: .whitespaces)
            let pw = password.stringValue
            var problem: String?
            if n.isEmpty {
                problem = String(localized: "Enter a name")
            } else if n.contains("/") || n.contains(":") {
                problem = String(localized: "A name cannot contain / or :")
            } else if !pw.isEmpty && !archiveEncrypts(format: kind.format) {
                problem = String(localized: "tar formats cannot have a password; choose ZIP or 7z")
            } else if pw != again.stringValue {
                problem = String(localized: "The two passwords are not the same")
            } else if kind.format == .rar && split.indexOfSelectedItem > 0 {
                problem = String(localized: "RAR cannot be split into parts; choose ZIP or 7z")
            }
            let dest = dir.appendingPathComponent("\(n).\(archiveExtension(format: kind.format))")
            if problem == nil && FileManager.default.fileExists(atPath: dest.path) {
                problem = String(localized: "“\(dest.lastPathComponent)” is already there")
            }
            if let problem {
                alert.informativeText = problem
                continue
            }
            return Choice(
                dest: dest, format: kind.format, level: [0, 1, 5, 9][max(0, level.indexOfSelectedItem)],
                password: pw.isEmpty ? nil : pw,
                volumeSize: split.indexOfSelectedItem > 0 ? splitSizes[split.indexOfSelectedItem - 1].1 : nil,
                dictSize: dict.indexOfSelectedItem > 0 ? dictSizes[dict.indexOfSelectedItem - 1].1 : nil)
        }
    }

    private static func popup(_ titles: [String], selected: Int) -> NSPopUpButton {
        let p = NSPopUpButton(frame: .zero, pullsDown: false)
        p.addItems(withTitles: titles)
        p.selectItem(at: selected)
        return p
    }

    private static func label(_ s: String) -> NSTextField {
        let l = NSTextField(labelWithString: s)
        l.alignment = .right
        return l
    }
}
