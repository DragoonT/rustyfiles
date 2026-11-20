import AppKit

extension Notification.Name {
    static let rfStatusCenterChanged = Notification.Name("rustyfiles.statusCenterChanged")
}

/// The Status Center: long operations as cards (a Ferry transfer for now), each with its
/// progress, its end and a way to stop it. The nav row's ☑ button opens it.
@MainActor
final class StatusCenter {
    static let shared = StatusCenter()

    enum State: Equatable {
        case running, done, cancelled
        case failed(String)
    }

    final class Card {
        let id = UUID()
        let title: String
        /// Where the result is (received files): the card offers to show it.
        let folder: URL?
        private(set) var done: UInt64 = 0
        private(set) var total: UInt64 = 0
        private(set) var state: State = .running
        let cancel: (() -> Void)?
        let started = Date()
        private(set) var ended: Date?
        /// Bytes a second, smoothed so the number does not jump about.
        private(set) var speed: Double = 0
        private var lastSample: (time: Date, done: UInt64)?
        /// The first progress this run saw: a resumed transfer starts with bytes that were
        /// already there, which say nothing about its speed.
        private var firstSample: (time: Date, done: UInt64)?

        init(title: String, folder: URL?, cancel: (() -> Void)?) {
            self.title = title
            self.folder = folder
            self.cancel = cancel
        }

        @MainActor func update(done: UInt64, total: UInt64) {
            let now = Date()
            if let last = lastSample {
                let dt = now.timeIntervalSince(last.time)
                if dt >= 0.2, done >= last.done {
                    let instant = Double(done - last.done) / dt
                    speed = speed == 0 ? instant : speed * 0.75 + instant * 0.25
                    lastSample = (now, done)
                }
            } else {
                lastSample = (now, done)
                firstSample = (now, done)
            }
            self.done = done
            self.total = total
            StatusCenter.shared.changed()
        }

        /// Seconds left at the current speed (nil while that can't be told).
        var remaining: TimeInterval? {
            guard state == .running, speed > 0, total > done else { return nil }
            return Double(total - done) / speed
        }

        var elapsed: TimeInterval { (ended ?? Date()).timeIntervalSince(started) }

        /// Bytes a second over the whole run, counting only what moved in it (nil for a run too
        /// short to say).
        var averageSpeed: Double? {
            guard let first = firstSample, let end = ended ?? lastSample?.time else { return nil }
            let seconds = end.timeIntervalSince(first.time)
            guard seconds >= 2, done > first.done else { return nil }
            return Double(done - first.done) / seconds
        }

        @MainActor func succeed() { state = .done; ended = Date(); if total > 0 { done = total }; StatusCenter.shared.changed() }
        @MainActor func cancelled() { state = .cancelled; ended = Date(); StatusCenter.shared.changed() }
        @MainActor func fail(_ message: String) {
            ended = Date()
            state = .failed(message)
            StatusCenter.shared.unseenFailure = true
            StatusCenter.shared.changed()
        }
    }

    private(set) var cards: [Card] = []
    /// A failure nobody has looked at yet: the button shows ⚠ until the Status Center opens.
    var unseenFailure = false

    var isBusy: Bool { cards.contains { $0.state == .running } }

    /// How far all running operations are, together (nil: nothing measurable runs).
    var overallProgress: Double? {
        let running = cards.filter { $0.state == .running && $0.total > 0 }
        let total = running.reduce(UInt64(0)) { $0 + $1.total }
        guard total > 0 else { return nil }
        return Double(running.reduce(UInt64(0)) { $0 + $1.done }) / Double(total)
    }

    static func duration(_ t: TimeInterval) -> String {
        let f = DateComponentsFormatter()
        f.unitsStyle = .abbreviated
        f.allowedUnits = t >= 3600 ? [.hour, .minute] : (t >= 60 ? [.minute, .second] : [.second])
        f.maximumUnitCount = 2
        return f.string(from: max(t, 1)) ?? ""
    }

    func add(title: String, folder: URL?, cancel: (() -> Void)?) -> Card {
        let c = Card(title: title, folder: folder, cancel: cancel)
        cards.insert(c, at: 0)
        changed()
        return c
    }

    func failed(title: String, message: String) {
        add(title: title, folder: nil, cancel: nil).fail(message)
    }

    /// Development aid (RF_DEMO_STATUS): a running card that moves, and a finished one.
    static func demo() {
        let done = shared.add(title: "Receiving with Ferry: Studio PC", folder: FileManager.default.homeDirectoryForCurrentUser, cancel: nil)
        done.update(done: 0, total: 7_000_000_000)
        done.succeed()
        let running = shared.add(title: "Sending with Ferry: Studio PC", folder: nil, cancel: {})
        var sent: UInt64 = 0
        Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { t in
            MainActor.assumeIsolated {
                sent = min(sent + 12_000_000, 1_000_000_000)
                running.update(done: sent, total: 1_000_000_000)
                if sent >= 1_000_000_000 { t.invalidate() }
            }
        }
    }

    func clearFinished() {
        cards.removeAll { $0.state != .running }
        changed()
    }

    func changed() {
        NotificationCenter.default.post(name: .rfStatusCenterChanged, object: nil)
    }
}

/// The Status Center's popover: the cards, newest first.
@MainActor
final class StatusCenterView: FlippedView {
    private var rows: [StatusCardView] = []
    private let clear = PlainLinkButton(String(localized: "Clear finished"))
    var onResize: ((NSSize) -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        clear.action = { StatusCenter.shared.clearFinished() }
        addSubview(clear)
        NotificationCenter.default.addObserver(self, selector: #selector(reload), name: .rfStatusCenterChanged, object: nil)
        reload()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc func reload() {
        let cards = StatusCenter.shared.cards
        if rows.map(\.card.id) != cards.map(\.id) {
            rows.forEach { $0.removeFromSuperview() }
            rows = cards.map { StatusCardView($0) }
            rows.forEach(addSubview)
        }
        rows.forEach { $0.needsDisplay = true; $0.needsLayout = true }
        clear.isHidden = !cards.contains { $0.state != .running }
        let h = max(CGFloat(rows.count) * 86 + 16 + 40, 120)
        setFrameSize(NSSize(width: 360, height: min(h, 520)))
        onResize?(frame.size)
        needsLayout = true
        needsDisplay = true
    }

    override func layout() {
        super.layout()
        var y: CGFloat = 40
        for r in rows {
            r.frame = NSRect(x: 12, y: y, width: bounds.width - 24, height: 78)
            y += 86
        }
        let cw = clear.intrinsicContentSize.width
        clear.frame = NSRect(x: bounds.width - 12 - cw, y: 10, width: cw, height: 24)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        Draw.text(String(localized: "Operations"), in: NSRect(x: 16, y: 10, width: 200, height: 24), font: NSFont.systemFont(ofSize: 14, weight: .semibold), color: p.text)
        if rows.isEmpty {
            Draw.text(String(localized: "Nothing is running."), in: NSRect(x: 16, y: 56, width: bounds.width - 32, height: 20), font: Metrics.textSm, color: p.textSecondary)
        }
    }
}

/// One card: the title, a 4 px bar while it runs, what it has done or how it ended, and a
/// button: stop it while it runs, show the folder once received files are in.
@MainActor
final class StatusCardView: HoverView {
    let card: StatusCenter.Card
    private let button = IconButton(icon: "x", size: NSSize(width: 28, height: 28), iconSize: 14)

    init(_ card: StatusCenter.Card) {
        self.card = card
        super.init(frame: .zero)
        button.tint = { $0.textSecondary }
        button.action = { [weak self] _ in self?.act() }
        addSubview(button)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func act() {
        switch card.state {
        case .running: card.cancel?()
        default:
            if let folder = card.folder { NSWorkspace.shared.open(folder) }
        }
    }

    override func layout() {
        super.layout()
        button.frame = NSRect(x: bounds.width - 8 - 28, y: (bounds.height - 28) / 2, width: 28, height: 28)
        switch card.state {
        case .running:
            button.icon = "x"
            button.toolTip = String(localized: "Stop")
            button.isHidden = card.cancel == nil
        default:
            button.icon = "folder-open"
            button.toolTip = String(localized: "Show in folder")
            button.isHidden = card.folder == nil
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: Metrics.radius, yRadius: Metrics.radius)
        p.hover.setFill()
        path.fill()
        p.border.setStroke()
        path.stroke()
        let textW = bounds.width - 12 - 44
        Draw.text(card.title, in: NSRect(x: 12, y: 8, width: textW, height: 20), font: Metrics.textSm, color: p.text)
        var first = "", second = ""
        var color = p.textSecondary
        let speed = card.speed > 0 ? formatSize(bytes: UInt64(card.speed)) + "/s" : ""
        switch card.state {
        case .running:
            let bar = NSRect(x: 12, y: 32, width: textW, height: 4)
            Draw.roundedRect(bar, radius: 2, fill: p.pressed)
            if card.total > 0 {
                let f = Double(card.done) / Double(card.total)
                Draw.roundedRect(NSRect(x: bar.minX, y: bar.minY, width: bar.width * CGFloat(min(max(f, 0), 1)), height: 4), radius: 2, fill: p.accent)
                first = String(localized: "\(formatSize(bytes: card.done)) of \(formatSize(bytes: card.total))") + " · \(Int(f * 100))%"
                // "48.3 MB/s · about 2 min left"
                var parts: [String] = []
                if !speed.isEmpty { parts.append(speed) }
                if let left = card.remaining { parts.append(String(localized: "about \(StatusCenter.duration(left)) left")) }
                second = parts.joined(separator: " · ")
            } else {
                first = String(localized: "Waiting for the other device…")
            }
        case .done:
            first = card.total > 0 ? String(localized: "Done · \(formatSize(bytes: card.total))") : String(localized: "Done")
            let avg = card.averageSpeed.map { formatSize(bytes: UInt64($0)) + "/s" } ?? ""
            second = String(localized: "in \(StatusCenter.duration(card.elapsed))") + (avg.isEmpty ? "" : " · " + String(localized: "\(avg) on average"))
        case .cancelled:
            first = String(localized: "Stopped")
            if card.total > 0 { second = String(localized: "\(formatSize(bytes: card.done)) of \(formatSize(bytes: card.total)) had arrived") }
        case .failed(let message):
            first = message
            color = p.danger
            if card.total > 0 { second = String(localized: "\(formatSize(bytes: card.done)) of \(formatSize(bytes: card.total)) had arrived") }
        }
        let y0: CGFloat = card.state == .running ? 42 : 32
        Draw.text(first, in: NSRect(x: 12, y: y0, width: textW, height: 16), font: Metrics.textXs, color: color)
        if !second.isEmpty {
            Draw.text(second, in: NSRect(x: 12, y: y0 + 16, width: textW, height: 16), font: Metrics.textXs, color: p.textSecondary)
        }
    }
}

/// Ferry's questions (an offer of files, a pairing code), one at a time in a small window at
/// the bottom right of the screen (ferry.rs: 380×176, 16 from the edges, no title bar, never
/// taking the focus). More waiting show as "+N". A question goes away after 185 s.
@MainActor
final class FerryQuestions {
    static let shared = FerryQuestions()

    enum Question {
        case offer(FerryOffer)
        case pairing(peer: String, name: String, code: String)
    }

    private struct Item {
        let id = UUID()
        let question: Question
        /// Shown in the window (asked there, or moved there from its notification).
        var inWindow: Bool
    }

    private var queue: [Item] = []
    private var panel: NSPanel?
    private let view = FerryQuestionView()

    /// Asks as a notification with the two answers as its buttons, as the Windows app asks in
    /// a toast; where notifications are not allowed, in the window.
    func ask(_ q: Question) {
        let viaNotification = FerryNotifications.shared.allowed
        let item = Item(question: q, inWindow: !viaNotification)
        queue.append(item)
        if viaNotification { FerryNotifications.shared.ask(q, id: item.id) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 185) { [weak self] in
            MainActor.assumeIsolated { self?.remove(item.id) }
        }
        sync()
    }

    /// Answered on its notification.
    func answer(_ id: UUID, _ yes: Bool) {
        guard let item = queue.first(where: { $0.id == id }) else { return }
        reply(item.question, yes)
        remove(id)
    }

    /// The notification's body was clicked: the question moves into the window.
    func moveToWindow(_ id: UUID) {
        guard let i = queue.firstIndex(where: { $0.id == id }) else { return }
        queue[i].inWindow = true
        sync()
    }

    private func reply(_ q: Question, _ yes: Bool) {
        switch q {
        case .offer(let o): yes ? FerryService.shared.accept(o) : FerryService.shared.reject(o)
        case .pairing(let peer, _, _): FerryService.shared.confirmPairing(peer, yes)
        }
    }

    func drop(pairingWith peer: String) {
        for item in queue {
            if case .pairing(let p, _, _) = item.question, p == peer { remove(item.id) }
        }
    }

    private func remove(_ id: UUID) {
        queue.removeAll { $0.id == id }
        FerryNotifications.shared.withdraw(id)
        sync()
    }

    private func answerInWindow(_ yes: Bool) {
        guard let first = queue.first(where: \.inWindow) else { return }
        reply(first.question, yes)
        remove(first.id)
    }

    private func sync() {
        let shown = queue.filter(\.inWindow)
        guard let first = shown.first else {
            panel?.orderOut(nil)
            return
        }
        let panel = self.panel ?? makePanel()
        self.panel = panel
        view.show(first.question, more: shown.count - 1)
        // at least 380×176, taller when the text needs it: it grows upwards from the corner
        let height = max(176, view.neededHeight(width: 380))
        if let screen = NSScreen.main {
            let v = screen.visibleFrame
            panel.setFrame(NSRect(x: v.maxX - 380 - 16, y: v.minY + 16, width: 380, height: height), display: true)
        }
        panel.orderFrontRegardless()
    }

    private func makePanel() -> NSPanel {
        let p = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 380, height: 176), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
            defer: false)
        p.isFloatingPanel = true
        p.level = .floating
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hasShadow = true
        p.isMovable = false
        let backdrop: NSView
        if AppSettings.isGlass {
            let g = NSGlassEffectView()
            g.cornerRadius = 16
            backdrop = g
        } else {
            let v = NSVisualEffectView()
            v.material = .popover
            v.state = .active
            v.wantsLayer = true
            v.layer?.cornerRadius = 16
            v.layer?.masksToBounds = true
            backdrop = v
        }
        backdrop.frame = NSRect(x: 0, y: 0, width: 380, height: 176)
        backdrop.autoresizingMask = [.width, .height]
        view.frame = backdrop.bounds
        view.autoresizingMask = [.width, .height]
        backdrop.addSubview(view)
        p.contentView = backdrop
        view.onAnswer = { [weak self] yes in self?.answerInWindow(yes) }
        return p
    }
}

/// What the question window shows (ferry.rs `QuestionView`): the Ferry header, the title, a
/// line, for a pairing the code (24 pt bold, "123 456"), and two buttons.
@MainActor
final class FerryQuestionView: FlippedView {
    var onAnswer: ((Bool) -> Void)?
    private var question: FerryQuestions.Question?
    private var more = 0
    private let no = PlainButton("", action: nil)
    private let yes = PrimaryButton("")

    override init(frame: NSRect) {
        super.init(frame: frame)
        no.action = { [weak self] in self?.onAnswer?(false) }
        yes.action = { [weak self] in self?.onAnswer?(true) }
        addSubview(no)
        addSubview(yes)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show(_ q: FerryQuestions.Question, more: Int) {
        question = q
        self.more = more
        switch q {
        case .offer:
            no.title = String(localized: "Deny")
            yes.title = String(localized: "Accept")
        case .pairing:
            no.title = String(localized: "Cancel")
            yes.title = String(localized: "Pair")
        }
        needsLayout = true
        needsDisplay = true
    }

    /// The question's text as laid out at `width`.
    private func lines(width: CGFloat) -> (title: String, line: String, code: String?) {
        guard let question else { return ("", "", nil) }
        switch question {
        case .offer(let o):
            let what: String
            switch o.names.count {
            case 0: what = o.fileCount == 1 ? String(localized: "1 item") : String(localized: "\(Int(o.fileCount)) items")
            case 1: what = o.names[0]
            default: what = String(localized: "\(o.names[0]) and \(o.names.count - 1) items")
            }
            let line = "\(what) · \(formatSize(bytes: o.totalBytes))"
                + (o.trusted ? "" : "\n" + String(localized: "Unpaired device: its name can’t be verified"))
            return (String(localized: "\(o.fromName) wants to send you files"), line, nil)
        case .pairing(_, let name, let c):
            return (
                String(localized: "Pair: \(name)"),
                String(localized: "Make sure both devices show the same code. Paired devices can send each other files without asking."),
                c.count == 6 ? "\(c.prefix(3)) \(c.suffix(3))" : c
            )
        }
    }

    private func lineHeight(_ text: String, width: CGFloat) -> CGFloat {
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byWordWrapping
        return ceil((text as NSString).boundingRect(
            with: NSSize(width: width, height: 1000), options: [.usesLineFragmentOrigin],
            attributes: [.font: Metrics.textXs, .paragraphStyle: para]).height)
    }

    /// How tall the window must be to show everything: header, title, the line in full, the
    /// code, the buttons.
    func neededHeight(width: CGFloat) -> CGFloat {
        let t = lines(width: width)
        return 16 + 22 + 24 + lineHeight(t.line, width: width - 32) + (t.code != nil ? 36 : 0) + 12 + 32 + 16
    }

    override func layout() {
        super.layout()
        let ys = yes.intrinsicContentSize, ns = no.intrinsicContentSize
        yes.frame = NSRect(x: bounds.width - 16 - ys.width, y: bounds.height - 16 - 32, width: ys.width, height: 32)
        no.frame = NSRect(x: yes.frame.minX - 8 - ns.width, y: yes.frame.minY, width: ns.width, height: 32)
        yes.needsDisplay = true
        no.needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let question else { return }
        let p = Palette.of(self)
        var y: CGFloat = 16
        Draw.icon("send", in: NSRect(x: 16, y: y, width: 16, height: 18), size: 16, color: p.accent)
        Draw.text("Ferry", in: NSRect(x: 16 + 16 + 8, y: y, width: 100, height: 18), font: Metrics.textXs, color: p.textSecondary)
        if more > 0 {
            Draw.text("+\(more)", in: NSRect(x: bounds.width - 16 - 60, y: y, width: 60, height: 18), font: Metrics.textXs, color: p.textSecondary, align: .right)
        }
        y += 18 + 4
        let (title, line, code) = lines(width: bounds.width)
        Draw.text(title, in: NSRect(x: 16, y: y, width: bounds.width - 32, height: 20), font: NSFont.systemFont(ofSize: 14, weight: .semibold), color: p.text)
        y += 20 + 4
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byWordWrapping
        let lh = lineHeight(line, width: bounds.width - 32)
        (line as NSString).draw(
            with: NSRect(x: 16, y: y, width: bounds.width - 32, height: lh), options: [.usesLineFragmentOrigin],
            attributes: [.font: Metrics.textXs, .foregroundColor: p.textSecondary, .paragraphStyle: para])
        if let code {
            Draw.text(code, in: NSRect(x: 16, y: y + lh + 4, width: bounds.width - 32, height: 32), font: NSFont.monospacedDigitSystemFont(ofSize: 24, weight: .bold), color: p.text)
        }
    }
}

/// The primary button (ferry.rs `primary`): 32 high, `px_4`, radius 6, accent fill.
@MainActor
final class PrimaryButton: HoverView {
    var title: String { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var action: (() -> Void)?

    init(_ title: String) {
        self.title = title
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }
    override var intrinsicContentSize: NSSize { NSSize(width: Draw.textWidth(title, font: Metrics.textSm) + 32, height: 32) }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        Draw.roundedRect(bounds, radius: Metrics.radiusSmall, fill: p.accent.withAlphaComponent(hovered ? 0.9 : 1))
        Draw.text(title, in: bounds, font: Metrics.textSm, color: p.accentText, align: .center)
    }

    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action?() }
    }
}

/// The "Send with Ferry" submenu (ferry.rs `send_menu`): the devices nearby (trusted ones with
/// a shield), or "No devices nearby"; then, on the Mac, AirDrop for Apple devices without
/// rustyfiles; then "Ferry settings…" (or "Turn on Ferry…" while it is off).
@MainActor
enum FerryMenu {
    static func make(for urls: [URL]) -> NSMenuItem {
        let item = NSMenuItem(title: String(localized: "Send with Ferry"), action: nil, keyEquivalent: "")
        item.image = Icons.menu("send")
        let sub = NSMenu()
        sub.autoenablesItems = false
        let running = FerryService.shared.isRunning
        if running {
            let peers = FerryService.shared.peers
            for peer in peers {
                let i = ClosureMenuItem(title: peer.name) { FerryService.shared.send(urls, to: peer) }
                i.image = Icons.menu(peer.trusted ? "shield-check" : "monitor")
                sub.addItem(i)
            }
            if peers.isEmpty {
                let none = NSMenuItem(title: String(localized: "No devices nearby"), action: nil, keyEquivalent: "")
                none.image = Icons.menu("wifi")
                none.isEnabled = false
                sub.addItem(none)
            }
            sub.addItem(.separator())
        }
        // Apple devices without rustyfiles: the system's own AirDrop window picks the device
        if let airdrop = NSSharingService(named: .sendViaAirDrop), airdrop.canPerform(withItems: urls) {
            let i = ClosureMenuItem(title: "AirDrop…") { airdrop.perform(withItems: urls) }
            i.image = airdrop.image
            sub.addItem(i)
            sub.addItem(.separator())
        }
        let settings = ClosureMenuItem(title: running ? String(localized: "Ferry settings…") : String(localized: "Turn on Ferry…")) {
            (NSApp.keyWindow?.windowController as? BrowserWindowController)?.openSettings(page: .ferry)
        }
        settings.image = Icons.menu("settings")
        sub.addItem(settings)
        item.submenu = sub
        return item
    }
}
