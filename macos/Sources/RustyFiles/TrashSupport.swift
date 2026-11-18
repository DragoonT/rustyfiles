import AppKit

/// The Trash (the Windows app's Recycle Bin view): where each item came from, putting it
/// back, deleting for good, emptying.
///
/// macOS keeps the original places in a private Finder file, so rustyfiles remembers the ones
/// it moved there itself; those can be put back. Finder's own are left to Finder.
@MainActor
enum TrashBin {
    static let url = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".Trash")
    private static let key = "trashOrigins"

    static func contains(_ u: URL?) -> Bool {
        guard let u else { return false }
        let p = u.standardizedFileURL.path, t = url.standardizedFileURL.path
        return p == t || p.hasPrefix(t + "/")
    }

    static func isTrash(_ u: URL?) -> Bool { u?.standardizedFileURL.path == url.standardizedFileURL.path }

    /// Remembers that `trashed` (in the Trash) came from `origin`.
    nonisolated static func remember(_ pairs: [(from: URL, to: URL)]) {
        var map = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        for p in pairs { map[p.from.standardizedFileURL.path] = p.to.path }
        // forget what is no longer there
        map = map.filter { FileManager.default.fileExists(atPath: $0.key) }
        UserDefaults.standard.set(map, forKey: key)
    }

    static func origin(of trashed: URL) -> URL? {
        let map = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        return map[trashed.standardizedFileURL.path].map { URL(fileURLWithPath: $0) }
    }

    /// Puts the items back where they were (a name taken there gets " (2)"); returns
    /// (where it went, where it was in the Trash) for the undo.
    nonisolated static func putBack(_ pairs: [(trashed: URL, origin: URL)]) throws -> [(from: URL, to: URL)] {
        let fm = FileManager.default
        var done: [(from: URL, to: URL)] = []
        for p in pairs {
            var dest = p.origin
            try fm.createDirectory(at: dest.deletingLastPathComponent(), withIntermediateDirectories: true)
            if fm.fileExists(atPath: dest.path) {
                dest = FileOps.unique(dest.deletingPathExtension().lastPathComponent, ext: dest.pathExtension, in: dest.deletingLastPathComponent())
            }
            try fm.moveItem(at: p.trashed, to: dest)
            done.append((from: dest, to: p.trashed))
        }
        return done
    }

    /// Asks, then deletes for good (this can't be undone).
    static func confirmDelete(_ urls: [URL], everything: Bool, in window: NSWindow?) -> Bool {
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = everything
            ? String(localized: "Empty the Trash?")
            : urls.count == 1 ? String(localized: "Delete “\(urls[0].lastPathComponent)” immediately?")
            : String(localized: "Delete these \(urls.count) items immediately?")
        alert.informativeText = String(localized: "This can’t be undone.")
        alert.addButton(withTitle: everything ? String(localized: "Empty Trash") : String(localized: "Delete"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.buttons[0].hasDestructiveAction = true
        return alert.runModal() == .alertFirstButtonReturn
    }

    nonisolated static func delete(_ urls: [URL]) throws {
        let fm = FileManager.default
        for u in urls {
            // a locked item first loses its lock
            var v = URLResourceValues()
            v.isUserImmutable = false
            var w = u
            try? w.setResourceValues(v)
            try fm.removeItem(at: u)
        }
    }

    nonisolated static func everything() -> [URL] {
        (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil, options: [])) ?? []
    }

    static func openPrivacySettings() {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!)
    }
}
