import AppKit
import UniformTypeIdentifiers

/// Dragging files out of an archive shown in a list. Each row is a file promise: Finder and
/// other apps get the member written where they drop it. Dropped on rustyfiles itself (a
/// sidebar folder, a tab, another pane), the members are written out first, then copied in
/// like any other files, with the usual progress and name conflicts.
@MainActor
enum ArchiveDrag {
    struct Members: Sendable {
        let archive: Archive
        let inner: [String]
    }

    /// The drag under way from an archive, while it lasts.
    static var current: Members?

    /// What a drop target registers for, on top of file URLs.
    static let draggedTypes: [NSPasteboard.PasteboardType] = NSFilePromiseReceiver.readableDraggedTypes.map { .init($0) }

    /// True when `info` carries members of an archive shown here.
    static func carries(_ info: NSDraggingInfo) -> Bool {
        current != nil && info.draggingSource != nil
            && info.draggingPasteboard.canReadObject(forClasses: [NSFilePromiseReceiver.self], options: nil)
    }

    /// The row's promise: `inner` is the member's path inside the archive.
    static func provider(_ archive: Archive, inner: String, isDir: Bool) -> NSFilePromiseProvider {
        let type = isDir ? UTType.folder : (UTType(filenameExtension: (inner as NSString).pathExtension) ?? .data)
        let p = NSFilePromiseProvider(fileType: type.identifier, delegate: ArchivePromises.shared)
        p.userInfo = ArchivePromises.Payload(archive: archive, inner: inner)
        return p
    }

    /// Writes the dragged members into a scratch folder, then copies them into `dest`.
    static func drop(into dest: URL, window: NSWindow?) {
        guard let m = current else { return }
        Task.detached(priority: .userInitiated) {
            let scratch = ArchiveStore.scratchFolder()
            let result = Result { try m.archive.extract(innerPaths: m.inner, dest: scratch.path) }
            await MainActor.run {
                switch result {
                case .success:
                    let urls = m.inner.map { scratch.appendingPathComponent($0) }
                    Transfer(.copy, urls, into: dest).start(window: window) { _ in }
                case .failure(let e):
                    let alert = NSAlert(error: e)
                    if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
                }
            }
        }
    }
}

/// Keeps the promises made to other apps.
final class ArchivePromises: NSObject, NSFilePromiseProviderDelegate, @unchecked Sendable {
    static let shared = ArchivePromises()

    final class Payload: @unchecked Sendable {
        let archive: Archive
        let inner: String
        init(archive: Archive, inner: String) {
            self.archive = archive
            self.inner = inner
        }
    }

    private let queue: OperationQueue = {
        let q = OperationQueue()
        q.qualityOfService = .userInitiated
        return q
    }()

    func filePromiseProvider(_ provider: NSFilePromiseProvider, fileNameForType fileType: String) -> String {
        ((provider.userInfo as? Payload)?.inner as NSString?)?.lastPathComponent ?? "file"
    }

    func operationQueue(for provider: NSFilePromiseProvider) -> OperationQueue { queue }

    func filePromiseProvider(_ provider: NSFilePromiseProvider, writePromiseTo url: URL, completionHandler: @escaping (Error?) -> Void) {
        guard let p = provider.userInfo as? Payload else { return completionHandler(CocoaError(.fileWriteUnknown)) }
        do {
            // written where the archive keeps it, then moved to the name the other app gave
            let scratch = ArchiveStore.scratchFolder()
            _ = try p.archive.extract(innerPaths: [p.inner], dest: scratch.path)
            try FileManager.default.moveItem(at: scratch.appendingPathComponent(p.inner), to: url)
            try? FileManager.default.removeItem(at: scratch)
            completionHandler(nil)
        } catch {
            completionHandler(error)
        }
    }
}
