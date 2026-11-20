import AppKit
import Quartz
import QuickLookThumbnailing

/// The grid view's Tiles style, sized as on Windows at its default size (folder_view.rs
/// `GRID_SIZES[2]`): tiles 124 wide in rows 150 high with 4 between them, the picture asked
/// for at 128 and drawn at 96, the label `text_xs`, centred, at most two lines.
enum TileMetrics {
    /// folder_view.rs `GRID_SIZES`: (tile width, row height, picture asked for), smallest first.
    static let sizes: [(CGFloat, CGFloat, CGFloat)] = [(76, 94, 64), (92, 112, 80), (124, 150, 128), (172, 204, 192), (236, 276, 272)]
    /// The size chosen in the Layout flyout (0…4, 2 by default).
    static var size: Int {
        let v = UserDefaults.standard.object(forKey: "tileSize") as? Int ?? 2
        return min(max(v, 0), sizes.count - 1)
    }
    static var width: CGFloat { sizes[size].0 }
    static var rowHeight: CGFloat { sizes[size].1 }
    static let gap: CGFloat = 4
    /// The picture drawn: three quarters of the one asked for.
    static var visual: CGFloat { sizes[size].2 * 0.75 }
    /// The grid's own padding (`py_2`, `px 6`).
    static let padX: CGFloat = 6
    static let padY: CGFloat = 8
}

/// Thumbnails from Quick Look, made off the main thread and kept for the session. A file
/// Quick Look can't picture keeps its icon.
@MainActor
final class ThumbnailCache {
    static let shared = ThumbnailCache()
    private var images: [String: NSImage] = [:]
    private var failed: Set<String> = []
    private var pending: [String: [(NSImage) -> Void]] = [:]

    func cached(_ path: String) -> NSImage? { images[path] }

    /// Calls `done` with the thumbnail when there is one (not at all when there is none).
    func request(_ path: String, scale: CGFloat, done: @escaping (NSImage) -> Void) {
        if let image = images[path] { return done(image) }
        guard !failed.contains(path) else { return }
        if pending[path] != nil {
            pending[path]?.append(done)
            return
        }
        pending[path] = [done]
        let side = TileMetrics.visual
        let request = QLThumbnailGenerator.Request(
            fileAt: URL(fileURLWithPath: path), size: CGSize(width: side, height: side), scale: scale, representationTypes: .thumbnail)
        QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { rep, _ in
            let cg = rep?.cgImage
            Task { @MainActor in
                let waiting = ThumbnailCache.shared.pending.removeValue(forKey: path) ?? []
                guard let cg else {
                    ThumbnailCache.shared.failed.insert(path)
                    return
                }
                let image = NSImage(cgImage: cg, size: NSSize(width: CGFloat(cg.width) / scale, height: CGFloat(cg.height) / scale))
                ThumbnailCache.shared.images[path] = image
                waiting.forEach { $0(image) }
            }
        }
    }

    /// Forgets every thumbnail (Advanced › Cache › Clear).
    func clear() {
        images = [:]
        failed = []
    }

    /// Forgets a folder's thumbnails (its files changed).
    func forget(under dir: String) {
        let prefix = dir.hasSuffix("/") ? dir : dir + "/"
        images = images.filter { !$0.key.hasPrefix(prefix) }
        failed = failed.filter { !$0.hasPrefix(prefix) }
    }
}

/// One tile (folder_view.rs, grid): radius 8, `p_1`, the picture in a box 4 larger than it,
/// the name under it. Selected: selection fill and a 1 px `selection_border`; hover: hover fill.
final class TileView: HoverView {
    let picture = NSImageView()
    let label = NSTextField(wrappingLabelWithString: "")
    /// Cards: the type, size and date beside the picture.
    let facts = NSTextField(wrappingLabelWithString: "")
    /// Tiles, Cards or List (folder_view.rs `GridStyle`).
    var style: ViewMode = .tiles {
        didSet {
            guard style != oldValue else { return }
            applyStyle()
            needsLayout = true
        }
    }
    var isSelected = false { didSet { needsDisplay = true } }
    var isThumbnail = false { didSet { picture.layer?.cornerRadius = isThumbnail ? 4 : 0 } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        picture.imageScaling = .scaleProportionallyUpOrDown
        picture.wantsLayer = true
        picture.layer?.masksToBounds = true
        label.font = Metrics.textXs
        label.alignment = .center
        // two lines at most, the second ending in "…" (a name cut at both ends reads as broken)
        label.maximumNumberOfLines = 2
        label.lineBreakMode = .byWordWrapping
        label.cell?.truncatesLastVisibleLine = true
        label.cell?.lineBreakMode = .byWordWrapping
        facts.font = Metrics.textXs
        facts.maximumNumberOfLines = 3
        facts.isHidden = true
        addSubview(picture)
        addSubview(label)
        addSubview(facts)
    }

    private func applyStyle() {
        switch style {
        case .cards:
            label.font = Metrics.textSmMedium
            label.alignment = .left
            label.maximumNumberOfLines = 1
            label.cell?.lineBreakMode = .byTruncatingTail
            facts.isHidden = false
        case .list:
            label.font = Metrics.textSm
            label.alignment = .left
            label.maximumNumberOfLines = 1
            label.cell?.lineBreakMode = .byTruncatingTail
            facts.isHidden = true
        default:
            label.font = Metrics.textXs
            label.alignment = .center
            label.maximumNumberOfLines = 2
            label.cell?.lineBreakMode = .byWordWrapping
            facts.isHidden = true
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        switch style {
        case .cards:
            // px_2 gap_3: the picture 60 (in a box of 64), the name and the facts beside it
            let side: CGFloat = 60
            picture.frame = NSRect(x: 8 + 2, y: (bounds.height - side) / 2, width: side, height: side)
            let x = 8 + side + 4 + 12, w = bounds.width - x - 8
            let lh = ceil(Metrics.textSmMedium.ascender - Metrics.textSmMedium.descender) + 2
            let fh = min(facts.sizeThatFits(NSSize(width: w, height: 100)).height, 3 * 16)
            let top = (bounds.height - lh - 2 - fh) / 2
            label.frame = NSRect(x: x, y: top, width: w, height: lh)
            facts.frame = NSRect(x: x, y: top + lh + 2, width: w, height: fh)
            return
        case .list:
            // px_2 gap_2: a 20 icon and the name
            picture.frame = NSRect(x: 8, y: (bounds.height - 20) / 2, width: 20, height: 20)
            let lh = ceil(Metrics.textSm.ascender - Metrics.textSm.descender) + 2
            label.frame = NSRect(x: 8 + 20 + 8, y: (bounds.height - lh) / 2, width: bounds.width - 44, height: lh)
            return
        default: break
        }
        let box = TileMetrics.visual + 4
        let boxX = (bounds.width - box) / 2
        picture.frame = NSRect(x: boxX + 2, y: 4 + 2, width: TileMetrics.visual, height: TileMetrics.visual)
        label.frame = NSRect(x: 4, y: 4 + box + 4, width: bounds.width - 8, height: bounds.height - (4 + box + 4) - 4)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: Metrics.radius, yRadius: Metrics.radius)
        if isSelected {
            p.selection.setFill()
            path.fill()
            p.selectionBorder.setStroke()
            path.lineWidth = 1
            path.stroke()
        } else if hovered {
            p.hover.setFill()
            path.fill()
        }
    }
}

final class TileItem: NSCollectionViewItem {
    static let id = NSUserInterfaceItemIdentifier("tile")
    var path = ""

    var tile: TileView { view as! TileView }

    override func loadView() {
        view = TileView(frame: NSRect(origin: .zero, size: ViewMode.tiles.cellSize))
        textField = tile.label
        imageView = tile.picture
    }

    override var isSelected: Bool { didSet { tile.isSelected = isSelected } }
}

/// The grid itself, with the same keys as the list.
@MainActor
final class TileCollectionView: NSCollectionView, NSServicesMenuRequestor {
    weak var list: FileListController?

    override func keyDown(with event: NSEvent) {
        if list?.handleKey(event) == true { return }
        super.keyDown(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        super.mouseDown(with: event)
        if event.clickCount == 2, indexPathForItem(at: convert(event.locationInWindow, from: nil)) != nil {
            list?.openSelected()
        }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let at = indexPathForItem(at: convert(event.locationInWindow, from: nil))
        if let at, !selectionIndexPaths.contains(at) {
            selectionIndexPaths = [at]
            delegate?.collectionView?(self, didSelectItemsAt: [at])
        }
        if at == nil {
            let old = selectionIndexPaths
            selectionIndexPaths = []
            delegate?.collectionView?(self, didDeselectItemsAt: old)
        }
        return list?.contextMenu()
    }

    // macOS's Services and Quick Actions (the Windows app's "Show more options") work on the
    // selected files: the menu shows the ones that take file URLs.
    override func validRequestor(forSendType sendType: NSPasteboard.PasteboardType?, returnType: NSPasteboard.PasteboardType?) -> Any? {
        if sendType == .fileURL, returnType == nil, let list, !list.selectedURLs.isEmpty, list.archiveIsNil { return self }
        return super.validRequestor(forSendType: sendType, returnType: returnType)
    }

    func writeSelection(to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard let urls = list?.selectedURLs, !urls.isEmpty else { return false }
        pboard.clearContents()
        return pboard.writeObjects(urls as [NSURL])
    }

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { true }
    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) { list?.beginQuickLook(panel) }
    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) { list?.endQuickLook(panel) }
}
