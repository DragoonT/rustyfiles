import AppKit

/// The Layout page's settings (settings.rs): one layout for every folder or one each, the
/// adaptive layout, grouping, the details columns.
extension AppSettings {
    private static var d: UserDefaults { .standard }

    private static func store(_ value: Any?, _ key: String) {
        if let value { d.set(value, forKey: key) } else { d.removeObject(forKey: key) }
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
    }

    /// One layout and sort for every folder (on), or each folder remembers its own.
    static var syncLayout: Bool {
        get { d.object(forKey: "syncLayout") == nil ? true : d.bool(forKey: "syncLayout") }
        set { store(newValue, "syncLayout") }
    }

    /// Folders that are mostly pictures and videos open as tiles.
    static var adaptiveLayout: Bool {
        get { d.bool(forKey: "adaptiveLayout") }
        set { store(newValue, "adaptiveLayout") }
    }

    enum GroupBy: String, CaseIterable {
        case none, type, date, size, name

        var title: String {
            switch self {
            case .none: String(localized: "None")
            case .type: String(localized: "Type")
            case .date: String(localized: "Date modified")
            case .size: String(localized: "Size")
            case .name: String(localized: "Name")
            }
        }

        /// The sort a grouping needs so its groups are whole.
        var column: ListColumn? {
            switch self {
            case .none: nil
            case .type: .type
            case .date: .modified
            case .size: .size
            case .name: .name
            }
        }
    }

    static var groupBy: GroupBy {
        get { d.string(forKey: "groupBy").flatMap(GroupBy.init(rawValue:)) ?? .none }
        set { store(newValue.rawValue, "groupBy") }
    }

    /// The details columns shown after Name, in their fixed order.
    static var columns: [ListColumn] {
        get {
            guard let list = d.stringArray(forKey: "columns") else { return [.modified, .type, .size] }
            return ListColumn.optional.filter { list.contains($0.rawValue) }
        }
        set { store(newValue.map(\.rawValue), "columns") }
    }

    static func showsColumn(_ c: ListColumn) -> Bool { columns.contains(c) }

    static func setShowsColumn(_ c: ListColumn, _ on: Bool) {
        var list = columns
        list.removeAll { $0 == c }
        if on { list.append(c) }
        columns = ListColumn.optional.filter { list.contains($0) }
    }
}

/// What a folder remembers when Layout › Same layout everywhere is off.
@MainActor
enum FolderLayouts {
    struct Layout {
        var view: ViewMode
        var sort: ListColumn
        var ascending: Bool
        var group: AppSettings.GroupBy
    }

    private static let key = "folderLayouts"

    static func get(_ url: URL) -> Layout? {
        guard let d = (UserDefaults.standard.dictionary(forKey: key)?[url.standardizedFileURL.path]) as? [String: String] else { return nil }
        return Layout(
            view: d["view"].flatMap(ViewMode.init(rawValue:)) ?? AppSettings.defaultView,
            sort: d["sort"].flatMap(ListColumn.init(rawValue:)) ?? AppSettings.sortKey,
            ascending: d["ascending"] != "false",
            group: d["group"].flatMap(AppSettings.GroupBy.init(rawValue:)) ?? .none)
    }

    static func set(_ url: URL, _ l: Layout) {
        var all = UserDefaults.standard.dictionary(forKey: key) ?? [:]
        all[url.standardizedFileURL.path] = ["view": l.view.rawValue, "sort": l.sort.rawValue, "ascending": l.ascending ? "true" : "false", "group": l.group.rawValue]
        // kept to the last 2000 folders
        if all.count > 2000, let some = all.keys.first { all.removeValue(forKey: some) }
        UserDefaults.standard.set(all, forKey: key)
    }
}

extension SettingsContent {
    static func layoutRows() -> [SettingsRow] {
        var rows: [SettingsRow] = [
            .card(
                title: String(localized: "Same layout and sort in every folder"),
                detail: String(localized: "Off: each folder remembers its own layout, sort and grouping."), icon: "folder-sync",
                control: .toggle(get: { AppSettings.syncLayout }, set: { AppSettings.syncLayout = $0 })),
            .card(
                title: String(localized: "Default layout"),
                detail: String(localized: "Adaptive: folders that are mostly pictures and videos open as tiles."), icon: "layout-grid",
                control: .choice(
                    options: [("details", String(localized: "Details")), ("tiles", String(localized: "Tiles")), ("adaptive", String(localized: "Adaptive"))],
                    get: { AppSettings.adaptiveLayout ? "adaptive" : AppSettings.defaultView.rawValue },
                    set: {
                        AppSettings.adaptiveLayout = $0 == "adaptive"
                        if $0 != "adaptive" { AppSettings.defaultView = ViewMode(rawValue: $0) ?? .details }
                    })),
            .section(String(localized: "Sort and group")),
            .card(
                title: String(localized: "Sort by"), detail: nil, icon: "arrow-up-down",
                control: .choice(
                    options: ListColumn.allCases.filter(\.sortable).map { ($0.rawValue, $0.title) },
                    get: { AppSettings.sortKey.rawValue },
                    set: { AppSettings.sortKey = ListColumn(rawValue: $0) ?? .name })),
            .card(
                title: String(localized: "Descending"), detail: nil, icon: "arrow-down-wide-narrow",
                control: .toggle(get: { AppSettings.descending }, set: { AppSettings.descending = $0 })),
            .card(
                title: String(localized: "Folders first"), detail: nil, icon: "folders",
                control: .toggle(get: { AppSettings.foldersFirst }, set: { AppSettings.foldersFirst = $0 })),
            .card(
                title: String(localized: "Group by"), detail: String(localized: "Groups show in the Details layout; the list is sorted by what it is grouped by."),
                icon: "group",
                control: .choice(
                    options: AppSettings.GroupBy.allCases.map { ($0.rawValue, $0.title) },
                    get: { AppSettings.groupBy.rawValue },
                    set: { AppSettings.groupBy = AppSettings.GroupBy(rawValue: $0) ?? .none })),
            .section(String(localized: "Details columns")),
            .card(title: String(localized: "Name"), detail: String(localized: "Always shown."), icon: nil, control: .none),
        ]
        rows += ListColumn.optional.map { c in
            .card(title: c.title, detail: nil, icon: nil, control: .toggle(get: { AppSettings.showsColumn(c) }, set: { AppSettings.setShowsColumn(c, $0) }))
        }
        return rows
    }
}

// MARK: - group titles

@MainActor
enum Grouping {
    /// The group an item belongs to.
    static func title(_ item: FileItem, by: AppSettings.GroupBy, display: ItemDisplay) -> String {
        let folder = item.isDir && !display.isPackage(item)
        switch by {
        case .none: return ""
        case .type: return folder ? String(localized: "File folder") : display.kind(item)
        case .name:
            guard let c = item.name.uppercased().first else { return "#" }
            if c.isLetter { return String(c) }
            return c.isNumber ? "0–9" : "#"
        case .size:
            if folder { return String(localized: "Folders") }
            switch item.size {
            case 0: return String(localized: "Empty")
            case ..<16_384: return String(localized: "Tiny (under 16 KB)")
            case ..<1_048_576: return String(localized: "Small (under 1 MB)")
            case ..<134_217_728: return String(localized: "Medium (under 128 MB)")
            case ..<1_073_741_824: return String(localized: "Large (under 1 GB)")
            default: return String(localized: "Huge (1 GB and more)")
            }
        case .date:
            guard item.modifiedMs > 0 else { return String(localized: "Unknown") }
            let date = Date(timeIntervalSince1970: Double(item.modifiedMs) / 1000)
            let cal = Calendar.current
            let now = Date()
            if cal.isDateInToday(date) { return String(localized: "Today") }
            if cal.isDateInYesterday(date) { return String(localized: "Yesterday") }
            if cal.isDate(date, equalTo: now, toGranularity: .weekOfYear) { return String(localized: "Earlier this week") }
            if let last = cal.date(byAdding: .weekOfYear, value: -1, to: now), cal.isDate(date, equalTo: last, toGranularity: .weekOfYear) {
                return String(localized: "Last week")
            }
            if cal.isDate(date, equalTo: now, toGranularity: .month) { return String(localized: "Earlier this month") }
            if let last = cal.date(byAdding: .month, value: -1, to: now), cal.isDate(date, equalTo: last, toGranularity: .month) {
                return String(localized: "Last month")
            }
            if cal.isDate(date, equalTo: now, toGranularity: .year) { return String(localized: "Earlier this year") }
            return String(cal.component(.year, from: date))
        }
    }
}

// MARK: - Git status column

/// `git status` for the folder shown: each entry's state, by its name in the folder.
enum GitStatus {
    /// nil when the folder is not in a Git work tree.
    static func read(_ dir: URL) -> [String: String]? {
        var d = dir
        var root: URL?
        while true {
            if FileManager.default.fileExists(atPath: d.appendingPathComponent(".git").path) { root = d; break }
            let up = d.deletingLastPathComponent()
            if up.path == d.path { break }
            d = up
        }
        guard root != nil, FileManager.default.isExecutableFile(atPath: "/usr/bin/git") else { return nil }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        p.arguments = ["status", "--porcelain=v1", "-z", "--untracked-files=normal", "--", "."]
        p.currentDirectoryURL = dir
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        guard p.terminationStatus == 0 else { return nil }
        // paths are relative to the work tree's root; ask where we are in it
        let prefix = gitPrefix(dir)
        var result: [String: String] = [:]
        let fields = data.split(separator: 0).map { String(decoding: $0, as: UTF8.self) }
        var i = 0
        while i < fields.count {
            let f = fields[i]
            i += 1
            guard f.count > 3 else { continue }
            let code = String(f.prefix(2))
            var path = String(f.dropFirst(3))
            if code.contains("R") { i += 1 }  // a rename carries the old path next
            guard path.hasPrefix(prefix) else { continue }
            path.removeFirst(prefix.count)
            guard let first = path.split(separator: "/").first.map(String.init) else { continue }
            let state = describe(code)
            // a folder takes the strongest state of what is inside it
            if let old = result[first], rank(old) >= rank(state) { continue }
            result[first] = state
        }
        return result
    }

    private static func gitPrefix(_ dir: URL) -> String {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        p.arguments = ["rev-parse", "--show-prefix"]
        p.currentDirectoryURL = dir
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return "" }
        let s = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        p.waitUntilExit()
        return s
    }

    private static func describe(_ code: String) -> String {
        if code == "??" { return String(localized: "Untracked") }
        if code.contains("U") { return String(localized: "Conflict") }
        if code.contains("D") { return String(localized: "Deleted") }
        if code.contains("R") { return String(localized: "Renamed") }
        if code.contains("A") { return String(localized: "Added") }
        return String(localized: "Modified")
    }

    private static func rank(_ s: String) -> Int {
        [String(localized: "Untracked"), String(localized: "Added"), String(localized: "Renamed"), String(localized: "Modified"),
         String(localized: "Deleted"), String(localized: "Conflict")].firstIndex(of: s) ?? 0
    }
}
