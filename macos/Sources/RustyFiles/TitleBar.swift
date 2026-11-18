import AppKit

/// What a tab strip needs to draw a tab.
struct TabInfo {
    let title: String
    let icon: String
}

/// The tabs, drawn as in the Windows app (workspace.rs `tab_strip`): one width for all,
/// `clamp((strip - 4) / n - 4, 72, 220)`, 32 high on the bar's bottom edge, top corners
/// rounded 8; the active one stands on a glass lens (laid out by the title bar, under the
/// strip) and its icon is in the accent colour.
/// Clicks on empty space drag the window; a double click zooms it.
@MainActor
final class TabStripView: HoverView {
    var tabs: [TabInfo] = [] { didSet { needsDisplay = true; superview?.needsLayout = true } }
    var active = 0 { didSet { needsDisplay = true; superview?.needsLayout = true } }
    var onSelect: ((Int) -> Void)?
    var onClose: ((Int) -> Void)?
    /// The tab menu for the tab at this index (workspace.rs `tab_menu`).
    var menuFor: ((Int) -> NSMenu?)?

    private var pressedClose: Int?

    private static let minW: CGFloat = 72
    private static let maxW: CGFloat = 220
    private static let height: CGFloat = 32

    override var mouseDownCanMoveWindow: Bool { false }

    /// Width every tab gets, and the strip's total width (the "+" button sits right after it).
    var tabWidth: CGFloat {
        let n = CGFloat(max(tabs.count, 1))
        let strip = max(bounds.width, 120)
        return min(max((strip - 4) / n - 4, Self.minW), Self.maxW)
    }

    var usedWidth: CGFloat { 4 + CGFloat(tabs.count) * (tabWidth + 4) }

    var activeTabRect: NSRect { tabs.indices.contains(active) ? tabRect(active) : .zero }

    /// Tabs stand on the bar's bottom edge, joined to the row under them, as on Windows.
    func tabRect(_ i: Int) -> NSRect {
        NSRect(x: 4 + CGFloat(i) * (tabWidth + 4), y: bounds.height - Self.height, width: tabWidth, height: Self.height)
    }

    private func closeRect(_ i: Int) -> NSRect {
        let r = tabRect(i)
        return NSRect(x: r.maxX - 4 - 22, y: r.midY - 11, width: 22, height: 22)
    }

    private func tab(at p: NSPoint) -> Int? {
        tabs.indices.first { tabRect($0).contains(p) }
    }

    /// Roomy tabs all show their ×; narrow ones only the active and the hovered one.
    private func showsClose(_ i: Int, hoveredTab: Int?) -> Bool {
        tabWidth >= 110 || i == active || i == hoveredTab
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let hoveredTab = hoverPoint.flatMap(tab(at:))
        for (i, t) in tabs.enumerated() {
            let r = tabRect(i)
            let isActive = i == active
            let path = NSBezierPath(roundedTopRect: r, radius: Metrics.radius)
            if isActive && !AppSettings.isGlass {
                // off glass the active tab is filled, as on Windows (no lens)
                p.tabActive.setFill()
                path.fill()
            } else if !isActive && hoveredTab == i {
                p.hover.setFill()
                path.fill()
            }
            // pl_3, icon 14, gap_2, title, gap_2, close 22, pr_1
            var x = r.minX + 12
            Draw.icon(t.icon, in: NSRect(x: x, y: r.minY, width: 14, height: r.height), size: 14, color: isActive ? p.accent : p.textSecondary)
            x += 14 + 8
            let close = showsClose(i, hoveredTab: hoveredTab)
            let textEnd = close ? closeRect(i).minX - 8 : r.maxX - 4
            Draw.text(t.title, in: NSRect(x: x, y: r.minY, width: max(textEnd - x, 0), height: r.height), font: Metrics.textSm, color: isActive ? p.text : p.textSecondary)
            if close {
                let c = closeRect(i)
                if let hp = hoverPoint, c.contains(hp) {
                    Draw.roundedRect(c, radius: Metrics.radiusSmall, fill: p.pressed)
                }
                Draw.icon("x", in: c, size: 12, color: isActive ? p.text : p.textSecondary)
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        guard let i = tab(at: pt) else {
            if event.clickCount == 2 {
                window?.performZoom(nil)
            } else {
                window?.performDrag(with: event)
            }
            return
        }
        if showsClose(i, hoveredTab: i), closeRect(i).contains(pt) {
            pressedClose = i
            return
        }
        onSelect?(i)
    }

    override func mouseUp(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        if let i = pressedClose, closeRect(i).contains(pt) { onClose?(i) }
        pressedClose = nil
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let i = tab(at: convert(event.locationInWindow, from: nil)) else { return nil }
        return menuFor?(i)
    }

    /// Middle click closes a tab, as on Windows.
    override func otherMouseUp(with event: NSEvent) {
        guard event.buttonNumber == 2, let i = tab(at: convert(event.locationInWindow, from: nil)) else { return }
        onClose?(i)
    }
}

extension NSBezierPath {
    /// A rectangle with only its top corners rounded (flipped coordinates: top is minY).
    convenience init(roundedTopRect r: NSRect, radius: CGFloat) {
        self.init()
        move(to: NSPoint(x: r.minX, y: r.maxY))
        line(to: NSPoint(x: r.minX, y: r.minY + radius))
        appendArc(withCenter: NSPoint(x: r.minX + radius, y: r.minY + radius), radius: radius, startAngle: 180, endAngle: 270)
        line(to: NSPoint(x: r.maxX - radius, y: r.minY))
        appendArc(withCenter: NSPoint(x: r.maxX - radius, y: r.minY + radius), radius: radius, startAngle: 270, endAngle: 360)
        line(to: NSPoint(x: r.maxX, y: r.maxY))
        close()
    }
}

/// The 40 px bar at the top: (window buttons) · tabs · "+".
/// Empty space drags the window and a double click zooms it, like a title bar.
@MainActor
final class TitleBarView: FlippedView {
    let strip = TabStripView()
    let plus = IconButton(icon: "plus", size: NSSize(width: 28, height: 28), iconSize: 14, tooltip: String(localized: "New tab"))
    /// The active tab's glass: as wide as the tab and 8 px taller downwards, cut by the strip's
    /// box, so its top corners are round and its bottom is flat (glass.rs `LensShape::Tab`).
    private let tabClip = FlippedView()
    private let tabLens = LensView(cornerRadius: Metrics.radius)
    /// The round glass behind "+" (`LensShape::Circle`).
    private let plusLens = LensView(cornerRadius: 14)

    override init(frame: NSRect) {
        super.init(frame: frame)
        tabClip.wantsLayer = true
        tabClip.layer?.masksToBounds = true
        tabClip.addSubview(tabLens)
        // lenses first: they lie under the tabs' text and the "+" icon
        [tabClip, plusLens, strip, plus].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { true }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { window?.performZoom(nil) } else { window?.performDrag(with: event) }
    }

    override func layout() {
        super.layout()
        // The tabs start right after the window buttons (the sidebar toggle is in the nav row).
        let stripX = Metrics.trafficLightsWidth
        let room = max(bounds.width - stripX - 4 - 28 - 12, 120)
        strip.frame = NSRect(x: stripX, y: 0, width: room, height: bounds.height)
        // the "+" follows the last tab: ml_1 and 2 px off the bottom
        let plusX = stripX + min(strip.usedWidth, room)
        plus.frame = NSRect(x: plusX, y: bounds.height - 2 - 28, width: 28, height: 28)
        plusLens.frame = plus.frame
        plusLens.isHidden = !AppSettings.isGlass

        tabClip.frame = strip.frame
        var lens = strip.activeTabRect
        lens.size.height += Metrics.radius
        tabLens.move(to: lens, animated: !inLiveResize)
    }
}
