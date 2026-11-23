import AppKit

/// rustyfiles' side of HyperDownload, the download manager add-on (rf-app hyperdownload.rs). The
/// add-on is a program of its own: rustyfiles starts it when a download is asked for
/// (`serve --data <dir> --downloads <dir>`), finds its port and secret in its rpc.json and speaks
/// aria2's JSON-RPC to it on 127.0.0.1. A download is a card of the Status Center, like a copy:
/// its bytes, its speed, and cancel. A torrent's files are chosen in a dialog first.
enum HyperDownload {
    static let id = "hyperdownload"

    /// Where the add-on keeps its lists, its torrent session and rpc.json.
    static var dataDir: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("rustyfiles", isDirectory: true).appendingPathComponent("hyperdownload", isDirectory: true)
    }

    static var isInstalled: Bool { Addons.installed(id) != nil }

    /// A running HyperDownload: its address and secret.
    struct Rpc: Sendable {
        let port: Int
        let token: String

        /// One JSON-RPC call; the secret goes first, as aria2 wants it. Blocks.
        func call(_ method: String, _ params: [Any] = []) throws -> Any? {
            let body: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": method, "params": ["token:\(token)"] + params]
            var request = URLRequest(url: URL(string: "http://127.0.0.1:\(port)/jsonrpc")!, timeoutInterval: 10)
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            let done = DispatchSemaphore(value: 0)
            nonisolated(unsafe) var answer: Result<Data, Error> = .failure(URLError(.unknown))
            URLSession.shared.dataTask(with: request) { data, _, error in
                answer = data.map { .success($0) } ?? .failure(error ?? URLError(.badServerResponse))
                done.signal()
            }.resume()
            done.wait()
            let json = try JSONSerialization.jsonObject(with: try answer.get()) as? [String: Any] ?? [:]
            if let e = json["error"] as? [String: Any] {
                throw FileOps.Failure(message: e["message"] as? String ?? "error")
            }
            return json["result"]
        }
    }

    private static func readRpc() -> Rpc? {
        guard let data = try? Data(contentsOf: dataDir.appendingPathComponent("rpc.json")),
            let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let port = v["port"] as? Int, let token = v["token"] as? String
        else { return nil }
        return Rpc(port: port, token: token)
    }

    private static func answers(_ r: Rpc) -> Bool { (try? r.call("aria2.getVersion")) != nil }

    /// The running add-on, started first when it is not running. Blocks (up to ~15 s the first
    /// time): off the main thread.
    static func connect() throws -> Rpc {
        if let r = readRpc(), answers(r) { return r }
        guard let program = Addons.program(id) else {
            throw FileOps.Failure(message: String(localized: "HyperDownload is not installed: Settings › Add-ons installs it."))
        }
        let fm = FileManager.default
        try? fm.createDirectory(at: dataDir, withIntermediateDirectories: true)
        try? fm.removeItem(at: dataDir.appendingPathComponent("rpc.json"))
        let downloads = fm.urls(for: .downloadsDirectory, in: .userDomainMask).first ?? dataDir
        let p = Process()
        p.executableURL = program
        p.arguments = ["serve", "--data", dataDir.path, "--downloads", downloads.path]
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        // it goes on with its downloads when rustyfiles quits
        try p.run()
        let until = Date().addingTimeInterval(15)
        while Date() < until {
            Thread.sleep(forTimeInterval: 0.15)
            if let r = readRpc(), answers(r) { return r }
        }
        throw FileOps.Failure(message: String(localized: "HyperDownload did not start."))
    }

    /// Whether a text is a link HyperDownload takes (http, https, magnet).
    static func isLink(_ text: String) -> Bool {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return (t.hasPrefix("http://") || t.hasPrefix("https://") || t.hasPrefix("magnet:?")) && !t.contains(where: \.isWhitespace)
    }

    /// The name a link's download is shown under before the server has said.
    static func label(of url: String) -> String {
        if url.hasPrefix("magnet:") {
            let dn = url.split(whereSeparator: { $0 == "?" || $0 == "&" }).first { $0.hasPrefix("dn=") }
            return dn.map { String($0.dropFirst(3)).replacingOccurrences(of: "+", with: " ").removingPercentEncoding ?? String($0.dropFirst(3)) } ?? "magnet"
        }
        let noQuery = url.split(whereSeparator: { $0 == "?" || $0 == "#" }).first.map(String.init) ?? url
        return noQuery.split(separator: "/").last.map(String.init) ?? url
    }

    // MARK: asking

    /// "Download from a link…": a link (one on the clipboard is filled in), into `dir`.
    @MainActor
    static func askLink(into dir: URL, window: NSWindow?) {
        let alert = NSAlert()
        alert.messageText = String(localized: "Download from a link")
        alert.informativeText = String(localized: "The file is downloaded into this folder: \(dir.path)")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 380, height: 24))
        field.placeholderString = "https://… / magnet:?…"
        if let clip = NSPasteboard.general.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines), isLink(clip) { field.stringValue = clip }
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "Download"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let url = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isLink(url) else {
            let a = NSAlert()
            a.messageText = String(localized: "Type an http, https or magnet link.")
            a.runModal()
            return
        }
        download(.link(url), into: dir)
    }

    /// A link pasted into a folder: whether to download it there.
    @MainActor
    static func offerPastedLink(_ url: String, into dir: URL) {
        let a = NSAlert()
        a.messageText = String(localized: "Download the link into this folder?")
        a.informativeText = url
        a.addButton(withTitle: String(localized: "Download"))
        a.addButton(withTitle: String(localized: "Cancel"))
        if a.runModal() == .alertFirstButtonReturn { download(.link(url), into: dir) }
    }

    /// A torrent's files with check boxes, the suggested ones ticked; nil: cancelled.
    @MainActor
    static func chooseFiles(name: String, files: [(String, UInt64)], suggested: [Int]) -> [Int]? {
        let a = NSAlert()
        a.messageText = name
        a.informativeText = String(localized: "Choose the files to download")
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        let boxes = files.enumerated().map { i, f in
            let b = NSButton(checkboxWithTitle: "\(f.0)  (\(formatSize(bytes: f.1)))", target: nil, action: nil)
            b.state = suggested.contains(i) ? .on : .off
            b.lineBreakMode = .byTruncatingMiddle
            return b
        }
        boxes.forEach(stack.addArrangedSubview)
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 460, height: min(CGFloat(files.count) * 22 + 8, 320)))
        stack.frame = NSRect(x: 0, y: 0, width: 440, height: CGFloat(files.count) * 22)
        scroll.documentView = stack
        scroll.hasVerticalScroller = true
        a.accessoryView = scroll
        a.addButton(withTitle: String(localized: "Download"))
        a.addButton(withTitle: String(localized: "Cancel"))
        while true {
            guard a.runModal() == .alertFirstButtonReturn else { return nil }
            let chosen = boxes.indices.filter { boxes[$0].state == .on }
            if !chosen.isEmpty { return chosen }
            a.informativeText = String(localized: "Choose at least one file")
        }
    }

    // MARK: downloading

    enum Source: Sendable {
        case link(String)
        case torrentFile(URL)
    }

    /// Downloads into `dir` as a card of the Status Center.
    @MainActor
    static func download(_ source: Source, into dir: URL) {
        let label: String = switch source {
        case .link(let u): label(of: u)
        case .torrentFile(let f): f.deletingPathExtension().lastPathComponent
        }
        final class Flag: @unchecked Sendable { var cancelled = false }
        let flag = Flag()
        let card = StatusCenter.shared.add(title: String(localized: "Downloading: \(label)"), folder: dir, cancel: { flag.cancelled = true })
        DownloadsHistory.add(label: label, folder: dir)
        Task.detached(priority: .utility) {
            func update(_ body: @escaping @MainActor () -> Void) { DispatchQueue.main.async { MainActor.assumeIsolated(body) } }
            do {
                let rpc = try connect()
                let options: [String: Any] = ["dir": dir.path]
                let gidAny: Any? = switch source {
                case .link(let u): try rpc.call("aria2.addUri", [[u], options])
                case .torrentFile(let f): try rpc.call("aria2.addTorrent", [try Data(contentsOf: f).base64EncodedString(), [String](), options])
                }
                let gid = gidAny as? String ?? ""
                var asked = false
                while true {
                    Thread.sleep(forTimeInterval: 0.3)
                    if flag.cancelled {
                        _ = try? rpc.call("aria2.remove", [gid])
                        _ = try? rpc.call("aria2.removeDownloadResult", [gid])
                        update { card.cancelled() }
                        return
                    }
                    guard let v = (try? rpc.call("aria2.tellStatus", [gid])) as? [String: Any] else { continue }
                    func num(_ k: String) -> UInt64 { UInt64(v[k] as? String ?? "") ?? 0 }
                    let status = v["status"] as? String ?? ""
                    let (done, total) = (num("completedLength"), num("totalLength"))
                    update { card.update(done: done, total: max(total, done)) }
                    // a torrent whose file list waits for the user's choice
                    if status == "waiting", v["hdChoice"] as? Bool == true, !asked {
                        asked = true
                        let all = (try? rpc.call("hd.choices")) as? [[String: Any]] ?? []
                        if let c = all.first(where: { $0["gid"] as? String == gid }) {
                            let files = (c["files"] as? [[String: Any]] ?? []).map { ($0["path"] as? String ?? "", UInt64($0["length"] as? String ?? "") ?? 0) }
                            let suggested = (c["suggested"] as? [String] ?? []).compactMap { Int($0) }.filter { $0 > 0 }.map { $0 - 1 }
                            let name = c["name"] as? String ?? label
                            let answer = DispatchQueue.main.sync { MainActor.assumeIsolated { chooseFiles(name: name, files: files, suggested: suggested) } }
                            guard let answer else {
                                _ = try? rpc.call("aria2.remove", [gid])
                                update { card.cancelled() }
                                return
                            }
                            _ = try rpc.call("hd.choose", [gid, answer.map { String($0 + 1) }])
                        }
                        continue
                    }
                    if status == "complete" {
                        update {
                            card.succeed()
                            DownloadsHistory.finish(label: label, folder: dir)
                        }
                        return
                    }
                    if status == "error" || status == "removed" {
                        let message = v["errorMessage"] as? String ?? String(localized: "The download stopped.")
                        update { card.fail(message) }
                        return
                    }
                }
            } catch {
                let message = error.localizedDescription
                update { card.fail(message) }
            }
        }
    }
}

/// The downloads asked for here, newest first (rf-app hd_history.rs), for the Downloads page.
@MainActor
enum DownloadsHistory {
    struct Entry: Codable {
        var label: String
        var folder: String
        var started: Date
        var finished: Date?
    }

    private static let key = "downloadsHistory"

    static var all: [Entry] {
        get { (UserDefaults.standard.data(forKey: key)).flatMap { try? JSONDecoder().decode([Entry].self, from: $0) } ?? [] }
        set { UserDefaults.standard.set(try? JSONEncoder().encode(Array(newValue.prefix(200))), forKey: key) }
    }

    static func add(label: String, folder: URL) {
        all = [Entry(label: label, folder: folder.path, started: Date())] + all
    }

    static func finish(label: String, folder: URL) {
        var list = all
        if let i = list.firstIndex(where: { $0.label == label && $0.folder == folder.path && $0.finished == nil }) {
            list[i].finished = Date()
            all = list
        }
    }

    static func clear() { all = [] }
}
