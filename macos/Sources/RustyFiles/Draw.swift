import AppKit

/// Sizes from the Windows app (crates/rf-app/src: workspace.rs, folder_view.rs, theme.rs).
/// gpui's rem is 16 px, so `text_sm` is 14 and `text_xs` 12.
enum Metrics {
    static let radius: CGFloat = 8
    /// The corner of a macOS 26 window with a compact toolbar; the window's glass follows it.
    static let windowRadius: CGFloat = 16
    static let radiusSmall: CGFloat = 6
    static let titleBarHeight: CGFloat = 40
    static let navRowHeight: CGFloat = 48
    static let commandBarHeight: CGFloat = 44
    static let headerHeight: CGFloat = 30
    static let rowHeight: CGFloat = 30
    static let statusBarHeight: CGFloat = 32
    static let sidebarWidth: CGFloat = 232
    static let sidebarMin: CGFloat = 160
    static let sidebarMax: CGFloat = 420
    static let bottomSpacer: CGFloat = 8
    static let cardRightMargin: CGFloat = 8
    /// Where the window's own buttons end on the left (the Windows app has its caption buttons on the right).
    static let trafficLightsWidth: CGFloat = 78

    // the interface's fonts: the system's, or the family chosen in Settings › Appearance › Font
    @MainActor static var textSm = font(14)
    @MainActor static var textSmMedium = font(14, .medium)
    @MainActor static var textXs = font(12)
    @MainActor static var textXsSemibold = font(12, .semibold)

    @MainActor
    private static func font(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
        let family = UserDefaults.standard.string(forKey: "fontFamily") ?? ""
        let system = NSFont.systemFont(ofSize: size, weight: weight)
        guard !family.isEmpty else { return system }
        let traits: NSFontTraitMask = weight == .regular ? [] : [.boldFontMask]
        let w = weight == .regular ? 5 : (weight == .medium ? 6 : 8)
        return NSFontManager.shared.font(withFamily: family, traits: traits, weight: w, size: size) ?? system
    }

    /// The font family changed: the fonts are made again (views pick them up as they redraw).
    @MainActor
    static func fontsChanged() {
        textSm = font(14)
        textSmMedium = font(14, .medium)
        textXs = font(12)
        textXsSemibold = font(12, .semibold)
    }

    /// The list's right gutter: `clamp(width * 0.03, 24, 56)` (folder_view.rs).
    static func gutter(listWidth: CGFloat) -> CGFloat { min(max(listWidth * 0.03, 24), 56) }
}

/// A view with a top-left origin, like the Windows layout code.
class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

enum Draw {
    static func roundedRect(_ r: NSRect, radius: CGFloat, fill: NSColor) {
        fill.setFill()
        NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius).fill()
    }

    /// A Lucide icon centred in `box` at `size` points, in `color`.
    @MainActor
    static func icon(_ name: String, in box: NSRect, size: CGFloat, color: NSColor) {
        let r = NSRect(x: box.midX - size / 2, y: box.midY - size / 2, width: size, height: size).integral
        image(Icons.named(name), in: r, color: color)
    }

    /// A template image filled with `color`.
    static func image(_ image: NSImage, in r: NSRect, color: NSColor) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        ctx.saveGState()
        ctx.beginTransparencyLayer(in: r, auxiliaryInfo: nil)
        image.draw(in: r, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        color.setFill()
        r.fill(using: .sourceAtop)
        ctx.endTransparencyLayer()
        ctx.restoreGState()
    }

    /// One line of text, vertically centred in `r`, cut with "…" when it does not fit.
    static func text(
        _ s: String, in r: NSRect, font: NSFont, color: NSColor, align: NSTextAlignment = .left,
        truncation: NSLineBreakMode = .byTruncatingTail
    ) {
        let p = NSMutableParagraphStyle()
        p.alignment = align
        p.lineBreakMode = truncation
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: p]
        let h = ceil(font.ascender - font.descender)
        let line = NSRect(x: r.minX, y: r.midY - h / 2, width: r.width, height: h)
        (s as NSString).draw(with: line, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine], attributes: attrs)
    }

    static func textWidth(_ s: String, font: NSFont) -> CGFloat {
        ceil((s as NSString).size(withAttributes: [.font: font]).width)
    }

    /// The selection marker: a 3 px accent pill on the left edge of `r`, 8 px off the top and bottom (theme.rs `selection_pill`).
    static func selectionPill(in r: NSRect, accent: NSColor) {
        let pill = NSRect(x: r.minX, y: r.minY + 8, width: 3, height: max(r.height - 16, 4))
        roundedRect(pill, radius: 1.5, fill: accent)
    }
}

/// A view that tracks the pointer and redraws when it enters or leaves.
class HoverView: FlippedView {
    var hovered = false { didSet { if hovered != oldValue { needsDisplay = true; hoverChanged() } } }
    var hoverPoint: NSPoint?

    func hoverChanged() {}

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(
            NSTrackingArea(
                rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeInActiveApp, .inVisibleRect],
                owner: self))
    }

    override func mouseEntered(with event: NSEvent) {
        hoverPoint = convert(event.locationInWindow, from: nil)
        hovered = true
    }

    override func mouseMoved(with event: NSEvent) {
        hoverPoint = convert(event.locationInWindow, from: nil)
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) {
        hoverPoint = nil
        hovered = false
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }
}

/// The Windows app's buttons: an icon (optionally a label and a ▾), a rounded hover fill.
/// `IconButton` covers the nav buttons (32×32), the command bar's (36×32), the pill's mode
/// buttons (28×28) and the dropdowns (label + chevron).
@MainActor
final class IconButton: HoverView {
    var icon: String { didSet { needsDisplay = true } }
    var iconSize: CGFloat
    var label: String? { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var chevron = false
    var isEnabled = true { didSet { needsDisplay = true; updateLens() } }
    /// On state (filter, preview pane, the pill's mode buttons): selection fill, accent icon.
    var isOn = false { didSet { needsDisplay = true; updateLens() } }
    /// Held down while its menu is open.
    var isOpen = false { didSet { needsDisplay = true } }
    /// A progress ring round the button (0…1), as Safari's downloads button has; nil: none.
    var ring: Double? { didSet { if ring != oldValue { needsDisplay = true } } }
    var tint: (Palette) -> NSColor = { $0.text }
    var action: ((IconButton) -> Void)?
    private var pressed = false { didSet { needsDisplay = true } }
    private let fixedSize: NSSize
    /// The glass under the button while the pointer is on it (in place of the hover fill).
    private var lens: LensView?

    init(icon: String, size: NSSize, iconSize: CGFloat = 16, tooltip: String? = nil, action: ((IconButton) -> Void)? = nil) {
        self.icon = icon
        self.iconSize = iconSize
        self.fixedSize = size
        self.action = action
        super.init(frame: NSRect(origin: .zero, size: size))
        toolTip = tooltip
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    override func hoverChanged() { updateLens() }

    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        if superview == nil { lens?.removeFromSuperview() }
    }

    override var frame: NSRect { didSet { lens?.frame = frame } }

    private func updateLens() {
        guard hovered, isEnabled, !isOn, AppSettings.isGlass, let superview else {
            lens?.removeFromSuperview()
            lens = nil
            return
        }
        if lens == nil {
            let l = LensView(cornerRadius: Metrics.radiusSmall)
            superview.addSubview(l, positioned: .below, relativeTo: self)
            lens = l
        }
        lens?.frame = frame
    }

    override var intrinsicContentSize: NSSize {
        guard label != nil || chevron else { return fixedSize }
        // px_2, icon, gap_1, label, gap_1, chevron 12, px_2
        var w: CGFloat = 8 + iconSize + 8
        if let label { w += 4 + Draw.textWidth(label, font: Metrics.textSm) }
        if chevron { w += 4 + 12 }
        return NSSize(width: w, height: fixedSize.height)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let box = bounds
        if isEnabled {
            if pressed || isOpen {
                Draw.roundedRect(box, radius: Metrics.radiusSmall, fill: p.pressed)
            } else if isOn {
                Draw.roundedRect(box, radius: Metrics.radiusSmall, fill: p.selection)
            } else if hovered && !AppSettings.isGlass {
                Draw.roundedRect(box, radius: Metrics.radiusSmall, fill: p.hover)
            }
            // on glass, hover is the lens under the button (`updateLens`)
        }
        let color = !isEnabled ? p.textTertiary : (isOn ? p.accent : tint(p))
        if let ring {
            let r = min(box.width, box.height) / 2 - 2
            let c = NSPoint(x: box.midX, y: box.midY)
            let track = NSBezierPath()
            track.appendArc(withCenter: c, radius: r, startAngle: 0, endAngle: 360)
            track.lineWidth = 2
            p.pressed.setStroke()
            track.stroke()
            // clockwise from the top (flipped view: angles run the other way)
            let arc = NSBezierPath()
            arc.appendArc(withCenter: c, radius: r, startAngle: -90, endAngle: -90 + 360 * CGFloat(min(max(ring, 0.02), 1)), clockwise: false)
            arc.lineWidth = 2
            arc.lineCapStyle = .round
            p.accent.setStroke()
            arc.stroke()
        }
        if label == nil && !chevron {
            Draw.icon(icon, in: box, size: iconSize, color: color)
            return
        }
        var x: CGFloat = 8
        Draw.icon(icon, in: NSRect(x: x, y: 0, width: iconSize, height: box.height), size: iconSize, color: color)
        x += iconSize
        if let label {
            x += 4
            let w = Draw.textWidth(label, font: Metrics.textSm)
            Draw.text(label, in: NSRect(x: x, y: 0, width: w, height: box.height), font: Metrics.textSm, color: isEnabled ? p.text : p.textTertiary)
            x += w
        }
        if chevron {
            x += 4
            Draw.icon("chevron-down", in: NSRect(x: x, y: 0, width: 12, height: box.height), size: 12, color: isEnabled ? p.textSecondary : p.textTertiary)
        }
    }

    override func mouseDown(with event: NSEvent) {
        guard isEnabled else { return }
        pressed = true
    }

    override func mouseDragged(with event: NSEvent) {
        guard isEnabled else { return }
        pressed = bounds.contains(convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) {
        guard isEnabled, pressed else { return }
        pressed = false
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action?(self) }
    }

    /// Opens `menu` under the button, held down while it is open.
    func pop(_ menu: NSMenu) {
        isOpen = true
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: bounds.height + 4), in: self)
        isOpen = false
    }
}

/// A 1 px vertical divider (`bg(divider)`).
final class Divider: FlippedView {
    override func draw(_ dirtyRect: NSRect) {
        Palette.of(self).divider.setFill()
        bounds.fill()
    }

    override func viewDidChangeEffectiveAppearance() { needsDisplay = true }
}
