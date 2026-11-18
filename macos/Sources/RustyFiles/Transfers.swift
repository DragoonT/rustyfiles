import AppKit
import Darwin

// MARK: - undo

/// What undoing (or redoing) an operation does: every file operation is undone by moving
/// things back or by sending what it made to the Trash, so each step has an exact inverse.
/// Nothing is ever deleted for good: replaced items went to the Trash too.
enum FileOp: Sendable {
    case moves([(from: URL, to: URL)])
    case trash([URL])
    indirect case group([FileOp])

    /// Does it, and returns what undoes it.
    func perform() throws -> FileOp {
        let fm = FileManager.default
        switch self {
        case .moves(let pairs):
            var done: [(from: URL, to: URL)] = []
            for p in pairs where fm.fileExists(atPath: p.from.path) {
                try fm.createDirectory(at: p.to.deletingLastPathComponent(), withIntermediateDirectories: true)
                try fm.moveItem(at: p.from, to: p.to)
                done.append((from: p.to, to: p.from))
            }
            return .moves(done.reversed())
        case .trash(let urls):
            var back: [(from: URL, to: URL)] = []
            for u in urls where fm.fileExists(atPath: u.path) {
                var out: NSURL?
                try fm.trashItem(at: u, resultingItemURL: &out)
                if let t = out as URL? { back.append((from: t, to: u)) }
            }
            TrashBin.remember(back)
            return .moves(back)
        case .group(let ops):
            var inverses: [FileOp] = []
            for op in ops { inverses.append(try op.perform()) }
            return .group(inverses.reversed())
        }
    }
}

/// Undo and redo for the file operations of every window (⌘Z / ⇧⌘Z; the Windows app's
/// Ctrl+Z / Ctrl+Y), through the system's undo manager so Edit says "Undo Rename" and the like.
@MainActor
final class FileUndo: NSObject {
    static let shared = FileUndo()
    let manager: UndoManager = {
        let m = UndoManager()
        m.levelsOfUndo = 50
        m.groupsByEvent = false
        return m
    }()

    /// Records an operation that `undo` takes back; `title` names it ("Rename", "Copy"…).
    func record(_ undo: FileOp, title: String) {
        manager.beginUndoGrouping()
        manager.registerUndo(withTarget: self) { me in me.run(undo, title: title) }
        manager.setActionName(title)
        manager.endUndoGrouping()
    }

    private func run(_ op: FileOp, title: String) {
        do {
            let inverse = try op.perform()
            // registered while undoing, this becomes the redo (and the other way round)
            manager.registerUndo(withTarget: self) { me in me.run(inverse, title: title) }
            manager.setActionName(title)
        } catch {
            NSAlert(error: error).runModal()
        }
    }
}

// MARK: - conflicts

enum Resolve: Sendable {
    case replace, skip, keepBoth
}

/// "These items already exist" (conflict_dialog.rs): one answer for all of them, replace,
/// skip or keep both, or cancel. Asked from the copy thread, which waits for it.
@MainActor
enum ConflictDialog {
    static func ask(_ names: [String], from source: URL, to dest: URL, window: NSWindow?) -> Resolve? {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = names.count == 1
            ? String(localized: "“\(names[0])” already exists in “\(dest.lastPathComponent)”")
            : String(localized: "\(names.count) items already exist in “\(dest.lastPathComponent)”")
        alert.informativeText = String(localized: "\(source.lastPathComponent) → \(dest.lastPathComponent). What is there now goes to the Trash if you replace it.")
        alert.addButton(withTitle: names.count == 1 ? String(localized: "Replace the file") : String(localized: "Replace the files"))
        alert.addButton(withTitle: String(localized: "Keep both"))
        alert.addButton(withTitle: names.count == 1 ? String(localized: "Skip this file") : String(localized: "Skip these files"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        switch alert.runModal() {
        case .alertFirstButtonReturn: return .replace
        case .alertSecondButtonReturn: return .keepBoth
        case .alertThirdButtonReturn: return .skip
        default: return nil
        }
    }
}

// MARK: - copy and move

/// A copy or a move with its Status Center card: progress by the byte, speed, time left and
/// Stop. Same-volume moves are renames; copies on APFS are clones where they can be (as
/// Finder does them: instant, no extra space). Runs off the main thread.
final class Transfer: @unchecked Sendable {
    enum Kind: Sendable { case copy, move }

    let kind: Kind
    let sources: [URL]
    let dest: URL
    private let lock = NSLock()
    private var _cancelled = false
    var cancelled: Bool {
        get { lock.withLock { _cancelled } }
        set { lock.withLock { _cancelled = newValue } }
    }
    private var done: UInt64 = 0
    private var total: UInt64 = 0
    private var lastReport = Date.distantPast

    init(_ kind: Kind, _ sources: [URL], into dest: URL) {
        self.kind = kind
        self.sources = sources
        self.dest = dest
    }

    /// The usual operation for a drop from `sources` into `dest`: a move within one volume, a
    /// copy across volumes, as Finder decides.
    static func defaultKind(_ sources: [URL], into dest: URL) -> Kind {
        func volume(_ u: URL) -> URL? { try? u.resourceValues(forKeys: [.volumeURLKey]).volume }
        let target = volume(dest)
        return sources.allSatisfy { volume($0) == target } ? .move : .copy
    }

    /// Starts it; `finished` gets the new items on the main thread (to select them).
    @MainActor
    func start(window: NSWindow?, finished: @escaping ([URL]) -> Void) {
        let count = sources.count
        let what = count == 1 ? sources[0].lastPathComponent : String(localized: "\(count) items")
        let title = kind == .copy
            ? String(localized: "Copying \(what) to \(dest.lastPathComponent)")
            : String(localized: "Moving \(what) to \(dest.lastPathComponent)")
        let card = StatusCenter.shared.add(title: title, folder: dest) { [weak self] in self?.cancelled = true }
        // Settings › Advanced › Copying, read once for the whole run
        limit = Double(AppSettings.copyLimit) * 1_000_000
        verify = AppSettings.verifyCopies
        DispatchQueue.global(qos: .userInitiated).async {
            let result = Result { try self.run(card: card, window: window) }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    switch result {
                    case .success(let outcome):
                        if self.cancelled { card.cancelled() } else { card.succeed() }
                        LongWork.finished(title, seconds: Date().timeIntervalSince(card.started))
                        if let undo = outcome.undo {
                            FileUndo.shared.record(undo, title: self.kind == .copy ? String(localized: "Copy") : String(localized: "Move"))
                        }
                        finished(outcome.made)
                    case .failure(let e):
                        card.fail(e.localizedDescription)
                        Log.write(title + ": " + e.localizedDescription)
                        LongWork.finished(title, seconds: Date().timeIntervalSince(card.started), failed: e.localizedDescription)
                    }
                }
            }
        }
    }

    private struct Outcome {
        let made: [URL]
        let undo: FileOp?
    }

    private struct Step {
        let source: URL
        var target: URL
        var replace: Bool
    }

    private func run(card: StatusCenter.Card, window: NSWindow?) throws -> Outcome {
        let fm = FileManager.default
        var steps: [Step] = []
        for src in sources {
            let s = src.standardizedFileURL, d = dest.standardizedFileURL
            if d.path == s.path || d.path.hasPrefix(s.path + "/") {
                throw FileOps.Failure(message: String(localized: "“\(src.lastPathComponent)” can’t be put inside itself."))
            }
            // moving an item into the folder it is already in does nothing
            if kind == .move && s.deletingLastPathComponent() == d { continue }
            var target = d.appendingPathComponent(s.lastPathComponent)
            // a copy into its own folder is "name - Copy", as on Windows
            if kind == .copy && s.deletingLastPathComponent() == d {
                let ext = s.pathExtension
                target = FileOps.unique(s.deletingPathExtension().lastPathComponent + " - " + String(localized: "Copy"), ext: ext, in: d)
            }
            steps.append(Step(source: s, target: target, replace: false))
        }

        // names already there: one question for all of them
        let clashing = steps.indices.filter { fm.fileExists(atPath: steps[$0].target.path) }
        if !clashing.isEmpty {
            let names = clashing.map { steps[$0].target.lastPathComponent }
            let source = sources[0].deletingLastPathComponent()
            let answer = DispatchQueue.main.sync { MainActor.assumeIsolated { ConflictDialog.ask(names, from: source, to: self.dest, window: window) } }
            guard let answer else {
                cancelled = true
                return Outcome(made: [], undo: nil)
            }
            for i in clashing.reversed() {
                switch answer {
                case .skip: steps.remove(at: i)
                case .replace: steps[i].replace = true
                case .keepBoth:
                    let t = steps[i].target
                    steps[i].target = FileOps.unique(t.deletingPathExtension().lastPathComponent, ext: t.pathExtension, in: t.deletingLastPathComponent())
                }
            }
        }

        // how much there is to do, for the progress
        let sameVolume = kind == .move && Self.defaultKind(steps.map(\.source), into: dest) == .move
        total = sameVolume ? UInt64(steps.count) : steps.reduce(0) { $0 + Self.size(of: $1.source) }
        report(card)

        var made: [URL] = []
        var undo: [FileOp] = []
        var replaced: [URL] = []
        for step in steps {
            if cancelled { break }
            if step.replace {
                // what is replaced goes to the Trash, so the undo can bring it back
                var out: NSURL?
                try fm.trashItem(at: step.target, resultingItemURL: &out)
                if let t = out as URL? { undo.append(.moves([(from: t, to: step.target)])); replaced.append(t) }
            }
            if sameVolume {
                try fm.moveItem(at: step.source, to: step.target)
                undo.append(.moves([(from: step.target, to: step.source)]))
                done += 1
                report(card)
            } else {
                try copy(step.source, to: step.target, card: card)
                if cancelled {
                    try? fm.removeItem(at: step.target)
                    break
                }
                if kind == .move {
                    try fm.removeItem(at: step.source)
                    undo.append(.moves([(from: step.target, to: step.source)]))
                } else {
                    undo.append(.trash([step.target]))
                }
            }
            made.append(step.target)
        }
        // undone last-first: the copies go, then what they replaced comes back
        return Outcome(made: made, undo: undo.isEmpty ? nil : .group(undo.reversed()))
    }

    // MARK: copying with progress

    private static func size(of url: URL) -> UInt64 {
        let keys: Set<URLResourceKey> = [.totalFileAllocatedSizeKey, .fileSizeKey, .isRegularFileKey]
        guard let walker = FileManager.default.enumerator(at: url, includingPropertiesForKeys: Array(keys), options: [], errorHandler: nil) else {
            return UInt64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
        var total = UInt64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        for case let u as URL in walker {
            total += UInt64((try? u.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
        return total
    }

    /// Bytes of the file being copied, as copyfile last reported them.
    fileprivate var fileDone: UInt64 = 0
    fileprivate weak var currentCard: StatusCenter.Card?

    /// Advanced › Copy speed limit (bytes a second, 0 none) and Check copies.
    private var limit: Double = 0
    private var verify = "off"
    private var began = Date()
    private var copied: UInt64 = 0

    fileprivate func progressed(_ bytes: UInt64) -> Bool {
        if bytes >= fileDone {
            done += bytes - fileDone
            copied += bytes - fileDone
        }
        fileDone = bytes
        if limit > 0 {
            // ahead of the limit: wait until the clock catches up with the bytes
            let due = Double(copied) / limit, now = Date().timeIntervalSince(began)
            if due > now { usleep(useconds_t(min(due - now, 1) * 1_000_000)) }
        }
        if let card = currentCard { report(card) }
        return !cancelled
    }

    fileprivate func nextFile() { fileDone = 0 }

    private func copy(_ src: URL, to dst: URL, card: StatusCenter.Card) throws {
        currentCard = card
        nextFile()
        let state = copyfile_state_alloc()
        defer { copyfile_state_free(state) }
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        copyfile_state_set(state, UInt32(COPYFILE_STATE_STATUS_CB), unsafeBitCast(transferCallback, to: UnsafeRawPointer.self))
        copyfile_state_set(state, UInt32(COPYFILE_STATE_STATUS_CTX), ctx)
        let flags = copyfile_flags_t(COPYFILE_ALL | COPYFILE_RECURSIVE | COPYFILE_CLONE | COPYFILE_NOFOLLOW_SRC)
        // a limit means bytes are really written: no clone
        let rc = copyfile(src.path, dst.path, state, limit > 0 ? flags & ~copyfile_flags_t(COPYFILE_CLONE) : flags)
        if rc != 0 && !cancelled {
            throw FileOps.Failure(message: String(localized: "“\(src.lastPathComponent)” could not be copied: \(String(cString: strerror(errno)))"))
        }
        if !cancelled, verify != "off", !Self.same(src, dst, full: verify == "full") {
            throw FileOps.Failure(message: String(localized: "The copy of “\(src.lastPathComponent)” does not match the original."))
        }
    }

    /// Check copies: every file's size (and date: fast) or content (full) against the original.
    private static func same(_ a: URL, _ b: URL, full: Bool) -> Bool {
        let fm = FileManager.default
        var pairs: [(URL, URL)] = [(a, b)]
        if (try? a.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true {
            guard let walker = fm.enumerator(at: a, includingPropertiesForKeys: [.isDirectoryKey], options: []) else { return false }
            pairs = []
            let base = a.standardizedFileURL.path
            for case let u as URL in walker where (try? u.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) != true {
                let rel = String(u.standardizedFileURL.path.dropFirst(base.count))
                pairs.append((u, URL(fileURLWithPath: b.path + rel)))
            }
        }
        for (x, y) in pairs {
            let vx = try? x.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
            let vy = try? y.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
            guard let vx, let vy, vx.fileSize == vy.fileSize else { return false }
            if full {
                guard let hx = try? FileHandle(forReadingFrom: x), let hy = try? FileHandle(forReadingFrom: y) else { return false }
                defer { try? hx.close(); try? hy.close() }
                while true {
                    let dx = (try? hx.read(upToCount: 1 << 20)) ?? Data(), dy = (try? hy.read(upToCount: 1 << 20)) ?? Data()
                    if dx != dy { return false }
                    if dx.isEmpty { break }
                }
            } else if let mx = vx.contentModificationDate, let my = vy.contentModificationDate, abs(mx.timeIntervalSince(my)) > 2 {
                return false
            }
        }
        return true
    }

    /// At most ten updates a second reach the card.
    private func report(_ card: StatusCenter.Card) {
        let now = Date()
        guard now.timeIntervalSince(lastReport) >= 0.1 || done >= total else { return }
        lastReport = now
        let (d, t) = (done, total)
        DispatchQueue.main.async { MainActor.assumeIsolated { card.update(done: d, total: t) } }
    }
}

/// copyfile's status callback: progress inside a file, and the start of the next one.
private let transferCallback: @convention(c) (Int32, Int32, copyfile_state_t?, UnsafePointer<CChar>?, UnsafePointer<CChar>?, UnsafeMutableRawPointer?) -> Int32 = {
    what, stage, state, _, _, ctx in
    guard let ctx else { return COPYFILE_CONTINUE }
    let transfer = Unmanaged<Transfer>.fromOpaque(ctx).takeUnretainedValue()
    if what == COPYFILE_RECURSE_FILE && stage == COPYFILE_START { transfer.nextFile() }
    if what == COPYFILE_COPY_DATA && stage == COPYFILE_PROGRESS, let state {
        var copied: off_t = 0
        copyfile_state_get(state, UInt32(COPYFILE_STATE_COPIED), &copied)
        return transfer.progressed(UInt64(max(copied, 0))) ? COPYFILE_CONTINUE : COPYFILE_QUIT
    }
    return COPYFILE_CONTINUE
}
