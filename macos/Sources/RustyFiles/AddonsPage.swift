import AppKit

/// Settings › Add-ons (rf-app addons_page.rs): each add-on with Install, Update or Remove, its
/// licence and source; HyperDownload's downloads under it.
@MainActor
enum AddonsPage {
    /// The list as last read (read when the page is first shown, and by "Look again").
    private static var listed: [Addons.Info]?
    private static var reading = false
    private static var busy: String?
    private static var problem: String?

    private static func changed() {
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "addons"])
    }

    private static func read() {
        guard !reading else { return }
        reading = true
        Task.detached(priority: .utility) {
            let result = Result { try Addons.list() }
            await MainActor.run {
                reading = false
                switch result {
                case .success(let l): listed = l; problem = nil
                case .failure(let e): problem = e.localizedDescription
                }
                changed()
            }
        }
    }

    private static func install(_ info: Addons.Info) {
        busy = String(localized: "Installing \(info.name)…")
        changed()
        let card = StatusCenter.shared.add(title: String(localized: "Installing \(info.name)"), folder: nil, cancel: nil)
        Task.detached(priority: .userInitiated) {
            let result = Result { try Addons.install(info) { done, total in DispatchQueue.main.async { MainActor.assumeIsolated { card.update(done: done, total: total) } } } }
            await MainActor.run {
                busy = nil
                switch result {
                case .success: card.succeed(); problem = nil
                case .failure(let e): card.fail(e.localizedDescription); problem = e.localizedDescription
                }
                changed()
            }
        }
    }

    static func rows() -> [SettingsRow] {
        if listed == nil && problem == nil { read() }
        var rows: [SettingsRow] = [.section(String(localized: "Add-ons"))]
        rows.append(.card(
            title: String(localized: "Programs of their own that rustyfiles installs when you ask"),
            detail: String(localized: "An add-on is downloaded only when you click Install, checked against its SHA-256 first, and can be removed at any time."),
            icon: "puzzle", control: .buttons([(String(localized: "Look again"), { listed = nil; problem = nil; read() })])))
        if let problem { rows.append(.card(title: problem, detail: nil, icon: "triangle-alert", control: .none)) }
        if let busy { rows.append(.card(title: busy, detail: nil, icon: "download", control: .none)) }
        for info in listed ?? [] {
            let have = Addons.installed(info.id)
            var buttons: [(String, () -> Void)] = []
            if info.file == nil {
                rows.append(.card(title: info.name, detail: String(localized: "Not available for the Mac yet."), icon: "download", control: .none))
                continue
            }
            if let have {
                if Addons.newer(info.version, than: have.version) {
                    buttons.append((String(localized: "Update to \(info.version)"), { install(info) }))
                }
                buttons.append((String(localized: "Remove"), {
                    Addons.remove(info.id)
                    changed()
                }))
            } else {
                buttons.append((String(localized: "Install"), { install(info) }))
            }
            buttons.append((String(localized: "Source"), { if let u = URL(string: info.source) { NSWorkspace.shared.open(u) } }))
            let state = have.map { String(localized: "Version \($0.version) installed") } ?? String(localized: "Not installed")
            rows.append(.card(
                title: info.name,
                detail: "\(state) · \(info.license)" + (info.id == HyperDownload.id
                    ? " · " + String(localized: "Downloads with many connections, resumes where it stopped, torrents and magnet links. \"Download from a link…\" in a folder's menu.") : ""),
                icon: "download", control: .buttons(buttons)))
        }
        // HyperDownload's downloads (the Windows app's Downloads page, in short)
        let history = DownloadsHistory.all
        if HyperDownload.isInstalled || !history.isEmpty {
            rows.append(.section(String(localized: "Downloads")))
            rows.append(.table(
                title: String(localized: "Downloads"), detail: nil, icon: "download",
                entries: history.prefix(50).map { e in
                    let when = ItemDisplay.date(Int64(e.started.timeIntervalSince1970 * 1000), style: AppSettings.dateStyle)
                    return (e.label, (e.finished == nil ? String(localized: "Not finished") : when) + " · " + (e.folder as NSString).abbreviatingWithTildeInPath, nil)
                },
                empty: String(localized: "Nothing downloaded yet."),
                rowActions: [(String(localized: "Show in folder"), { i in
                    let e = history[i]
                    let file = URL(fileURLWithPath: e.folder).appendingPathComponent(e.label)
                    if FileManager.default.fileExists(atPath: file.path) { NSWorkspace.shared.activateFileViewerSelecting([file]) } else { NSWorkspace.shared.open(URL(fileURLWithPath: e.folder)) }
                })],
                buttons: [(String(localized: "Clear"), { DownloadsHistory.clear(); changed() })]))
        }
        return rows
    }
}
