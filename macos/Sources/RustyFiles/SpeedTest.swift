import AppKit

/// Settings › Speed test (bench_view.rs): how fast rustyfiles lists folders of 1,000, 10,000
/// and 100,000 files, against the way Finder and most Mac apps list them (Foundation, with the
/// name, size, dates and kind of every item). The folders are made in a scratch place, each
/// test runs three times and the middle time counts; the scratch folders go afterwards.
@MainActor
final class SpeedTest {
    static let shared = SpeedTest()

    struct Row {
        let title: String
        let ours: Double
        let theirs: Double
    }

    enum State {
        case idle
        case running(String)
        case done([Row])
        case failed(String)
    }

    private(set) var state: State = .idle {
        didSet { NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "speedTest"]) }
    }
    private var cancelled = false

    var isRunning: Bool {
        if case .running = state { return true }
        return false
    }

    func stop() { cancelled = true }

    func run() {
        guard !isRunning else { return }
        cancelled = false
        state = .running(String(localized: "Making the test folders…"))
        let sizes = [1_000, 10_000, 100_000]
        DispatchQueue.global(qos: .userInitiated).async {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent("rustyfiles-speed-\(UUID().uuidString)")
            defer { try? FileManager.default.removeItem(at: root) }
            var rows: [Row] = []
            for n in sizes {
                let dir = root.appendingPathComponent("\(n)")
                do {
                    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                    for i in 0..<n {
                        let ext = ["txt", "jpg", "pdf", "swift", "mov"][i % 5]
                        FileManager.default.createFile(atPath: dir.appendingPathComponent(String(format: "file %06d.%@", i, ext)).path, contents: Data(count: i % 7 * 100))
                    }
                } catch {
                    DispatchQueue.main.async { MainActor.assumeIsolated { SpeedTest.shared.state = .failed(error.localizedDescription) } }
                    return
                }
                let title = String(localized: "\(n.formatted()) files")
                DispatchQueue.main.async { MainActor.assumeIsolated { SpeedTest.shared.state = .running(String(localized: "Listing \(title)…")) } }
                let ours = Self.median { _ = try? Folder.open(path: dir.path, column: .name, ascending: true, foldersFirst: true, showHidden: false) }
                let theirs = Self.median { Self.foundationList(dir) }
                rows.append(Row(title: title, ours: ours, theirs: theirs))
                let stop = DispatchQueue.main.sync { MainActor.assumeIsolated { SpeedTest.shared.cancelled } }
                if stop {
                    DispatchQueue.main.async { MainActor.assumeIsolated { SpeedTest.shared.state = .idle } }
                    return
                }
            }
            let done = rows
            DispatchQueue.main.async { MainActor.assumeIsolated { SpeedTest.shared.state = .done(done) } }
        }
    }

    /// Milliseconds, the middle of three runs.
    nonisolated private static func median(_ work: () -> Void) -> Double {
        var times: [Double] = []
        for _ in 0..<3 {
            let t = Date()
            work()
            times.append(Date().timeIntervalSince(t) * 1000)
        }
        return times.sorted()[1]
    }

    /// What Finder-like listing does: every item with its name, size, dates and kind, sorted.
    nonisolated private static func foundationList(_ dir: URL) {
        let keys: [URLResourceKey] = [.nameKey, .fileSizeKey, .contentModificationDateKey, .creationDateKey, .isDirectoryKey, .isHiddenKey, .localizedTypeDescriptionKey]
        let items = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: keys, options: [])) ?? []
        var names: [String] = []
        names.reserveCapacity(items.count)
        for u in items {
            let v = try? u.resourceValues(forKeys: Set(keys))
            names.append(v?.name ?? "")
        }
        names.sort { $0.localizedStandardCompare($1) == .orderedAscending }
    }
}

extension SettingsContent {
    static func speedRows() -> [SettingsRow] {
        let t = SpeedTest.shared
        var rows: [SettingsRow] = []
        let detail: String
        switch t.state {
        case .idle: detail = String(localized: "Lists folders of 1,000, 10,000 and 100,000 files the way rustyfiles does and the way most Mac apps do (Foundation, with each item’s size, dates and kind), three times each. Takes about a minute.")
        case .running(let s): detail = s
        case .done: detail = String(localized: "The middle of three runs. Lower is better.")
        case .failed(let e): detail = e
        }
        rows.append(.card(
            title: String(localized: "Folder listing speed"), detail: detail, icon: "gauge",
            control: .buttons([t.isRunning ? (String(localized: "Stop"), { t.stop() }) : (String(localized: "Run the test"), { t.run() })])))
        if case .done(let results) = t.state {
            func ms(_ v: Double) -> String { v >= 1000 ? String(format: "%.2f s", v / 1000) : String(format: "%.0f ms", v) }
            rows.append(.table(
                title: String(localized: "Results"), detail: nil, icon: nil,
                entries: results.map { r in
                    let x = r.theirs / max(r.ours, 0.01)
                    let faster = x >= 1 ? String(localized: "\(String(format: x >= 10 ? "%.0f" : "%.1f", x))× faster") : String(localized: "slower")
                    return (r.title, "rustyfiles \(ms(r.ours)) · Foundation \(ms(r.theirs)) · \(faster)", nil)
                },
                empty: "", rowActions: [], buttons: []))
        }
        return rows
    }
}
