import AppKit

/// The command bar's Layout flyout (layout_panel.rs, shaped like Files'): the five layouts as
/// tiles, the tiles' size, and switches for hidden items, file extensions, adaptive layout
/// and the shelf. Everything applies at once (and to this folder, when folders keep their own).
@MainActor
final class LayoutFlyout: NSViewController {
    private weak var list: FileListController?
    private var tiles: [LayoutTile] = []
    private let slider = NSSlider(value: Double(TileMetrics.size), minValue: 0, maxValue: Double(TileMetrics.sizes.count - 1), target: nil, action: nil)

    init(list: FileListController) {
        self.list = list
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    static func show(from button: NSView, list: FileListController) {
        let pop = NSPopover()
        pop.behavior = .transient
        pop.contentViewController = LayoutFlyout(list: list)
        pop.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
    }

    override func loadView() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 316, height: 10))
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 14, right: 14)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor), stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.topAnchor.constraint(equalTo: root.topAnchor), stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            root.widthAnchor.constraint(equalToConstant: 316),
        ])

        stack.addArrangedSubview(heading(String(localized: "Layout")))
        let row = NSStackView()
        row.spacing = 4
        for mode in ViewMode.allCases {
            let t = LayoutTile(mode: mode)
            t.onPick = { [weak self] in self?.pick(mode) }
            tiles.append(t)
            row.addArrangedSubview(t)
        }
        stack.addArrangedSubview(row)

        stack.addArrangedSubview(heading(String(localized: "Tile size")))
        slider.numberOfTickMarks = TileMetrics.sizes.count
        slider.allowsTickMarkValuesOnly = true
        slider.target = self
        slider.action = #selector(sized)
        slider.widthAnchor.constraint(equalToConstant: 288).isActive = true
        stack.addArrangedSubview(slider)

        let box = NSBox()
        box.boxType = .separator
        box.widthAnchor.constraint(equalToConstant: 288).isActive = true
        stack.addArrangedSubview(box)
        stack.addArrangedSubview(toggle(String(localized: "Hidden items"), on: AppSettings.showHidden) { AppSettings.showHidden = $0 })
        stack.addArrangedSubview(toggle(String(localized: "File extensions"), on: AppSettings.showExtensions) { AppSettings.showExtensions = $0 })
        stack.addArrangedSubview(toggle(String(localized: "Adaptive layout"), on: AppSettings.adaptiveLayout) { AppSettings.adaptiveLayout = $0 })
        stack.addArrangedSubview(toggle(String(localized: "Shelf"), on: list?.shelfShown ?? false) { [weak self] in self?.list?.shelfShown = $0 })
        let wc = list?.card.window?.windowController as? BrowserWindowController
        stack.addArrangedSubview(toggle(String(localized: "Dual pane"), on: wc?.isDualPane ?? false) { _ in wc?.toggleDualPane(nil) })
        view = root
        refresh()
    }

    private func heading(_ s: String) -> NSTextField {
        let l = NSTextField(labelWithString: s)
        l.font = Metrics.textXsSemibold
        l.textColor = .secondaryLabelColor
        return l
    }

    private func toggle(_ title: String, on: Bool, _ set: @escaping (Bool) -> Void) -> NSView {
        let label = NSTextField(labelWithString: title)
        label.font = Metrics.textSm
        let sw = ClosureSwitch(set)
        sw.state = on ? .on : .off
        sw.controlSize = .small
        let row = NSStackView(views: [label, NSView(), sw])
        row.widthAnchor.constraint(equalToConstant: 288).isActive = true
        return row
    }

    private func refresh() {
        let mode = list?.viewMode ?? .details
        tiles.forEach { $0.isOn = $0.mode == mode }
        slider.isEnabled = mode == .tiles
    }

    private func pick(_ mode: ViewMode) {
        list?.setViewMode(mode)
        refresh()
    }

    @objc private func sized() {
        UserDefaults.standard.set(Int(slider.doubleValue.rounded()), forKey: "tileSize")
        list?.tileSizeChanged()
    }
}

/// An NSSwitch that calls back.
final class ClosureSwitch: NSSwitch {
    private let changed: (Bool) -> Void

    init(_ changed: @escaping (Bool) -> Void) {
        self.changed = changed
        super.init(frame: .zero)
        target = self
        action = #selector(flipped)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func flipped() { changed(state == .on) }
}

/// One layout in the flyout: its icon over its name, the chosen one in the accent.
@MainActor
final class LayoutTile: HoverView {
    let mode: ViewMode
    var onPick: (() -> Void)?
    var isOn = false { didSet { needsDisplay = true } }

    init(mode: ViewMode) {
        self.mode = mode
        super.init(frame: .zero)
        widthAnchor.constraint(equalToConstant: 54).isActive = true
        heightAnchor.constraint(equalToConstant: 54).isActive = true
        toolTip = mode.title
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        if isOn {
            Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.selection)
            p.accent.setStroke()
            let b = NSBezierPath(roundedRect: r, xRadius: Metrics.radiusSmall, yRadius: Metrics.radiusSmall)
            b.stroke()
        } else if hovered {
            Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover)
        }
        Draw.icon(mode.icon, in: NSRect(x: 0, y: 8, width: bounds.width, height: 20), size: 18, color: isOn ? p.accent : p.text)
        Draw.text(mode.title, in: NSRect(x: 2, y: 32, width: bounds.width - 4, height: 14), font: NSFont.systemFont(ofSize: 10), color: p.textSecondary, align: .center)
    }

    override func mouseUp(with event: NSEvent) { onPick?() }
}
