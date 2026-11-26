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

/// The app's icons. Appearance › Icons picks the set: "macos" draws Apple's SF Symbols, as
/// Finder does; "rustyfiles" draws the Lucide set the Windows app draws (gpui-kit-assets),
/// bundled as SVG in Resources/icons. Both are template images, so they take the text colour.
/// A "place-…" name is a known folder (Desktop, Downloads…): its own symbol on macOS, a plain
/// folder in the Lucide set.
@MainActor
enum Icons {
    private static var cache: [String: NSImage] = [:]

    private static var menuCache: [String: NSImage] = [:]

    /// Appearance › Icons changed: everything is drawn again from the other set.
    static func forget() {
        cache = [:]
        menuCache = [:]
    }

    /// The Lucide file a name is drawn from.
    private static func lucide(_ name: String) -> String {
        guard name.hasPrefix("place-") else { return name }
        return name == "place-home" ? "house" : "folder"
    }

    /// The SF Symbol drawn in place of a Lucide icon (macOS icons); nil: Lucide's own.
    private static let symbols: [String: String] = [
        "app-window": "macwindow",
        "app-window-mac": "square.grid.3x3",
        "archive": "archivebox",
        "arrow-down-wide-narrow": "arrow.up.arrow.down",
        "arrow-left": "chevron.left",
        "arrow-right": "chevron.right",
        "arrow-up": "arrow.up",
        "arrow-up-down": "arrow.up.arrow.down",
        "calendar": "calendar",
        "chevron-down": "chevron.down",
        "chevron-right": "chevron.right",
        "chevron-up": "chevron.up",
        "circle-alert": "exclamationmark.circle",
        "circle-plus": "plus.circle",
        "clipboard-copy": "doc.on.clipboard",
        "clipboard-paste": "doc.on.clipboard",
        "copy": "doc.on.doc",
        "download": "arrow.down.circle",
        "eject": "eject",
        "eye": "eye",
        "eye-off": "eye.slash",
        "file-archive": "doc.zipper",
        "file-text": "doc.text",
        "folder": "folder",
        "folder-down": "tray.and.arrow.down",
        "folder-input": "folder.badge.plus",
        "folder-open": "folder",
        "folder-output": "folder.badge.minus",
        "folder-plus": "folder.badge.plus",
        "folder-search": "folder.badge.questionmark",
        "folders": "rectangle.stack",
        "globe": "globe",
        "hard-drive": "internaldrive",
        "house": "house",
        "image": "photo",
        "info": "info.circle",
        "layout-grid": "square.grid.2x2",
        "library": "building.columns",
        "link": "link",
        "list": "list.bullet",
        "list-checks": "checklist",
        "list-filter": "line.3.horizontal.decrease",
        "monitor": "desktopcomputer",
        "music": "music.note",
        "network": "network",
        "paintbrush": "paintbrush",
        "palette": "paintpalette",
        "panel-left": "sidebar.left",
        "panel-right": "sidebar.right",
        "pencil": "pencil",
        "pin": "pin",
        "pin-off": "pin.slash",
        "plus": "plus",
        "rotate-ccw": "arrow.counterclockwise",
        "rotate-cw": "arrow.clockwise",
        "scissors": "scissors",
        "search": "magnifyingglass",
        "send": "paperplane",
        "server": "server.rack",
        "settings": "gearshape",
        "share-2": "square.and.arrow.up",
        "shield-check": "checkmark.shield",
        "sparkles": "sparkles",
        "square-arrow-out-up-right": "arrow.up.forward.square",
        "square-chevron-right": "chevron.right.square",
        "square-plus": "plus.square",
        "square-terminal": "apple.terminal",
        "sun-moon": "circle.lefthalf.filled",
        "tag": "tag",
        "text-cursor-input": "character.cursor.ibeam",
        "trash": "trash",
        "triangle-alert": "exclamationmark.triangle",
        "type": "textformat",
        "usb": "externaldrive",
        "video": "film",
        "wifi": "wifi",
        "wrench": "wrench.adjustable",
        "x": "xmark",
        "zap": "bolt",
        "undo-2": "arrow.uturn.backward",
        "redo-2": "arrow.uturn.forward",
        "eraser": "eraser",
        "languages": "globe",
        "power": "power",
        "arrow-right-left": "arrow.left.arrow.right",
        "file-type": "doc.richtext",
        "mouse-pointer-click": "cursorarrow.click",
        "corner-left-up": "arrow.turn.left.up",
        "calculator": "plusminus",
        "at-sign": "at",
        "move-vertical": "arrow.up.and.down",
        "paint-bucket": "drop",
        "panel-top": "rectangle.tophalf.inset.filled",
        "panel-bottom": "rectangle.bottomhalf.inset.filled",
        "folder-sync": "arrow.triangle.2.circlepath",
        "group": "rectangle.3.group",
        "keyboard": "keyboard",
        "code": "chevron.left.forwardslash.chevron.right",
        "square-code": "curlybraces.square",
        "layers": "square.3.layers.3d",
        "activity": "waveform.path.ecg",
        "file-cog": "doc.badge.gearshape",
        "scroll-text": "scroll",
        "database": "cylinder",
        "gauge": "gauge.with.dots.needle.33percent",
        "bell": "bell",
        "square-check": "checkmark.square",
        "mouse-pointer": "cursorarrow",
        "wand-sparkles": "wand.and.sparkles",
        "a-large-small": "textformat.size",
        "file": "doc",
        "command": "command",
        "clock": "clock",
        "cloud": "icloud",
        "pencil-line": "pencil.line",
        "chart-pie": "chart.pie",
        "files": "doc.on.doc",
        "archive-restore": "arrow.up.bin",
        "layout-list": "list.bullet.rectangle",
        "rows-3": "rectangle.grid.1x2",
        "columns-3": "rectangle.split.3x1",
        "git-branch": "arrow.triangle.branch",
        "git-branch-plus": "arrow.triangle.branch",
        "refresh-ccw": "arrow.counterclockwise",
        "refresh-cw": "arrow.clockwise",
        "arrow-down-to-line": "arrow.down.to.line",
        "arrow-up-from-line": "arrow.up.to.line",
        "shield": "shield",
        "chevrons-down-up": "chevron.up.chevron.down",
        "chevrons-up-down": "chevron.up.chevron.down",
        "zoom-in": "plus.magnifyingglass",
        "minimize-2": "arrow.down.right.and.arrow.up.left",
        "save": "square.and.arrow.down",
        "file-plus": "doc.badge.plus",
        "flask-conical": "flask",
        "puzzle": "puzzlepiece.extension",
        "pause": "pause",
        "play": "play",
        "list-tree": "list.bullet.indent",
        "place-desktop": "menubar.dock.rectangle",
        "place-downloads": "arrow.down.circle",
        "place-documents": "doc",
        "place-pictures": "photo",
        "place-music": "music.note",
        "place-movies": "film",
        "place-home": "house"
    ]

    /// `name`'s SF Symbol fitted into a square of `side` points with `inset` free round it
    /// (symbols are not square; Lucide's 24-unit box leaves about 2 units of air).
    private static func symbol(_ name: String, side: CGFloat, inset: CGFloat, weight: NSFont.Weight) -> NSImage? {
        guard AppSettings.iconStyle == "macos", let symbolName = symbols[name],
            let base = NSImage(systemSymbolName: symbolName, accessibilityDescription: nil)
        else { return nil }
        let config = NSImage.SymbolConfiguration(pointSize: side, weight: weight)
        let sym = base.withSymbolConfiguration(config) ?? base
        let box = side - inset * 2
        // a symbol's image carries air round its drawing; the drawn part is what is fitted
        let ink = inkBounds(sym)
        let image = NSImage(size: NSSize(width: side, height: side), flipped: false) { rect in
            guard ink.width > 0, ink.height > 0 else { return true }
            let k = min(box / ink.width, box / ink.height)
            let w = ink.width * k, h = ink.height * k
            let x = rect.midX - w / 2 - ink.minX * k, y = rect.midY - h / 2 - ink.minY * k
            sym.draw(in: NSRect(x: x, y: y, width: sym.size.width * k, height: sym.size.height * k))
            return true
        }
        image.isTemplate = true
        return image
    }

    /// The part of `image` that is drawn (not transparent), in its own points.
    private static func inkBounds(_ image: NSImage) -> NSRect {
        let scale: CGFloat = 4
        let w = Int(ceil(image.size.width * scale)), h = Int(ceil(image.size.height * scale))
        guard w > 0, h > 0, let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
            let data = ctx.data
        else { return NSRect(origin: .zero, size: image.size) }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
        image.draw(in: NSRect(x: 0, y: 0, width: CGFloat(w), height: CGFloat(h)))
        NSGraphicsContext.restoreGraphicsState()
        let px = data.bindMemory(to: UInt8.self, capacity: w * h * 4)
        var (x0, y0, x1, y1) = (w, h, -1, -1)
        for row in 0..<h {
            for col in 0..<w where px[(row * w + col) * 4 + 3] > 24 {
                x0 = min(x0, col); x1 = max(x1, col)
                y0 = min(y0, row); y1 = max(y1, row)
            }
        }
        guard x1 >= x0, y1 >= y0 else { return NSRect(origin: .zero, size: image.size) }
        // rows run top-down in memory; the rect is in bottom-up points
        return NSRect(x: CGFloat(x0) / scale, y: CGFloat(h - 1 - y1) / scale, width: CGFloat(x1 - x0 + 1) / scale, height: CGFloat(y1 - y0 + 1) / scale)
    }

    /// The icon for a menu row: 16 pt with a light line, as macOS draws its own menu symbols
    /// (the 24 px, 2 px Lucide drawing reads heavy at menu size).
    static func menu(_ name: String) -> NSImage {
        if let cached = menuCache[name] { return cached }
        if let sym = symbol(name, side: 16, inset: 2, weight: .regular) {
            menuCache[name] = sym
            return sym
        }
        var image = NSImage(size: NSSize(width: 16, height: 16))
        if let url = Bundle.main.url(forResource: lucide(name), withExtension: "svg", subdirectory: "icons"),
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
        if let sym = symbol(name, side: 24, inset: 2.5, weight: .regular) {
            cache[name] = sym
            return sym
        }
        let url = Bundle.main.url(forResource: lucide(name), withExtension: "svg", subdirectory: "icons")
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
