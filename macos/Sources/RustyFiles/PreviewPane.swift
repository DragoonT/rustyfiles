import AppKit
import AVFoundation
import ImageIO
import Quartz

/// The preview / details pane at the right of the list (preview.rs): a large picture of the
/// focused item, its name, and its properties. Quick Look draws the picture, so pictures,
/// PDFs, text, code, audio and video (playable) show as they do in Finder; a folder shows its
/// icon and how much is in it. Nothing is read on the main thread.
@MainActor
final class PreviewPane: FlippedView {
    static let minWidth: CGFloat = 220
    static let maxWidth: CGFloat = 720
    static let defaultWidth: CGFloat = 300

    private let visualBox = FlippedView()
    private var quickLook: QLPreviewView?
    private let iconView = NSImageView()
    private let nameLabel = NSTextField(wrappingLabelWithString: "")
    private let rows = FlippedView()
    private let empty = NSTextField(labelWithString: String(localized: "Select an item to preview"))
    private let emptyIcon = NSImageView()
    private let scroll = NSScrollView()
    private let content = FlippedView()
    private var shown: URL?
    private var properties: [(String, String)] = []
    private var generation = 0

    override init(frame: NSRect) {
        super.init(frame: frame)
        visualBox.wantsLayer = true
        visualBox.layer?.cornerRadius = Metrics.radius
        visualBox.layer?.masksToBounds = true
        iconView.imageScaling = .scaleProportionallyUpOrDown
        visualBox.addSubview(iconView)
        nameLabel.font = NSFont.systemFont(ofSize: 14, weight: .semibold)
        nameLabel.maximumNumberOfLines = 4
        nameLabel.isSelectable = true
        empty.font = Metrics.textSm
        empty.alignment = .center
        emptyIcon.image = Icons.named("panel-right")
        emptyIcon.contentTintColor = .tertiaryLabelColor
        [visualBox, nameLabel, rows].forEach(content.addSubview)
        scroll.documentView = content
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        [scroll, empty, emptyIcon].forEach(addSubview)
        show(nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Under the list (Appearance › Preview pane position): the picture at the left, the
    /// name and properties beside it (preview_wide.rs).
    var atBottom = false { didSet { if atBottom != oldValue { needsLayout = true; needsDisplay = true } } }

    override func draw(_ dirtyRect: NSRect) {
        // the divider on the side next to the list (`border_l_1`, or `border_t_1` at the bottom)
        let p = Palette.of(self)
        p.divider.setFill()
        (atBottom ? NSRect(x: 0, y: 0, width: bounds.width, height: 1) : NSRect(x: 0, y: 0, width: 1, height: bounds.height)).fill()
    }

    override func layout() {
        super.layout()
        let p = Palette.of(self)
        visualBox.layer?.backgroundColor = p.hover.cgColor
        scroll.frame = atBottom ? NSRect(x: 0, y: 1, width: bounds.width, height: bounds.height - 1) : NSRect(x: 1, y: 0, width: bounds.width - 1, height: bounds.height)
        emptyIcon.frame = NSRect(x: (bounds.width - 32) / 2, y: bounds.height / 2 - 40, width: 32, height: 32)
        empty.frame = NSRect(x: 12, y: bounds.height / 2, width: bounds.width - 24, height: 20)
        empty.textColor = p.textSecondary
        layoutContent()
    }

    /// `p_3 gap_3`: the picture 232 high, the name, then the property rows.
    private func layoutContent() {
        if atBottom { return layoutWide() }
        let w = scroll.contentSize.width
        let inner = w - 24
        var y: CGFloat = 12
        let visualH: CGFloat = 232
        visualBox.frame = NSRect(x: 12, y: y, width: inner, height: visualH)
        quickLook?.frame = visualBox.bounds
        iconView.frame = NSRect(x: (inner - 96) / 2, y: (visualH - 96) / 2, width: 96, height: 96)
        y += visualH + 12
        nameLabel.preferredMaxLayoutWidth = inner
        let nh = nameLabel.sizeThatFits(NSSize(width: inner, height: 200)).height
        nameLabel.frame = NSRect(x: 12, y: y, width: inner, height: nh)
        y += nh + 12
        let rh = layoutRows(width: inner)
        rows.frame = NSRect(x: 12, y: y, width: inner, height: rh)
        y += rh + 12
        content.frame = NSRect(x: 0, y: 0, width: w, height: max(y, scroll.contentSize.height))
    }

    private func layoutWide() {
        let w = scroll.contentSize.width, h = scroll.contentSize.height
        let side = min(h - 24, 320, w * 0.4)
        visualBox.frame = NSRect(x: 12, y: 12, width: side, height: side)
        quickLook?.frame = visualBox.bounds
        iconView.frame = NSRect(x: (side - 96) / 2, y: (side - 96) / 2, width: 96, height: 96)
        let x = 12 + side + 16, inner = w - x - 12
        nameLabel.preferredMaxLayoutWidth = inner
        let nh = nameLabel.sizeThatFits(NSSize(width: inner, height: 200)).height
        nameLabel.frame = NSRect(x: x, y: 12, width: inner, height: nh)
        let rh = layoutRows(width: inner)
        rows.frame = NSRect(x: x, y: 12 + nh + 10, width: inner, height: rh)
        content.frame = NSRect(x: 0, y: 0, width: w, height: max(12 + nh + 10 + rh + 12, side + 24, h))
    }

    /// Key 92 wide in the secondary colour, the value beside it (`prop_row`).
    private func layoutRows(width: CGFloat) -> CGFloat {
        var y: CGFloat = 0
        for pair in rows.subviews.chunked(2) {
            guard pair.count == 2, let key = pair[0] as? NSTextField, let value = pair[1] as? NSTextField else { continue }
            let vw = width - 92 - 8
            value.preferredMaxLayoutWidth = vw
            let h = max(value.sizeThatFits(NSSize(width: vw, height: 400)).height, 16)
            key.frame = NSRect(x: 0, y: y, width: 92, height: 16)
            value.frame = NSRect(x: 100, y: y, width: vw, height: h)
            y += h + 6
        }
        return max(y - 6, 0)
    }

    // MARK: what it shows

    /// Shows `url` (the focused item), or the empty prompt for nil.
    func show(_ url: URL?) {
        guard url != shown || url == nil else { return }
        shown = url
        generation += 1
        let none = url == nil
        scroll.isHidden = none
        empty.isHidden = !none
        emptyIcon.isHidden = !none
        guard let url else {
            quickLook?.previewItem = nil
            return
        }
        let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .isPackageKey])
        let folder = values?.isDirectory == true && values?.isPackage != true
        nameLabel.stringValue = FileManager.default.displayName(atPath: url.path)

        if folder {
            quickLook?.isHidden = true
            iconView.isHidden = false
            iconView.image = NSWorkspace.shared.icon(forFile: url.path)
        } else {
            if quickLook == nil, let q = QLPreviewView(frame: visualBox.bounds, style: .compact) {
                q.autostarts = false
                q.shouldCloseWithWindow = false
                visualBox.addSubview(q)
                quickLook = q
            }
            quickLook?.isHidden = false
            iconView.isHidden = true
            quickLook?.previewItem = url as NSURL
        }
        setProperties(Self.quickProperties(url, folder: folder))
        // the slow ones (folder contents, picture size, duration) come when they are known
        let gen = generation
        DispatchQueue.global(qos: .userInitiated).async {
            let more = Self.slowProperties(url, folder: folder)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard gen == self.generation, !more.isEmpty else { return }
                    self.setProperties(self.properties + more)
                }
            }
        }
        needsLayout = true
    }

    /// Forgets what it showed, so the next `show` reads it again (the file changed).
    func reload() {
        let u = shown
        shown = nil
        show(u)
    }

    private func setProperties(_ list: [(String, String)]) {
        properties = list
        rows.subviews.forEach { $0.removeFromSuperview() }
        let p = Palette.of(self)
        for (k, v) in list {
            let key = NSTextField(labelWithString: k)
            key.font = Metrics.textXs
            key.textColor = p.textSecondary
            key.lineBreakMode = .byTruncatingTail
            let value = NSTextField(wrappingLabelWithString: v)
            value.font = Metrics.textXs
            value.textColor = p.text
            value.isSelectable = true
            rows.addSubview(key)
            rows.addSubview(value)
        }
        needsLayout = true
    }

    // MARK: properties

    private static let keys: Set<URLResourceKey> = [
        .localizedTypeDescriptionKey, .fileSizeKey, .contentModificationDateKey, .creationDateKey,
        .contentAccessDateKey, .isHiddenKey, .isWritableKey,
    ]

    private static func when(_ d: Date?) -> String? {
        guard let d else { return nil }
        return ItemDisplay.date(Int64(d.timeIntervalSince1970 * 1000), style: AppSettings.dateStyle)
    }

    nonisolated private static func quickPropertiesRaw(_ url: URL) -> URLResourceValues? { try? url.resourceValues(forKeys: keys) }

    private static func quickProperties(_ url: URL, folder: Bool) -> [(String, String)] {
        let v = quickPropertiesRaw(url)
        var out: [(String, String)] = []
        out.append((String(localized: "Type"), v?.localizedTypeDescription ?? (folder ? String(localized: "Folder") : "")))
        if !folder, let s = v?.fileSize { out.append((String(localized: "Size"), formatSize(bytes: UInt64(s)))) }
        if let m = when(v?.contentModificationDate) { out.append((String(localized: "Modified"), m)) }
        if let c = when(v?.creationDate) { out.append((String(localized: "Created"), c)) }
        if let a = when(v?.contentAccessDate) { out.append((String(localized: "Accessed"), a)) }
        // zero-width spaces after each "/" let long paths wrap at folder boundaries
        out.append((String(localized: "Location"), url.deletingLastPathComponent().path.replacingOccurrences(of: "/", with: "/\u{200B}")))
        var attrs: [String] = []
        if v?.isHidden == true { attrs.append(String(localized: "Hidden")) }
        if v?.isWritable == false { attrs.append(String(localized: "Read only")) }
        if !attrs.isEmpty { out.append((String(localized: "Attributes"), attrs.joined(separator: ", "))) }
        return out
    }

    /// Read off the main thread: a folder's contents and size; a picture's dimensions; a
    /// recording's length; Finder tags.
    nonisolated private static func slowProperties(_ url: URL, folder: Bool) -> [(String, String)] {
        var out: [(String, String)] = []
        if folder {
            let n = (try? FileManager.default.contentsOfDirectory(atPath: url.path).count) ?? 0
            out.append((String(localized: "Contains"), n == 1 ? String(localized: "1 item") : String(localized: "\(n) items")))
            out.append((String(localized: "Size"), formatSize(bytes: folderSize(path: url.path))))
        } else if let src = CGImageSourceCreateWithURL(url as CFURL, nil),
            let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
            let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int
        {
            out.append((String(localized: "Dimensions"), "\(w) × \(h)"))
            if let exif = props[kCGImagePropertyExifDictionary] as? [CFString: Any], let taken = exif[kCGImagePropertyExifDateTimeOriginal] as? String {
                out.append((String(localized: "Taken"), taken))
            }
            if let tiff = props[kCGImagePropertyTIFFDictionary] as? [CFString: Any], let model = tiff[kCGImagePropertyTIFFModel] as? String {
                out.append((String(localized: "Camera"), model))
            }
        } else {
            let asset = AVURLAsset(url: url)
            let semaphore = DispatchSemaphore(value: 0)
            nonisolated(unsafe) var seconds: Double = 0
            Task.detached {
                if let d = try? await asset.load(.duration), d.isNumeric { seconds = d.seconds }
                semaphore.signal()
            }
            _ = semaphore.wait(timeout: .now() + 2)
            if seconds > 0 {
                let s = Int(seconds.rounded())
                let text = s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
                out.append((String(localized: "Length"), text))
            }
        }
        if let tags = try? url.resourceValues(forKeys: [.tagNamesKey]).tagNames, !tags.isEmpty {
            out.append((String(localized: "Tags"), tags.joined(separator: ", ")))
        }
        return out
    }
}

/// The pane's left edge: drag to resize (the pane grows to the left).
@MainActor
final class PreviewResizeHandle: FlippedView {
    var onDrag: ((CGFloat) -> Void)?
    var currentWidth: () -> CGFloat = { PreviewPane.defaultWidth }
    private var start: (x: CGFloat, width: CGFloat)?

    override var mouseDownCanMoveWindow: Bool { false }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }
    override func mouseDown(with event: NSEvent) { start = (event.locationInWindow.x, currentWidth()) }

    override func mouseDragged(with event: NSEvent) {
        guard let start else { return }
        onDrag?(min(max(start.width - (event.locationInWindow.x - start.x), PreviewPane.minWidth), PreviewPane.maxWidth))
    }

    override func mouseUp(with event: NSEvent) { start = nil }
}

extension Array {
    func chunked(_ n: Int) -> [[Element]] {
        stride(from: 0, to: count, by: n).map { Array(self[$0..<Swift.min($0 + n, count)]) }
    }
}
