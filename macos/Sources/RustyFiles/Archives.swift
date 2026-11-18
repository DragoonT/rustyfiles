import AppKit

/// A place inside an archive: the archive file and the folder in it ("" for the top). Such a
/// place is written as a path that goes on past the archive (`~/a.zip/photos/2024`), so the
/// breadcrumb, Up and the history work on it as on any folder.
struct ArchivePlace: Sendable {
    let archive: URL
    let inner: String

    /// The archive `url` lies in, if it does: the nearest file on the way up that rustyfiles
    /// opens as a folder. Touches the disk (a few `stat`s); cheap enough off the main thread.
    static func locate(_ url: URL) -> ArchivePlace? {
        guard url.isFileURL else { return nil }
        var parts: [String] = []
        var u = url.standardizedFileURL
        let fm = FileManager.default
        while u.path != "/" && !u.path.isEmpty {
            var isDir: ObjCBool = false
            if fm.fileExists(atPath: u.path, isDirectory: &isDir) {
                if !isDir.boolValue, isArchive(path: u.path) {
                    return ArchivePlace(archive: u, inner: parts.reversed().joined(separator: "/"))
                }
                // a real folder: nothing above it is an archive this path goes into
                if isDir.boolValue { return nil }
            }
            parts.append(u.lastPathComponent)
            u = u.deletingLastPathComponent()
        }
        return nil
    }

    func child(_ name: String) -> String { inner.isEmpty ? name : inner + "/" + name }
}

/// What the window keeps about an archive it shows: where in it, and the listed archive.
struct ArchiveView {
    let place: ArchivePlace
    let archive: Archive
}

/// Listed archives, kept while their file is unchanged, and the passwords given for them.
@MainActor
enum ArchiveStore {
    private struct Key: Hashable {
        let path: String
        let modified: Date?
    }

    private static var archives: [Key: Archive] = [:]
    private static var passwords: [String: String] = [:]

    private static func key(_ url: URL) -> Key {
        Key(path: url.path, modified: try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate)
    }

    static func cached(_ url: URL) -> Archive? { archives[key(url)] }
    static func store(_ archive: Archive, for url: URL) { archives[key(url)] = archive }
    static func password(for url: URL) -> String? { passwords[url.path] }
    static func setPassword(_ pw: String, for url: URL) { passwords[url.path] = pw }

    /// Asks for an archive's password (nil: cancelled).
    static func askPassword(for url: URL, in window: NSWindow?, wrong: Bool) -> String? {
        let alert = NSAlert()
        alert.messageText = wrong
            ? String(localized: "The password for “\(url.lastPathComponent)” is not right.")
            : String(localized: "“\(url.lastPathComponent)” is protected by a password.")
        alert.informativeText = String(localized: "Enter the password to open it.")
        let field = NSSecureTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "Open"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn, !field.stringValue.isEmpty else { return nil }
        return field.stringValue
    }

    /// A new, empty folder for files written out of archives (opened, previewed, copied).
    nonisolated static func scratchFolder() -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("rustyfiles-archives", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }
}

/// Disk images (.dmg): macOS mounts them (hdiutil); the mounted disk is then a folder like any.
enum DiskImages {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static func isDiskImage(_ url: URL) -> Bool {
        ["dmg", "sparseimage", "sparsebundle"].contains(url.pathExtension.lowercased())
    }

    /// Mounts `url` and returns where it is mounted. Blocks: call it off the main thread.
    /// `diskutil image attach` is the current tool; `hdiutil attach` (deprecated in macOS 27)
    /// stands in where it is missing.
    static func attach(_ url: URL) throws -> URL {
        do {
            return try run("/usr/sbin/diskutil", ["image", "attach", "--plist", url.path], url)
        } catch {
            return try run("/usr/bin/hdiutil", ["attach", "-plist", "-noautoopen", url.path], url)
        }
    }

    private static func run(_ tool: String, _ args: [String], _ url: URL) throws -> URL {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: tool)
        p.arguments = args
        let out = Pipe(), err = Pipe()
        p.standardOutput = out
        p.standardError = err
        // a licence agreement would be shown on the terminal: answer it by agreeing, as Finder does
        let input = Pipe()
        p.standardInput = input
        try p.run()
        input.fileHandleForWriting.write("Y\n".data(using: .utf8)!)
        try? input.fileHandleForWriting.close()
        let data = out.fileHandleForReading.readDataToEndOfFile()
        let errText = String(data: err.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        p.waitUntilExit()
        guard p.terminationStatus == 0 else {
            throw Failure(message: errText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? String(localized: "“\(url.lastPathComponent)” could not be opened.") : errText)
        }
        // the plist may come after the licence text: start at its XML header
        let xml = data.range(of: Data("<?xml".utf8)).map { data[$0.lowerBound...] } ?? data
        guard let plist = try? PropertyListSerialization.propertyList(from: Data(xml), format: nil) as? [String: Any],
            let entities = plist["system-entities"] as? [[String: Any]],
            let mount = entities.compactMap({ $0["mount-point"] as? String }).first
        else {
            throw Failure(message: String(localized: "“\(url.lastPathComponent)” was opened but has no disk to show."))
        }
        return URL(fileURLWithPath: mount)
    }
}
