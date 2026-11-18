import AppKit
import Network

extension Notification.Name {
    /// Ferry's devices, status or visibility changed.
    static let rfFerryChanged = Notification.Name("rustyfiles.ferryChanged")
}

/// Ferry's settings (crates/rf-app/src/settings.rs: ferry_*). Off by default, as on Windows.
@MainActor
enum FerrySettings {
    private static var d: UserDefaults { .standard }

    static var enabled: Bool {
        get { d.bool(forKey: "ferryEnabled") }
        set { d.set(newValue, forKey: "ferryEnabled"); FerryService.shared.restart() }
    }

    static var visibility: FerryVisibility {
        get {
            switch d.string(forKey: "ferryVisibility") {
            case "off": .off
            case "everyone": .everyone
            default: .trusted
            }
        }
        set {
            d.set(newValue == .off ? "off" : newValue == .everyone ? "everyone" : "trusted", forKey: "ferryVisibility")
        }
    }

    /// The name other devices see; empty: the Mac's own name.
    static var name: String {
        get { d.string(forKey: "ferryName") ?? "" }
        set { d.set(newValue, forKey: "ferryName"); FerryService.shared.restart() }
    }

    static var effectiveName: String {
        name.isEmpty ? (Host.current().localizedName ?? String(localized: "Mac")) : name
    }

    /// Where received files go; empty: Downloads.
    static var inbox: String {
        get { d.string(forKey: "ferryInbox") ?? "" }
        set { d.set(newValue, forKey: "ferryInbox"); FerryService.shared.restart() }
    }

    static var effectiveInbox: URL {
        inbox.isEmpty ? (FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser) : URL(fileURLWithPath: inbox)
    }

    static var autoAcceptTrusted: Bool {
        get { d.object(forKey: "ferryAutoAccept") == nil ? true : d.bool(forKey: "ferryAutoAccept") }
        set { d.set(newValue, forKey: "ferryAutoAccept"); FerryService.shared.restart() }
    }

    /// "Everyone" lasts this long, then falls back to trusted devices only.
    static let everyoneMinutes: UInt32 = 10
}

/// The sound of files on their way in: AirDrop's own invitation sound, played from where macOS
/// keeps it (not shipped with the app), else the system's "Glass".
@MainActor
enum FerrySound {
    private static let airdrop = "/System/Library/PrivateFrameworks/Sharing.framework/Versions/A/Resources/airdrop_invite.caf"
    private static let sound: NSSound? = NSSound(contentsOfFile: airdrop, byReference: true) ?? NSSound(named: "Glass")

    static func play() {
        guard let sound else { return }
        sound.stop()
        sound.play()
    }
}

/// Ferry for the whole app: started while the setting is on, its events turned into the
/// question window, Status Center cards and the Settings page.
@MainActor
final class FerryService {
    static let shared = FerryService()

    private(set) var ferry: Ferry?
    private(set) var peers: [FerryPeer] = []
    private(set) var trusted: [FerryPeer] = []
    /// Why devices can't be found, if they can't (shown on the Settings page).
    private(set) var problem: String?
    private var localNetworkDenied = false
    private var browser: NWBrowser?
    /// Transfers and their Status Center cards.
    private var cards: [UInt64: StatusCenter.Card] = [:]
    private var activity: NSObjectProtocol?

    var isRunning: Bool { ferry != nil }

    func startIfEnabled() {
        guard FerrySettings.enabled, ferry == nil else { return }
        FerryNotifications.shared.setUp()
        // Everyone does not outlive the app, as on Windows
        if FerrySettings.visibility == .everyone { FerrySettings.visibility = .trusted }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("rustyfiles/nearby", isDirectory: true)
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        let config = FerryConfig(
            name: FerrySettings.effectiveName, os: "macos", stateDir: support.path, inbox: FerrySettings.effectiveInbox.path,
            autoAcceptTrusted: FerrySettings.autoAcceptTrusted, port: 0, discovery: true,
            everyoneMinutes: FerrySettings.everyoneMinutes, localOnly: false)
        do {
            let f = try Ferry.start(config: config, listener: FerryEvents())
            ferry = f
            f.setVisibility(visibility: FerrySettings.visibility)
            problem = nil
            refreshLists()
            watchLocalNetwork()
            LinkMonitor.shared.start()
            let (kind, mbps) = LinkMonitor.shared.current()
            f.setLink(kind: kind, mbps: mbps)
            becomeVisibleForPage()
        } catch {
            problem = error.localizedDescription
        }
        changed()
    }

    func stop() {
        LinkMonitor.shared.stop()
        ferry?.stop()
        ferry = nil
        browser?.cancel()
        browser = nil
        peers = []
        trusted = []
        problem = nil
        changed()
    }

    /// The setting changed: start, stop, or start again with the new name or folder.
    func restart() {
        stop()
        startIfEnabled()
    }

    func setVisibility(_ v: FerryVisibility) {
        // a choice made by hand stays: leaving the page does not undo it
        restoreVisibility = nil
        apply(v)
    }

    private func apply(_ v: FerryVisibility) {
        FerrySettings.visibility = v
        ferry?.setVisibility(visibility: v)
        changed()
    }

    /// Settings › Ferry pages on screen. While one is, this Mac can be seen by everyone, as a
    /// Mac is while Finder's AirDrop window is open; leaving puts back what was chosen before
    /// (the ten-minute limit still applies).
    private var pageViewers: Set<ObjectIdentifier> = []
    private var restoreVisibility: FerryVisibility?

    func settingsPage(_ viewer: AnyObject, isShowing: Bool) {
        let id = ObjectIdentifier(viewer)
        let wasEmpty = pageViewers.isEmpty
        if isShowing { pageViewers.insert(id) } else { pageViewers.remove(id) }
        if wasEmpty && !pageViewers.isEmpty {
            becomeVisibleForPage()
        } else if !wasEmpty && pageViewers.isEmpty, let previous = restoreVisibility {
            restoreVisibility = nil
            // only if it is still the page's doing (not changed since, nor timed out)
            if FerrySettings.visibility == .everyone { apply(previous) }
        }
    }

    private func becomeVisibleForPage() {
        guard isRunning, !pageViewers.isEmpty, FerrySettings.visibility != .everyone else { return }
        restoreVisibility = FerrySettings.visibility
        apply(.everyone)
    }

    private func refreshLists() {
        guard let ferry else { return }
        peers = ferry.peers()
        trusted = ferry.trusted()
    }

    private func changed() {
        NotificationCenter.default.post(name: .rfFerryChanged, object: nil)
    }

    // MARK: actions

    func send(_ urls: [URL], to peer: FerryPeer) {
        guard let ferry else { return }
        do {
            let t = try ferry.send(peerId: peer.id, paths: urls.map(\.path))
            track(t, title: String(localized: "Sending with Ferry: \(peer.name)"), folder: nil)
        } catch {
            StatusCenter.shared.failed(title: String(localized: "Sending with Ferry: \(peer.name)"), message: error.localizedDescription)
        }
    }

    func refresh() {
        ferry?.refresh()
        peers = []
        changed()
    }

    /// Speed tests: the last result per device, and the ones running now.
    private(set) var speeds: [String: FerrySpeed] = [:]
    private(set) var measuring: Set<String> = []

    func measureSpeed(_ peer: FerryPeer) {
        guard let ferry, !measuring.contains(peer.id) else { return }
        do {
            _ = try ferry.measureSpeed(peerId: peer.id)
            measuring.insert(peer.id)
        } catch {
            speeds[peer.id] = FerrySpeed(ok: false, upBps: 0, downBps: 0, rttMs: 0, error: error.localizedDescription)
        }
        changed()
    }

    fileprivate func speedMeasured(_ peer: String, _ r: FerrySpeed) {
        measuring.remove(peer)
        speeds[peer] = r
        refreshLists()
        changed()
    }

    func pair(_ peer: FerryPeer) {
        do { try ferry?.pair(peerId: peer.id) } catch { problem = error.localizedDescription; changed() }
    }

    func forget(_ peer: FerryPeer) {
        try? ferry?.forget(peerId: peer.id)
        refreshLists()
        changed()
    }

    func accept(_ offer: FerryOffer) {
        senders[offer.transfer] = offer.fromName
        let inbox = FerrySettings.effectiveInbox
        ferry?.accept(offerId: offer.id, destDir: inbox.path)
        track(offer.transfer, title: String(localized: "Receiving with Ferry: \(offer.fromName)"), folder: inbox)
    }

    func reject(_ offer: FerryOffer) { ferry?.reject(offerId: offer.id) }

    func confirmPairing(_ peer: String, _ ok: Bool) { ferry?.confirmPairing(peer: peer, ok: ok) }

    func name(of peerID: String) -> String {
        (peers + trusted).first { $0.id == peerID }?.name ?? peerID
    }

    // MARK: transfers

    private func track(_ transfer: UInt64, title: String, folder: URL?) {
        let card = StatusCenter.shared.add(title: title, folder: folder) { [weak self] in self?.ferry?.cancel(transfer: transfer) }
        cards[transfer] = card
        // a lid closed or App Nap must not stall a transfer more than it has to
        if activity == nil {
            activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .idleSystemSleepDisabled], reason: "Ferry transfer")
        }
    }

    private func finishedTracking(_ transfer: UInt64) {
        cards[transfer] = nil
        if cards.isEmpty, let a = activity {
            ProcessInfo.processInfo.endActivity(a)
            activity = nil
        }
    }

    /// Received files carry the quarantine flag, like a download: Gatekeeper checks an app
    /// that came this way before it first runs.
    nonisolated private static func quarantine(_ paths: [String]) {
        let value = String(format: "0081;%08x;rustyfiles;", Int(Date().timeIntervalSince1970))
        func mark(_ path: String) {
            _ = value.withCString { setxattr(path, "com.apple.quarantine", $0, strlen($0), 0, XATTR_NOFOLLOW) }
        }
        for path in paths {
            mark(path)
            // everything inside a received folder too, as the Windows app marks each file
            guard let walker = FileManager.default.enumerator(atPath: path) else { continue }
            for case let rel as String in walker { mark((path as NSString).appendingPathComponent(rel)) }
        }
    }

    // MARK: events (on the main thread)

    fileprivate func peersChanged(_ list: [FerryPeer]) {
        peers = list
        trusted = ferry?.trusted() ?? trusted
        changed()
    }

    fileprivate func offer(_ o: FerryOffer) {
        FerrySound.play()
        FerryQuestions.shared.ask(.offer(o))
    }

    /// Who is sending what is being received (for "<name> sent you files").
    private var senders: [UInt64: String] = [:]

    fileprivate func receiving(_ o: FerryOffer) {
        FerrySound.play()
        senders[o.transfer] = o.fromName
        track(o.transfer, title: String(localized: "Receiving with Ferry: \(o.fromName)"), folder: FerrySettings.effectiveInbox)
    }

    fileprivate func progress(_ transfer: UInt64, _ done: UInt64, _ total: UInt64) {
        cards[transfer]?.update(done: done, total: total)
    }

    fileprivate func finished(_ transfer: UInt64, _ r: FerryResult) {
        if !r.paths.isEmpty {
            let paths = r.paths
            DispatchQueue.global(qos: .utility).async { Self.quarantine(paths) }
            if r.ok, let from = senders[transfer] { FerryNotifications.shared.arrived(from: from, paths: paths) }
        }
        senders[transfer] = nil
        if let card = cards[transfer] {
            if r.ok {
                card.succeed()
            } else if r.cancelled {
                card.cancelled()
            } else {
                card.fail(Self.failureText(r.error))
            }
        }
        finishedTracking(transfer)
    }

    fileprivate func pairingCode(_ peer: String, _ code: String) {
        FerryQuestions.shared.ask(.pairing(peer: peer, name: name(of: peer), code: code))
    }

    fileprivate func paired(_ peer: String, _ ok: Bool) {
        FerryQuestions.shared.drop(pairingWith: peer)
        refreshLists()
        changed()
    }

    fileprivate func visibilityChanged(_ v: FerryVisibility) {
        restoreVisibility = nil
        FerrySettings.visibility = v
        changed()
    }

    fileprivate func status(_ s: FerryStatus) {
        switch s {
        case .ready: problem = localNetworkDenied ? Self.localNetworkText : nil
        case .discoveryUnavailable(let reason): problem = reason
        }
        changed()
    }

    /// The crate's error, as the Windows app words it (ferry.rs `failure_text`).
    static func failureText(_ error: String?) -> String {
        guard let e = error, !e.isEmpty else { return String(localized: "The transfer stopped partway.") }
        // the reasons' words are fixed by the protocol (docs/nearby-protocol.md)
        let lower = e.lowercased()
        if lower.contains("only takes files from its trusted devices") {
            return String(localized: "That device only takes files from its trusted devices. Pair with it first.")
        }
        if lower.contains("declined") { return String(localized: "The other device declined.") }
        if lower.contains("busy with other offers") { return String(localized: "You already have an offer waiting on that device.") }
        if lower.contains("busy with another speed test") { return String(localized: "That device is busy with another speed test. Try again in a few seconds.") }
        if lower.contains("arrived damaged") {
            return String(localized: "A file arrived damaged. Send again: the rest picks up where it stopped.")
        }
        if lower.contains("timed out") || lower.contains("did not answer") || lower.contains("timeout") {
            return String(localized: "The device did not answer.")
        }
        return e
    }

    // MARK: Local Network permission

    private static let localNetworkText = String(
        localized: "rustyfiles may not use the local network. Allow it in System Settings › Privacy & Security › Local Network.")

    /// A refused Local Network permission drops multicast silently, which Rust can't see; a
    /// Bonjour browser for Ferry's own service is told (and is itself what asks the first time).
    private func watchLocalNetwork() {
        let b = NWBrowser(for: .bonjour(type: "_rustyfiles._udp", domain: nil), using: NWParameters())
        b.stateUpdateHandler = { state in
            Task { @MainActor in
                let svc = FerryService.shared
                switch state {
                case .waiting(let error), .failed(let error):
                    if case .dns(let code) = error, code == DNSServiceErrorType(kDNSServiceErr_PolicyDenied) {
                        svc.localNetworkDenied = true
                        svc.problem = Self.localNetworkText
                        svc.changed()
                    }
                case .ready:
                    if svc.localNetworkDenied {
                        svc.localNetworkDenied = false
                        svc.problem = nil
                        svc.changed()
                    }
                default: break
                }
            }
        }
        b.start(queue: .main)
        browser = b
    }
}

/// Ferry's events, from its own threads onto the main thread.
final class FerryEvents: FerryListener, @unchecked Sendable {
    private func main(_ work: @escaping @MainActor (FerryService) -> Void) {
        DispatchQueue.main.async { MainActor.assumeIsolated { work(FerryService.shared) } }
    }

    func peersChanged(peers: [FerryPeer]) { main { $0.peersChanged(peers) } }
    func offer(offer: FerryOffer) { main { $0.offer(offer) } }
    func receiving(offer: FerryOffer) { main { $0.receiving(offer) } }
    func progress(transfer: UInt64, done: UInt64, total: UInt64) { main { $0.progress(transfer, done, total) } }
    func finished(transfer: UInt64, result: FerryResult) { main { $0.finished(transfer, result) } }
    func pairingCode(peer: String, code: String) { main { $0.pairingCode(peer, code) } }
    func paired(peer: String, ok: Bool) { main { $0.paired(peer, ok) } }
    func visibilityChanged(visibility: FerryVisibility) { main { $0.visibilityChanged(visibility) } }
    func status(status: FerryStatus) { main { $0.status(status) } }
    func speedMeasured(peer: String, result: FerrySpeed) { main { $0.speedMeasured(peer, result) } }
}
