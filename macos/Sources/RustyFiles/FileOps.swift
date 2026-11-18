import AppKit

/// File operations, run off the main thread by the caller. Names for new and copied items
/// follow the Windows app: "New folder", "New folder (2)", "report - Copy", "report - Copy (2)".
enum FileOps {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// `base` (+ extension) in `dir`, made unique with " (2)", " (3)" … before the extension.
    static func unique(_ base: String, ext: String, in dir: URL) -> URL {
        let fm = FileManager.default
        func url(_ stem: String) -> URL {
            dir.appendingPathComponent(ext.isEmpty ? stem : "\(stem).\(ext)")
        }
        var candidate = url(base)
        var n = 2
        while fm.fileExists(atPath: candidate.path) {
            candidate = url("\(base) (\(n))")
            n += 1
        }
        return candidate
    }

    static func newFolder(in dir: URL) throws -> URL {
        let url = unique(String(localized: "New folder"), ext: "", in: dir)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false)
        return url
    }

    /// Copies (or moves) `urls` into `dir`. A copy into the folder it came from is named
    /// "name - Copy"; anything else whose name is taken gets " (2)" and so on. Nothing is
    /// overwritten. Returns the new items.
    static func paste(_ urls: [URL], into dir: URL, move: Bool) throws -> [URL] {
        let fm = FileManager.default
        var made: [URL] = []
        for src in urls {
            let ext = src.pathExtension
            let stem = src.deletingPathExtension().lastPathComponent
            var dest = dir.appendingPathComponent(src.lastPathComponent)
            if move && src.deletingLastPathComponent().standardizedFileURL == dir.standardizedFileURL { continue }
            if dir.standardizedFileURL.path.hasPrefix(src.standardizedFileURL.path + "/") || dir.standardizedFileURL == src.standardizedFileURL {
                throw Failure(message: String(localized: "“\(src.lastPathComponent)” can’t be put inside itself."))
            }
            if fm.fileExists(atPath: dest.path) {
                let sameFolder = src.deletingLastPathComponent().standardizedFileURL == dir.standardizedFileURL
                dest = unique(sameFolder ? "\(stem) - " + String(localized: "Copy") : stem, ext: ext, in: dir)
            }
            if move {
                try fm.moveItem(at: src, to: dest)
            } else {
                try fm.copyItem(at: src, to: dest)
            }
            made.append(dest)
        }
        return made
    }

    /// Moves `urls` to the Trash; returns (where each went, where it was) for the undo.
    static func trash(_ urls: [URL]) throws -> [(from: URL, to: URL)] {
        var out: [(from: URL, to: URL)] = []
        for u in urls {
            var result: NSURL?
            try FileManager.default.trashItem(at: u, resultingItemURL: &result)
            if let r = result as URL? { out.append((from: r, to: u)) }
        }
        TrashBin.remember(out)
        return out
    }

    static func rename(_ url: URL, to name: String) throws -> URL {
        guard !name.contains("/"), name != ".", name != ".." else {
            throw Failure(message: String(localized: "A name can’t contain “/”."))
        }
        let dest = url.deletingLastPathComponent().appendingPathComponent(name)
        // a change of case only is a rename of its own on a case-insensitive disk
        if dest.path.lowercased() != url.path.lowercased(), FileManager.default.fileExists(atPath: dest.path) {
            throw Failure(message: String(localized: "There is already an item named “\(name)”."))
        }
        try FileManager.default.moveItem(at: url, to: dest)
        return dest
    }
}

/// The clipboard for files. macOS has no "cut" for files, so a cut is remembered here along
/// with the pasteboard's change count: a paste moves only while that copy is still on it.
@MainActor
enum Clipboard {
    private static var cutChangeCount: Int?

    static func put(_ urls: [URL], cut: Bool) {
        guard !urls.isEmpty else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects(urls as [NSURL])
        cutChangeCount = cut ? pb.changeCount : nil
    }

    static var hasFiles: Bool {
        NSPasteboard.general.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true])
    }

    /// The files to paste and whether they move. A cut is used up by its paste.
    static func take() -> ([URL], Bool)? {
        let pb = NSPasteboard.general
        guard let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty else {
            return nil
        }
        let move = cutChangeCount == pb.changeCount
        if move {
            cutChangeCount = nil
            pb.clearContents()
        }
        return (urls, move)
    }
}

/// Watches one folder and calls `onChange` (debounced) when something in it is added,
/// removed or renamed.
final class DirectoryWatcher {
    private var source: DispatchSourceFileSystemObject?
    private var pending: DispatchWorkItem?

    init?(url: URL, onChange: @escaping @MainActor () -> Void) {
        let fd = open(url.path, O_EVTONLY)
        guard fd >= 0 else { return nil }
        let s = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .delete, .rename, .link], queue: .main)
        s.setEventHandler { [weak self] in
            self?.pending?.cancel()
            let work = DispatchWorkItem { MainActor.assumeIsolated { onChange() } }
            self?.pending = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
        }
        s.setCancelHandler { close(fd) }
        s.resume()
        source = s
    }

    deinit {
        pending?.cancel()
        source?.cancel()
    }
}
