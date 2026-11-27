import AppKit
import CryptoKit

/// Add-ons (rf-app addons.rs): separate programs rustyfiles installs on the user's click and
/// talks to, never code loaded into it. The first is HyperDownload, the download manager.
///
/// WHERE THEY COME FROM: addons.json in rustyfiles' repository, the same list the Windows app
/// reads: each add-on's version and, per platform, its zip's URL and SHA-256 (the Mac's is
/// "macos-arm64", or "macos-x64" on an Intel Mac). A zip whose SHA-256 is not the listed one is not installed. Nothing is
/// downloaded before the user clicks Install or Update.
///
/// WHERE THEY GO: Application Support/rustyfiles/addons/<id>/<version>/, with installed.json
/// beside the versions naming the one in use. An update unpacks the new version next to the old
/// one and then switches installed.json, so a failed update leaves the old one working.
/// RF_ADDONS_MANIFEST names another list (a URL or a file) for testing.
enum Addons {
    static let manifestURL = URL(string: "https://raw.githubusercontent.com/gelenmedya/rustyfiles/main/addons.json")!
    #if arch(x86_64)
    static let platform = "macos-x64"
    #else
    static let platform = "macos-arm64"
    #endif

    struct FileRef: Decodable, Sendable {
        let url: String
        let sha256: String
        let size: UInt64?
    }

    struct Info: Decodable, Sendable {
        let id: String
        let name: String
        let version: String
        let license: String
        let source: String
        let files: [String: FileRef]?
        var file: FileRef? { files?[Addons.platform] }
    }

    private struct Manifest: Decodable { let addons: [Info] }
    private struct Installed: Codable { let version: String; let program: String }

    static var root: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("rustyfiles", isDirectory: true).appendingPathComponent("addons", isDirectory: true)
    }

    /// The add-on list. Blocks: off the main thread.
    static func list() throws -> [Info] {
        let source = ProcessInfo.processInfo.environment["RF_ADDONS_MANIFEST"]
        let url = source.map { $0.hasPrefix("http") ? URL(string: $0)! : URL(fileURLWithPath: $0) } ?? manifestURL
        let data = try fetch(url)
        return try JSONDecoder().decode(Manifest.self, from: data).addons
    }

    /// The installed version of `id` and its program, if it is installed.
    static func installed(_ id: String) -> (version: String, program: URL)? {
        let dir = root.appendingPathComponent(id, isDirectory: true)
        guard let data = try? Data(contentsOf: dir.appendingPathComponent("installed.json")),
            let i = try? JSONDecoder().decode(Installed.self, from: data)
        else { return nil }
        let program = dir.appendingPathComponent(i.version).appendingPathComponent(i.program)
        return FileManager.default.isExecutableFile(atPath: program.path) ? (i.version, program) : nil
    }

    static func program(_ id: String) -> URL? { installed(id)?.program }

    /// Whether `a` is a newer version than `b` ("0.2.0" > "0.1.9"; a leading "v" is ignored).
    static func newer(_ a: String, than b: String) -> Bool {
        func parts(_ s: String) -> [Int] { s.trimmingCharacters(in: CharacterSet(charactersIn: "v")).split(separator: ".").map { Int($0.prefix { $0.isNumber }) ?? 0 } }
        let (x, y) = (parts(a), parts(b))
        for i in 0..<max(x.count, y.count) {
            let (p, q) = (i < x.count ? x[i] : 0, i < y.count ? y[i] : 0)
            if p != q { return p > q }
        }
        return false
    }

    /// Downloads, checks and unpacks `info`, then makes it the version in use. Blocks.
    static func install(_ info: Info, progress: @escaping @Sendable (UInt64, UInt64) -> Void) throws {
        guard let file = info.file, let url = URL(string: file.url) else {
            throw FileOps.Failure(message: String(localized: "\(info.name) is not available for this Mac yet."))
        }
        let data = try fetch(url, progress: progress, expected: file.size ?? 0)
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        guard digest == file.sha256.lowercased() else {
            throw FileOps.Failure(message: String(localized: "The download of \(info.name) is not the one listed (SHA-256 differs); nothing was installed."))
        }
        let dir = root.appendingPathComponent(info.id, isDirectory: true)
        let target = dir.appendingPathComponent(info.version, isDirectory: true)
        let fm = FileManager.default
        try? fm.removeItem(at: target)
        try fm.createDirectory(at: target, withIntermediateDirectories: true)
        let zip = target.appendingPathComponent("download.zip")
        try data.write(to: zip)
        let unzip = Process()
        unzip.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        unzip.arguments = ["-x", "-k", zip.path, target.path]
        try unzip.run()
        unzip.waitUntilExit()
        try? fm.removeItem(at: zip)
        guard unzip.terminationStatus == 0 else { throw FileOps.Failure(message: String(localized: "\(info.name) could not be unpacked.")) }
        // the program: the add-on's own name, wherever the zip put it
        let walker = fm.enumerator(at: target, includingPropertiesForKeys: nil)
        var program: String?
        while let u = walker?.nextObject() as? URL {
            if u.lastPathComponent == info.id, fm.isExecutableFile(atPath: u.path) {
                program = String(u.path.dropFirst(target.path.count + 1))
                break
            }
        }
        guard let program else { throw FileOps.Failure(message: String(localized: "\(info.name)'s program is missing from its download.")) }
        let record = try JSONEncoder().encode(Installed(version: info.version, program: program))
        try record.write(to: dir.appendingPathComponent("installed.json"), options: .atomic)
        // older versions go once the new one is in use
        for old in (try? fm.contentsOfDirectory(atPath: dir.path)) ?? [] where old != info.version && old != "installed.json" {
            try? fm.removeItem(at: dir.appendingPathComponent(old))
        }
    }

    static func remove(_ id: String) {
        try? FileManager.default.removeItem(at: root.appendingPathComponent(id, isDirectory: true))
    }

    /// A URL's body, with progress for a big one. Blocks.
    private static func fetch(_ url: URL, progress: (@Sendable (UInt64, UInt64) -> Void)? = nil, expected: UInt64 = 0) throws -> Data {
        if url.isFileURL { return try Data(contentsOf: url) }
        let done = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var result: Result<Data, Error> = .failure(URLError(.unknown))
        let task = URLSession.shared.dataTask(with: url) { data, response, error in
            if let data, (response as? HTTPURLResponse)?.statusCode ?? 200 < 400 {
                result = .success(data)
            } else {
                result = .failure(error ?? URLError(.badServerResponse))
            }
            done.signal()
        }
        task.resume()
        while done.wait(timeout: .now() + 0.2) == .timedOut {
            progress?(UInt64(max(task.countOfBytesReceived, 0)), max(expected, UInt64(max(task.countOfBytesExpectedToReceive, 0))))
        }
        return try result.get()
    }
}
