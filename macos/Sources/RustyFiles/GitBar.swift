import AppKit

/// Built-in Git (git_ui.rs, like Files): in a Git work tree the status bar shows the branch and
/// how far it is ahead of / behind its upstream; a click offers Fetch, Pull, Push and Sync and
/// the branches to switch to, or a new one. Everything runs `/usr/bin/git` off the main thread;
/// network operations show in the Status Center.
enum GitRepo {
    struct Info: Sendable, Equatable {
        let root: URL
        let branch: String
        let ahead: Int
        let behind: Int
        let hasUpstream: Bool
    }

    /// Runs git in `dir`; (exit status, output, error output).
    static func run(_ args: [String], in dir: URL) -> (Int32, String, String) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        p.arguments = args
        p.currentDirectoryURL = dir
        // never a password prompt nobody can answer
        p.environment = ProcessInfo.processInfo.environment.merging(["GIT_TERMINAL_PROMPT": "0"]) { $1 }
        let out = Pipe(), err = Pipe()
        p.standardOutput = out
        p.standardError = err
        guard (try? p.run()) != nil else { return (-1, "", "git could not be started") }
        let o = out.fileHandleForReading.readDataToEndOfFile()
        let e = err.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return (p.terminationStatus, String(decoding: o, as: UTF8.self), String(decoding: e, as: UTF8.self))
    }

    /// The repository `dir` is in, or nil.
    static func info(_ dir: URL) -> Info? {
        guard dir.isFileURL, FileManager.default.isExecutableFile(atPath: "/usr/bin/git") else { return nil }
        // a quick look for a .git above, so folders outside any repository cost nothing
        var d = dir
        var found = false
        while true {
            if FileManager.default.fileExists(atPath: d.appendingPathComponent(".git").path) { found = true; break }
            let up = d.deletingLastPathComponent()
            if up.path == d.path { break }
            d = up
        }
        guard found else { return nil }
        let (st, out, _) = run(["status", "--porcelain=v2", "--branch", "--untracked-files=no"], in: dir)
        guard st == 0 else { return nil }
        var branch = "", ahead = 0, behind = 0, upstream = false
        for line in out.split(separator: "\n") {
            if line.hasPrefix("# branch.head ") { branch = String(line.dropFirst("# branch.head ".count)) }
            if line.hasPrefix("# branch.upstream ") { upstream = true }
            if line.hasPrefix("# branch.ab ") {
                for part in line.dropFirst("# branch.ab ".count).split(separator: " ") {
                    if part.hasPrefix("+") { ahead = Int(part.dropFirst()) ?? 0 }
                    if part.hasPrefix("-") { behind = Int(part.dropFirst()) ?? 0 }
                }
            }
        }
        if branch == "(detached)" { branch = String(localized: "detached") }
        return Info(root: d, branch: branch, ahead: ahead, behind: behind, hasUpstream: upstream)
    }

    static func branches(_ root: URL) -> [String] {
        let (st, out, _) = run(["for-each-ref", "--format=%(refname:short)", "refs/heads"], in: root)
        return st == 0 ? out.split(separator: "\n").map(String.init) : []
    }
}

extension FileListController {
    /// The branch strip's menu (git_ui.rs): sync, then the branches.
    func gitMenu(_ info: GitRepo.Info) -> NSMenu {
        let b = MenuBuilder()
        let root = info.root
        b.add(String(localized: "Fetch"), "refresh-cw") { [weak self] in self?.git(["fetch", "--prune"], title: String(localized: "Fetch"), in: root, network: true) }
        b.add(String(localized: "Pull"), "arrow-down-to-line", enabled: info.hasUpstream) { [weak self] in self?.git(["pull", "--ff-only"], title: String(localized: "Pull"), in: root, network: true) }
        b.add(String(localized: "Push"), "arrow-up-from-line") { [weak self] in
            self?.git(info.hasUpstream ? ["push"] : ["push", "-u", "origin", info.branch], title: String(localized: "Push"), in: root, network: true)
        }
        b.add(String(localized: "Sync"), "refresh-ccw", enabled: info.hasUpstream) { [weak self] in
            self?.git(["pull", "--ff-only"], title: String(localized: "Sync"), in: root, network: true) {
                self?.git(["push"], title: String(localized: "Sync"), in: root, network: true)
            }
        }
        b.separator()
        b.sub(String(localized: "Switch branch"), "git-branch") { m in
            for name in GitRepo.branches(root) {
                m.add(name, checked: name == info.branch) { [weak self] in
                    guard name != info.branch else { return }
                    self?.git(["switch", name], title: String(localized: "Switch branch"), in: root, network: false)
                }
            }
        }
        b.add(String(localized: "New branch…"), "git-branch-plus") { [weak self] in
            let alert = NSAlert()
            alert.messageText = String(localized: "New branch")
            alert.informativeText = String(localized: "It starts from “\(info.branch)” and becomes the current one.")
            let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
            alert.accessoryView = field
            alert.addButton(withTitle: String(localized: "Create"))
            alert.addButton(withTitle: String(localized: "Cancel"))
            alert.window.initialFirstResponder = field
            guard alert.runModal() == .alertFirstButtonReturn else { return }
            let n = field.stringValue.trimmingCharacters(in: .whitespaces)
            guard !n.isEmpty else { return }
            self?.git(["switch", "-c", n], title: String(localized: "New branch"), in: root, network: false)
        }
        return b.done()
    }

    /// Runs one git command; a network one shows in the Status Center. Then the list and the
    /// strip are read again.
    /// Reads the branch strip again for the folder shown (off the main thread).
    func refreshGit() {
        guard let dir = url, dir.isFileURL, archive == nil else {
            status.git = nil
            return
        }
        Task.detached(priority: .utility) {
            let info = GitRepo.info(dir)
            await MainActor.run { [weak self] in
                guard let self, self.url == dir else { return }
                self.status.git = info
            }
        }
    }

    func git(_ args: [String], title: String, in root: URL, network: Bool, then: (() -> Void)? = nil) {
        let job = network ? StatusCenter.shared.add(title: "Git: \(title) · \(root.lastPathComponent)", folder: nil, cancel: nil) : nil
        Task.detached(priority: .userInitiated) {
            let (st, out, err) = GitRepo.run(args, in: root)
            await MainActor.run { [weak self] in
                let text = (err.isEmpty ? out : err).trimmingCharacters(in: .whitespacesAndNewlines)
                if let job { st == 0 ? job.succeed() : job.fail(text) }
                if st != 0 {
                    self?.onError?(text.isEmpty ? String(localized: "git \(args.first ?? "") failed") : text)
                } else {
                    then?()
                }
                self?.onRefresh?()
                self?.refreshGit()
            }
        }
    }
}
