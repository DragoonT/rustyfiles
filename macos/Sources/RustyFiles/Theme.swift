import AppKit

/// rustyfiles' palette, value for value from the Windows app (crates/rf-app/src/theme.rs,
/// `Palette::light()` and `Palette::dark()`) as it is over glass: the window and content washes
/// follow the "Surface wash" setting (`glass_washes`). The accent is the one chosen in Settings,
/// else the user's macOS accent colour, shaded the way Windows shades its accent (darker on
/// light, lighter on dark).
struct Palette {
    let dark: Bool
    let windowBg: NSColor
    let contentBg: NSColor
    let cardBg: NSColor
    let tileBg: NSColor
    let tileHover: NSColor
    let border: NSColor
    let divider: NSColor
    let text: NSColor
    let textSecondary: NSColor
    let textTertiary: NSColor
    let accent: NSColor
    let accentText: NSColor
    let hover: NSColor
    let pressed: NSColor
    let selection: NSColor
    let selectionBorder: NSColor
    let tabActive: NSColor
    let danger: NSColor

    static func hex(_ v: UInt32, _ a: CGFloat = 1) -> NSColor {
        NSColor(
            srgbRed: CGFloat((v >> 16) & 0xFF) / 255, green: CGFloat((v >> 8) & 0xFF) / 255,
            blue: CGFloat(v & 0xFF) / 255, alpha: a)
    }

    /// "#RRGGBB" → colour.
    static func parse(_ s: String) -> NSColor? {
        let t = s.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        guard t.count == 6, let v = UInt32(t, radix: 16) else { return nil }
        return hex(v)
    }

    /// The accent (chosen, else the system's) as Windows would shade it for this theme.
    @MainActor
    private static func shadedAccent(dark: Bool) -> NSColor {
        let base = parse(AppSettings.accent) ?? NSColor.controlAccentColor
        guard let a = base.usingColorSpace(.sRGB) else { return dark ? hex(0x60CDFF) : hex(0x005FB8) }
        return dark ? (a.blended(withFraction: 0.45, of: .white) ?? a) : (a.blended(withFraction: 0.25, of: .black) ?? a)
    }

    @MainActor
    private static func make(dark: Bool) -> Palette {
        let accent = shadedAccent(dark: dark)
        let wash = AppSettings.washes(dark: dark)
        let material = AppSettings.material
        // Solid: Windows 11's plain window colour; Translucent: the files on an opaque
        // surface; glass: the washes over the glass.
        let windowA: CGFloat = material == .solid ? 1 : (material == .finder ? 0 : wash.window)
        let contentOpaque = material == .finder
        if !dark {
            return Palette(
                dark: false,
                windowBg: hex(0xF3F3F3, windowA),
                contentBg: contentOpaque ? hex(0xFFFFFF) : hex(0xFFFFFF, wash.content),
                cardBg: hex(0xF9F9F9),
                tileBg: hex(0xFFFFFF, 0.70),
                tileHover: hex(0xF9F9F9, 0.50),
                border: hex(0x000000, 0.07),
                divider: hex(0x000000, 0.07),
                text: hex(0x000000, 0.90),
                textSecondary: hex(0x000000, 0.62),
                textTertiary: hex(0x000000, 0.45),
                accent: accent,
                accentText: hex(0xFFFFFF),
                hover: hex(0x000000, 0.04),
                pressed: hex(0x000000, 0.07),
                selection: hex(0x000000, 0.06),
                selectionBorder: accent.withAlphaComponent(0.55),
                tabActive: hex(0xFFFFFF, 0.70),
                danger: hex(0xC42B1C))
        }
        if AppSettings.theme == .black {
            // theme.rs `Palette::black`: every surface black and opaque, lines a little brighter
            return Palette(
                dark: true, windowBg: hex(0x000000), contentBg: hex(0x000000), cardBg: hex(0x0A0A0A),
                tileBg: hex(0xFFFFFF, 0.04), tileHover: hex(0xFFFFFF, 0.08), border: hex(0xFFFFFF, 0.14), divider: hex(0xFFFFFF, 0.12),
                text: hex(0xFFFFFF), textSecondary: hex(0xFFFFFF, 0.72), textTertiary: hex(0xFFFFFF, 0.50),
                accent: accent, accentText: hex(0x000000, 0.90), hover: hex(0xFFFFFF, 0.08), pressed: hex(0xFFFFFF, 0.12),
                selection: hex(0xFFFFFF, 0.11), selectionBorder: accent.withAlphaComponent(0.55), tabActive: hex(0xFFFFFF, 0.08),
                danger: hex(0xFF99A4))
        }
        return Palette(
            dark: true,
            windowBg: hex(0x202020, windowA),
            contentBg: contentOpaque ? hex(0x1E1E1E) : hex(0x141414, wash.content),
            cardBg: hex(0x2C2C2C),
            tileBg: hex(0xFFFFFF, 0.0512),
            tileHover: hex(0xFFFFFF, 0.0837),
            border: hex(0xFFFFFF, 0.08),
            divider: hex(0xFFFFFF, 0.07),
            text: hex(0xFFFFFF, 0.94),
            textSecondary: hex(0xFFFFFF, 0.70),
            textTertiary: hex(0xFFFFFF, 0.46),
            accent: accent,
            accentText: hex(0x000000, 0.90),
            hover: hex(0xFFFFFF, 0.06),
            pressed: hex(0xFFFFFF, 0.10),
            selection: hex(0xFFFFFF, 0.085),
            selectionBorder: accent.withAlphaComponent(0.55),
            tabActive: hex(0xFFFFFF, 0.06),
            danger: hex(0xFF99A4))
    }

    @MainActor private static var cache: [Bool: Palette] = [:]

    /// Drops the cached palettes (a setting they depend on changed).
    @MainActor static func invalidate() { cache = [:] }

    /// The palette for a view's current appearance (light or dark).
    @MainActor
    static func of(_ view: NSView) -> Palette {
        let dark = view.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        if let p = cache[dark] { return p }
        let p = make(dark: dark)
        cache[dark] = p
        return p
    }
}

/// Lucide icons, the same set the Windows app draws (gpui-kit-assets), bundled as SVG in
/// Resources/icons and drawn as template images so they take the text colour.
@MainActor
enum Icons {
    private static var cache: [String: NSImage] = [:]

    private static var menuCache: [String: NSImage] = [:]

    /// The icon for a menu row: 16 pt with a light line, as macOS draws its own menu symbols
    /// (the 24 px, 2 px Lucide drawing reads heavy at menu size).
    static func menu(_ name: String) -> NSImage {
        if let cached = menuCache[name] { return cached }
        var image = NSImage(size: NSSize(width: 16, height: 16))
        if let url = Bundle.main.url(forResource: name, withExtension: "svg", subdirectory: "icons"),
            var svg = try? String(contentsOf: url, encoding: .utf8)
        {
            // drawn at 16 pt: 1.75 units of 24 is about a 1.2 px line, like the system symbols
            svg = svg.replacingOccurrences(of: "stroke-width=\"2\"", with: "stroke-width=\"1.75\"")
            if let img = NSImage(data: Data(svg.utf8)) { image = img }
        }
        image.size = NSSize(width: 16, height: 16)
        image.isTemplate = true
        menuCache[name] = image
        return image
    }

    static func named(_ name: String) -> NSImage {
        if let cached = cache[name] { return cached }
        let url = Bundle.main.url(forResource: name, withExtension: "svg", subdirectory: "icons")
        let image = url.flatMap(NSImage.init(contentsOf:)) ?? NSImage(size: NSSize(width: 16, height: 16))
        image.isTemplate = true
        cache[name] = image
        return image
    }
}

/// A glass lens: the Liquid Glass piece that marks a selection over a glass window (the
/// sidebar's selected place, the active tab, the round glass behind "+"), where the Windows
/// app draws HyperGlass lenses. It sits *under* the drawn content, so text stays on top.
@MainActor
final class LensView: NSGlassEffectView {
    init(cornerRadius: CGFloat) {
        super.init(frame: .zero)
        self.cornerRadius = cornerRadius
        style = .clear
        // live lensing under the pointer and on press (macOS 27)
        if #available(macOS 27.0, *) { effectIsInteractive = true }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    /// Lenses never take clicks; the view above them does.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// Moves to `frame`, sliding there when it was already showing (the lens travels from the
    /// old selection to the new one, as on Windows).
    func move(to frame: NSRect, animated: Bool) {
        // off glass the selections are drawn the Fluent way by their owners
        if frame.isEmpty || !AppSettings.isGlass {
            isHidden = true
            return
        }
        if isHidden || !animated || self.frame.isEmpty || !AppSettings.animations {
            isHidden = false
            self.frame = frame
            return
        }
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.22
            ctx.timingFunction = CAMediaTimingFunction(name: .easeOut)
            animator().frame = frame
        }
    }
}
