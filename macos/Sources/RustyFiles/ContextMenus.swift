import AppKit
import CoreText
import UniformTypeIdentifiers

extension Notification.Name {
    /// The pinned folders changed (the sidebar and Home list them).
    static let rfPinsChanged = Notification.Name("rustyfiles.pinsChanged")
}

/// The Windows app's menus (ctx_menu.rs, fluent_menu.rs) as native menus: a quick row of icon
/// buttons on top, then rows with a Lucide icon, the label and the shortcut as a hint.
@MainActor
struct MenuBuilder {
    let menu = NSMenu()

    init() { menu.autoenablesItems = false }

    /// The row of icon buttons at the top (Cut, Copy, Paste, Rename, Share, Delete…); the
    /// shortcut is in each button's tooltip.
    func quick(_ buttons: [(icon: String, tip: String, enabled: Bool, action: () -> Void)]) {
        guard !buttons.isEmpty else { return }
        let item = NSMenuItem()
        item.view = QuickRowView(buttons)
        menu.addItem(item)
        menu.addItem(.separator())
    }

    @discardableResult
    func add(_ title: String, _ icon: String? = nil, key: String = "", mods: NSEvent.ModifierFlags = .command, enabled: Bool = true,
             checked: Bool = false, _ action: @escaping () -> Void) -> NSMenuItem {
        let i = ClosureMenuItem(title: title, handler: action)
        if let icon { i.image = Icons.menu(icon) }
        i.keyEquivalent = key
        i.keyEquivalentModifierMask = key.isEmpty ? [] : mods
        i.isEnabled = enabled
        i.state = checked ? .on : .off
        menu.addItem(i)
        return i
    }

    @discardableResult
    func sub(_ title: String, _ icon: String? = nil, _ fill: (MenuBuilder) -> Void) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        if let icon { i.image = Icons.menu(icon) }
        let b = MenuBuilder()
        fill(b)
        i.submenu = b.menu
        menu.addItem(i)
        return i
    }

    func separator() {
        // never two in a row, never one first
        if let last = menu.items.last, !last.isSeparatorItem { menu.addItem(.separator()) }
    }

    func done() -> NSMenu {
        while menu.items.last?.isSeparatorItem == true { menu.removeItem(at: menu.items.count - 1) }
        return menu
    }

    static let up = String(Character(UnicodeScalar(NSUpArrowFunctionKey)!))
    static let down = String(Character(UnicodeScalar(NSDownArrowFunctionKey)!))
    static let backspace = String(Character(UnicodeScalar(NSBackspaceCharacter)!))
    static let f2 = String(Character(UnicodeScalar(NSF2FunctionKey)!))
}

/// The quick row: 36×32 icon buttons, 8 in from the menu's edges.
@MainActor
final class QuickRowView: NSView {
    init(_ buttons: [(icon: String, tip: String, enabled: Bool, action: () -> Void)]) {
        super.init(frame: NSRect(x: 0, y: 0, width: CGFloat(buttons.count) * 38 + 14, height: 40))
        for (i, b) in buttons.enumerated() {
            let button = IconButton(icon: b.icon, size: NSSize(width: 36, height: 32), tooltip: b.tip)
            button.isEnabled = b.enabled
            let action = b.action
            button.action = { [weak self] _ in
                self?.enclosingMenuItem?.menu?.cancelTracking()
                DispatchQueue.main.async { action() }
            }
            button.frame = NSRect(x: 8 + CGFloat(i) * 38, y: 4, width: 36, height: 32)
            addSubview(button)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }
}

// MARK: - what the menus do

/// The macOS counterparts of the Windows menus' own commands.
@MainActor
enum MenuActions {
    static func copyPaths(_ urls: [URL]) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(urls.map(\.path).joined(separator: "\n"), forType: .string)
    }

    static func openInTerminal(_ dir: URL) {
        guard let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") else { return }
        NSWorkspace.shared.open([dir], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
    }

    static func setDesktopPicture(_ url: URL) throws {
        for screen in NSScreen.screens { try NSWorkspace.shared.setDesktopImageURL(url, for: screen, options: [:]) }
    }

    /// Fonts are installed for this user, as on Windows.
    static func installFonts(_ urls: [URL]) {
        CTFontManagerRegisterFontURLs(urls as CFArray, .persistent, true, nil)
    }

    static func isPinned(_ url: URL) -> Bool { Pins.load().contains { $0.standardizedFileURL == url.standardizedFileURL } }

    static func setPinned(_ url: URL, _ on: Bool) {
        var pins = Pins.load().filter { $0.standardizedFileURL != url.standardizedFileURL }
        if on { pins.append(url) }
        Pins.save(pins)
        NotificationCenter.default.post(name: .rfPinsChanged, object: nil)
    }

    // MARK: Open with

    /// The apps that open the first file, its default first; "Other…" to choose one.
    static func openWithMenu(_ urls: [URL], _ b: MenuBuilder) {
        guard let first = urls.first else { return }
        let fm = FileManager.default
        let preferred = OpenRules.opener(for: first) ?? NSWorkspace.shared.urlForApplication(toOpen: first)
        var apps = NSWorkspace.shared.urlsForApplications(toOpen: first)
        if let preferred { apps = [preferred] + apps.filter { $0 != preferred } }
        // Settings › File types: the list's apps first, in its order
        let own = (OpenRules.rule(for: first)?.apps ?? []).map { URL(fileURLWithPath: $0) }
        apps = own + apps.filter { !own.contains($0) }
        for app in apps.prefix(24) {
            var title = fm.displayName(atPath: app.path)
            if app == preferred { title += " " + String(localized: "(default)") }
            let item = b.add(title) { NSWorkspace.shared.open(urls, withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration()) }
            let icon = NSWorkspace.shared.icon(forFile: app.path)
            icon.size = NSSize(width: 16, height: 16)
            item.image = icon
        }
        b.separator()
        b.add(String(localized: "Other…"), "layout-grid") {
            let panel = NSOpenPanel()
            panel.directoryURL = URL(fileURLWithPath: "/Applications")
            panel.allowedContentTypes = [.application]
            panel.prompt = String(localized: "Open")
            if panel.runModal() == .OK, let app = panel.url {
                NSWorkspace.shared.open(urls, withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
            }
        }
    }

    // MARK: Tags (Finder's own tags)

    /// Finder's coloured tags (their names as this Mac calls them), checked when every
    /// selected item has the tag; choosing one gives it to all or takes it from all.
    static func tagsMenu(_ urls: [URL], _ b: MenuBuilder) {
        let current = urls.map { Set((try? $0.resourceValues(forKeys: [.tagNamesKey]).tagNames) ?? []) }
        for tag in TagStore.all {
            let name = tag.name
            let all = !current.isEmpty && current.allSatisfy { $0.contains(name) }
            let item = b.add(name, checked: all) {
                for u in urls {
                    var tags = (try? u.resourceValues(forKeys: [.tagNamesKey]).tagNames) ?? []
                    tags.removeAll { $0 == name }
                    if !all { tags.append(name) }
                    try? (u as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
                }
            }
            item.image = TagStore.dot(TagStore.color(tag.color))
        }
        b.separator()
        b.add(String(localized: "Edit tags…"), "settings") { AppDelegate.shared.frontController?.openSettings(page: .tags) }
    }

    private static func dot(_ color: NSColor) -> NSImage {
        NSImage(size: NSSize(width: 12, height: 12), flipped: false) { r in
            color.setFill()
            NSBezierPath(ovalIn: r.insetBy(dx: 1, dy: 1)).fill()
            return true
        }
    }

    // MARK: images and fonts

    static let rotatable: Set<String> = ["jpg", "jpeg", "jfif", "jpe", "png", "bmp", "tif", "tiff", "gif", "ico", "heic", "webp"]
    static let pictures: Set<String> = ["jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "webp", "jfif", "heic", "avif"]
    static let fonts: Set<String> = ["ttf", "otf", "ttc", "fon"]
}

extension FileOps {
    /// "name alias" next to the item, as Finder's Make Alias; " 2", " 3"… when taken.
    static func makeAlias(_ url: URL) throws -> URL {
        let data = try url.bookmarkData(options: .suitableForBookmarkFile, includingResourceValuesForKeys: nil, relativeTo: nil)
        let base = url.lastPathComponent + " " + String(localized: "alias")
        let dest = unique(base, ext: "", in: url.deletingLastPathComponent())
        try URL.writeBookmarkData(data, to: dest)
        return dest
    }

    /// A new folder in `dir` with `urls` moved into it (Windows: "New folder with selection").
    static func newFolder(with urls: [URL], in dir: URL) throws -> URL {
        let folder = unique(String(localized: "New folder with items"), ext: "", in: dir)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
        for u in urls { try FileManager.default.moveItem(at: u, to: folder.appendingPathComponent(u.lastPathComponent)) }
        return folder
    }

    static func newTextDocument(in dir: URL) throws -> URL {
        let url = unique(String(localized: "New text document"), ext: "txt", in: dir)
        guard FileManager.default.createFile(atPath: url.path, contents: Data()) else {
            throw Failure(message: String(localized: "The text document could not be made."))
        }
        return url
    }

    /// Turns pictures by 90° (`sips`, which keeps the format and its metadata).
    static func rotate(_ urls: [URL], clockwise: Bool) throws {
        for u in urls {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/sips")
            p.arguments = ["--rotate", clockwise ? "90" : "270", u.path]
            p.standardOutput = FileHandle.nullDevice
            p.standardError = FileHandle.nullDevice
            try p.run()
            p.waitUntilExit()
            if p.terminationStatus != 0 { throw Failure(message: String(localized: "“\(u.lastPathComponent)” could not be turned.")) }
        }
    }

    /// The archive "Compress to …" makes from `urls`: named after the one item, or after the
    /// folder they are in, made unique.
    static func archiveName(for urls: [URL], ext: String) -> URL {
        let dir = urls[0].deletingLastPathComponent()
        let base = urls.count == 1 ? urls[0].deletingPathExtension().lastPathComponent : dir.lastPathComponent
        return unique(base.isEmpty ? String(localized: "Archive") : base, ext: ext, in: dir)
    }
}
