import AppKit
import UniformTypeIdentifiers

/// What a row shows besides its name. Icons are the system's own (as the Windows app shows the
/// shell's icons); types, sizes and dates are worded as in the Windows app, sizes and dates by
/// rf-core itself. Nothing here touches the disk except a package's icon, cached per path.
@MainActor
final class ItemDisplay {
    private var iconByExt: [String: NSImage] = [:]
    private var packageIcons: [String: NSImage] = [:]
    private lazy var folderIcon = NSWorkspace.shared.icon(for: .folder)

    /// Extension from the name (the core leaves it empty for folders, but a package's matters).
    private func ext(_ item: FileItem) -> String {
        item.isDir ? (item.name as NSString).pathExtension.lowercased() : item.ext
    }

    /// A folder that macOS treats as one file (an app, a bundle, a Photos library).
    /// The name as shown: without the extension when Files › Show file extensions is off
    /// (folders keep theirs).
    func name(_ item: FileItem) -> String {
        guard !AppSettings.showExtensions, !item.isDir || isPackage(item) else { return item.name }
        return name(original: item.name)
    }

    func name(original: String) -> String {
        guard !AppSettings.showExtensions else { return original }
        let stem = (original as NSString).deletingPathExtension
        return stem.isEmpty ? original : stem
    }

    func isPackage(_ item: FileItem) -> Bool {
        guard item.isDir else { return false }
        let e = ext(item)
        guard !e.isEmpty, let type = UTType(filenameExtension: e) else { return false }
        return type.conforms(to: .package) || type.conforms(to: .application) || type.conforms(to: .bundle)
    }

    func icon(_ item: FileItem, path: String) -> NSImage {
        if isPackage(item) {
            if let cached = packageIcons[path] { return cached }
            let image = NSWorkspace.shared.icon(forFile: path)
            packageIcons[path] = image
            return image
        }
        if item.isDir { return folderIcon }
        if let cached = iconByExt[item.ext] { return cached }
        let type = item.ext.isEmpty ? UTType.data : (UTType(filenameExtension: item.ext) ?? .data)
        let image = NSWorkspace.shared.icon(for: type)
        iconByExt[item.ext] = image
        return image
    }

    /// The Type column, worded as on Windows (icons.rs `type_label_of`): "File folder",
    /// "PDF Document", "XLSX File". An app is "Application".
    func kind(_ item: FileItem) -> String {
        if isPackage(item) {
            return ext(item) == "app" ? String(localized: "Application") : String(localized: "\(ext(item).uppercased()) File")
        }
        if item.isDir { return String(localized: "File folder") }
        switch item.ext {
        case "": return String(localized: "File")
        case "txt": return String(localized: "Text Document")
        case "pdf": return String(localized: "PDF Document")
        case "zip": return String(localized: "Compressed (zipped) Folder")
        default: return String(localized: "\(item.ext.uppercased()) File")
        }
    }

    /// Files show their size; folders leave the cell empty (Windows offers "Calculate size" there).
    func size(_ item: FileItem) -> String {
        item.isDir ? "" : formatSize(bytes: item.size)
    }

    /// Recent times as rf-core words them ("5 min ago", "3 d ago"); older ones in the system's
    /// own short date and time, as the Windows app uses Windows' regional format.
    func date(_ ms: Int64) -> String { Self.date(ms, style: AppSettings.dateStyle) }

    /// `ms` as `style` writes it.
    static func date(_ ms: Int64, style: AppSettings.DateStyle) -> String {
        guard ms > 0 else { return "" }
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        switch style {
        case .universal: return Self.universal.string(from: date)
        case .system: return Self.shortDate.string(from: date)
        case .app:
            let age = Date().timeIntervalSince(date)
            if age >= 0 && age < 7 * 86_400 { return formatDate(ms: ms) }
            return Self.shortDate.string(from: date)
        }
    }

    /// What `style` makes of a time five minutes ago and one a month ago, for Settings.
    static func example(_ style: AppSettings.DateStyle) -> String {
        let now = Date().timeIntervalSince1970
        let recent = Int64((now - 5 * 60) * 1000), old = Int64((now - 30 * 86_400) * 1000)
        let a = date(recent, style: style), b = date(old, style: style)
        return "\(a) · \(b)"
    }

    private static let universal: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH:mm"
        return f
    }()

    private static let shortDate: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .short
        f.timeStyle = .short
        return f
    }()
}
