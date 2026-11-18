import CoreWLAN
import Darwin
import Network

/// This Mac's network link, told to Ferry so other devices can guess a transfer's speed before
/// anything has been measured: the kind from the path the system uses, the rate from Wi-Fi's
/// current transmit rate or the wired interface's link speed. Read again when the network
/// changes and every 30 s (Wi-Fi's rate drifts), as the Windows app reads its link.
@MainActor
final class LinkMonitor {
    static let shared = LinkMonitor()

    private let monitor = NWPathMonitor()
    private var timer: Timer?
    private var path: NWPath?
    private var running = false

    func start() {
        guard !running else { return }
        running = true
        monitor.pathUpdateHandler = { path in
            Task { @MainActor in
                LinkMonitor.shared.path = path
                LinkMonitor.shared.report()
            }
        }
        monitor.start(queue: .main)
        timer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { _ in
            MainActor.assumeIsolated { LinkMonitor.shared.report() }
        }
    }

    func stop() {
        guard running else { return }
        running = false
        monitor.cancel()
        timer?.invalidate()
        timer = nil
    }

    /// The link now: its kind and its rate in Mbps (0 when it can't be told).
    func current() -> (FerryLink, UInt32) {
        guard let path, path.status == .satisfied else { return (.unknown, 0) }
        if path.usesInterfaceType(.wifi) {
            let rate = CWWiFiClient.shared().interface()?.transmitRate() ?? 0
            return (.wifi, UInt32(max(rate, 0)))
        }
        if path.usesInterfaceType(.wiredEthernet) {
            let name = path.availableInterfaces.first { $0.type == .wiredEthernet }?.name
            return (.ethernet, name.map(Self.baudrateMbps) ?? 0)
        }
        return (.other, 0)
    }

    private func report() {
        let (kind, mbps) = current()
        FerryService.shared.ferry?.setLink(kind: kind, mbps: mbps)
    }

    /// A wired interface's link speed (`if_data.ifi_baudrate`, bits a second) in Mbps.
    private static func baudrateMbps(_ name: String) -> UInt32 {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return 0 }
        defer { freeifaddrs(list) }
        var p: UnsafeMutablePointer<ifaddrs>? = first
        while let a = p {
            if String(cString: a.pointee.ifa_name) == name, a.pointee.ifa_addr?.pointee.sa_family == UInt8(AF_LINK),
                let data = a.pointee.ifa_data
            {
                let baud = data.assumingMemoryBound(to: if_data.self).pointee.ifi_baudrate
                return UInt32(min(UInt64(baud) / 1_000_000, UInt64(UInt32.max)))
            }
            p = a.pointee.ifa_next
        }
        return 0
    }
}
