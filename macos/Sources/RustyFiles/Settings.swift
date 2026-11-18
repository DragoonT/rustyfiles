import AppKit

extension Notification.Name {
    /// A setting changed; every window applies the new values at once.
    static let rfSettingsChanged = Notification.Name("rustyfiles.settingsChanged")
}

/// The app's settings, kept in the user defaults. The names and defaults follow the Windows
/// app's settings (crates/rf-app/src/settings.rs); only the ones the Mac version acts on are here.
@MainActor
enum AppSettings {
    /// "black": the dark palette with every surface pure black (OLED screens).
    enum Theme: String, CaseIterable { case system, light, dark, black }
    enum DateStyle: String, CaseIterable { case app, system, universal }

    /// The interface language: "system" follows macOS (Turkish on a Turkish Mac, else English),
    /// or "en" / "tr". Kept as the app's own AppleLanguages, read when it starts.
    static var language: String {
        get {
            guard let list = UserDefaults.standard.persistentDomain(forName: Bundle.main.bundleIdentifier ?? "")?["AppleLanguages"] as? [String],
                let first = list.first
            else { return "system" }
            return first.hasPrefix("tr") ? "tr" : "en"
        }
        set {
            if newValue == "system" {
                UserDefaults.standard.removeObject(forKey: "AppleLanguages")
            } else {
                UserDefaults.standard.set([newValue], forKey: "AppleLanguages")
            }
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "language"])
        }
    }

    /// The language the interface is in now.
    static var speaksTurkish: Bool { Bundle.main.preferredLocalizations.first == "tr" }

    /// What the window is made of.
    enum Material: String, CaseIterable {
        /// One sheet of Liquid Glass, with glass lenses on the selections.
        case glass
        /// macOS's translucent window material: the frame takes the desktop's tone, the files
        /// are on an opaque surface. (Stored as "finder", its first name.)
        case finder
        /// No transparency, as Finder looks on macOS 26.
        case solid
    }

    private static var d: UserDefaults { .standard }

    private static func get<T: RawRepresentable>(_ key: String, _ fallback: T) -> T where T.RawValue == String {
        d.string(forKey: key).flatMap(T.init(rawValue:)) ?? fallback
    }

    private static func bool(_ key: String, _ fallback: Bool) -> Bool {
        d.object(forKey: key) == nil ? fallback : d.bool(forKey: key)
    }

    private static func set(_ value: Any?, _ key: String) {
        d.set(value, forKey: key)
        apply()
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
    }

    /// The key a `rfSettingsChanged` notification is about.
    static func changedKey(_ note: Notification) -> String { note.userInfo?["key"] as? String ?? "" }

    // MARK: General

    static var dateStyle: DateStyle {
        get { get("dateStyle", .app) }
        set { set(newValue.rawValue, "dateStyle") }
    }

    // MARK: Appearance

    static var theme: Theme {
        get { get("theme", .system) }
        set { set(newValue.rawValue, "theme") }
    }

    /// "Surface wash" (HyperGlass settings), 0…100: how strongly the app's own surfaces cover
    /// the glass. 0 is no cover, 50 the default.
    static var surfaceWash: Int {
        get { d.object(forKey: "surfaceWash") == nil ? 50 : min(max(d.integer(forKey: "surfaceWash"), 0), 100) }
        set { set(min(max(newValue, 0), 100), "surfaceWash") }
    }

    /// The accent colour as "#RRGGBB", or "" for the system's.
    static var accent: String {
        get { d.string(forKey: "accent") ?? "" }
        set { set(newValue, "accent") }
    }

    static var material: Material {
        get { get("material", .glass) }
        set { set(newValue.rawValue, "material") }
    }

    static var isGlass: Bool { material == .glass }

    // MARK: Layout

    static var defaultView: ViewMode {
        get { get("viewMode", .details) }
        set { set(newValue.rawValue, "viewMode") }
    }

    static var sortKey: ListColumn {
        get { get("sortKey", .name) }
        set { set(newValue.rawValue, "sortKey") }
    }

    static var descending: Bool {
        get { bool("descending", false) }
        set { set(newValue, "descending") }
    }

    static var foldersFirst: Bool {
        get { bool("foldersFirst", true) }
        set { set(newValue, "foldersFirst") }
    }

    // MARK: Files and folders

    static var showHidden: Bool {
        get { bool("showHidden", false) }
        set { set(newValue, "showHidden") }
    }

    static var showThumbnails: Bool {
        get { bool("showThumbnails", true) }
        set { set(newValue, "showThumbnails") }
    }

    static var decimalSizes: Bool {
        get { bool("decimalSizes", false) }
        set { set(newValue, "decimalSizes") }
    }

    // MARK: applying

    /// Puts the settings that live outside the windows into effect (theme, rf-core formats).
    static func apply() {
        switch theme {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark, .black: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
        setDecimalSizes(on: decimalSizes)
        Palette.invalidate()
    }

    /// The window and content washes for the surface value, as theme.rs `glass_washes`: 50 is
    /// dark 0.18 / 0.22, light 0.20 / 0.35; 100 the Mica strength; 0 none; straight lines between.
    /// Off glass the washes are the full strength the Windows app gives Mica (100).
    static func washes(dark: Bool) -> (window: CGFloat, content: CGFloat) {
        let (w50, c50, w100, c100): (CGFloat, CGFloat, CGFloat, CGFloat) = dark ? (0.18, 0.22, 0.40, 0.45) : (0.20, 0.35, 0.30, 0.50)
        let v = CGFloat(isGlass ? surfaceWash : 100)
        if v <= 50 {
            let t = v / 50
            return (w50 * t, c50 * t)
        }
        let t = (v - 50) / 50
        return (w50 + (w100 - w50) * t, c50 + (c100 - c50) * t)
    }

    /// The accent swatches of the Windows app's settings.
    static let swatches: [UInt32] = [0x0078D4, 0x8764B8, 0xE3008C, 0xE81123, 0xF7630C, 0xFFB900, 0x107C10, 0x00B7C3, 0x69797E]
}
