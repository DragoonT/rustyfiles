import AppKit
import NetFS

/// Go › Connect to Server… (Windows: network shares and FTP): an smb://, afp://, nfs://,
/// ftp:// or WebDAV (https://) address is mounted with macOS's own NetFS, asking for a name
/// and password when the server wants them, and the share opens here. The last servers are
/// offered again.
@MainActor
enum ConnectServer {
    private static let key = "recentServers"

    static var recent: [String] {
        get { UserDefaults.standard.stringArray(forKey: key) ?? [] }
        set { UserDefaults.standard.set(Array(newValue.prefix(10)), forKey: key) }
    }

    static func ask(in window: NSWindow?, then open: @escaping (URL) -> Void) {
        let alert = NSAlert()
        alert.messageText = String(localized: "Connect to server")
        alert.informativeText = String(localized: "A server address, such as smb://nas.local/share, afp://, nfs://, ftp:// or https:// for WebDAV.")
        let field = NSComboBox(frame: NSRect(x: 0, y: 0, width: 300, height: 26))
        field.addItems(withObjectValues: recent)
        field.stringValue = recent.first ?? "smb://"
        field.completes = true
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "Connect"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        var text = field.stringValue.trimmingCharacters(in: .whitespaces)
        if !text.contains("://") { text = "smb://" + text }
        guard let url = URL(string: text), url.host != nil else {
            return fail(String(localized: "“\(text)” is not a server address."), window)
        }
        recent = [text] + recent.filter { $0 != text }
        mount(url, window: window, then: open)
    }

    private static func mount(_ url: URL, window: NSWindow?, then open: @escaping (URL) -> Void) {
        let job = StatusCenter.shared.add(title: String(localized: "Connecting to \(url.host ?? url.absoluteString)"), folder: nil, cancel: nil)
        // macOS asks for the name and password itself when the server wants them
        let ui = NSMutableDictionary()
        ui[kNAUIOptionKey] = kNAUIOptionAllowUI
        var request: AsyncRequestID?
        let status = NetFSMountURLAsync(url as CFURL, nil, nil, nil, ui, nil, &request, DispatchQueue.main) { status, _, mounts in
            MainActor.assumeIsolated {
                let points = (mounts as? [String]) ?? []
                if status == 0, let first = points.first {
                    job.succeed()
                    open(URL(fileURLWithPath: first))
                } else if status == 0 || status == Int32(EEXIST) {
                    // already mounted: the volume under /Volumes with the share's name
                    job.succeed()
                    let name = url.lastPathComponent.isEmpty ? (url.host ?? "") : url.lastPathComponent
                    open(URL(fileURLWithPath: "/Volumes").appendingPathComponent(name))
                } else if status == Int32(ECANCELED) || status == -128 {
                    job.fail(String(localized: "Cancelled"))
                } else {
                    let text = String(localized: "Could not connect to \(url.absoluteString) (error \(Int(status))).")
                    job.fail(text)
                    fail(text, window)
                }
            }
        }
        if status != 0 {
            let text = String(localized: "Could not connect to \(url.absoluteString) (error \(Int(status))).")
            job.fail(text)
            fail(text, window)
        }
    }

    private static func fail(_ text: String, _ window: NSWindow?) {
        let a = NSAlert()
        a.messageText = text
        a.alertStyle = .warning
        if let window { a.beginSheetModal(for: window) } else { a.runModal() }
    }
}
