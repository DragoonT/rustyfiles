import AppKit
import UserNotifications

/// Ferry's notifications (the Windows app's toasts): a question with its two answers as
/// buttons (clicking the notification itself moves the question into Ferry's window), and
/// "<name> sent you files" when files arrive (clicking it shows them).
@MainActor
final class FerryNotifications: NSObject, @preconcurrency UNUserNotificationCenterDelegate {
    static let shared = FerryNotifications()

    /// Notifications may be shown (asked for once, when Ferry first starts).
    private(set) var allowed = false
    private var isSetUp = false
    private let center = UNUserNotificationCenter.current()

    private enum ID {
        static let offer = "ferry.offer", pairing = "ferry.pairing", arrived = "ferry.arrived"
        static let yes = "yes", no = "no"
    }

    func setUp() {
        guard !isSetUp else { return }
        isSetUp = true
        center.delegate = self
        let accept = UNNotificationAction(identifier: ID.yes, title: String(localized: "Accept"), options: [])
        let deny = UNNotificationAction(identifier: ID.no, title: String(localized: "Deny"), options: [.destructive])
        let pair = UNNotificationAction(identifier: ID.yes, title: String(localized: "Pair"), options: [])
        let cancel = UNNotificationAction(identifier: ID.no, title: String(localized: "Cancel"), options: [])
        center.setNotificationCategories([
            UNNotificationCategory(identifier: ID.offer, actions: [accept, deny], intentIdentifiers: [], options: [.customDismissAction]),
            UNNotificationCategory(identifier: ID.pairing, actions: [pair, cancel], intentIdentifiers: [], options: [.customDismissAction]),
            UNNotificationCategory(identifier: ID.arrived, actions: [], intentIdentifiers: [], options: []),
        ])
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            Task { @MainActor in FerryNotifications.shared.allowed = granted }
        }
    }

    func ask(_ q: FerryQuestions.Question, id: UUID) {
        let content = UNMutableNotificationContent()
        // the AirDrop sound is played by FerrySound; a second sound would be noise
        content.userInfo = ["question": id.uuidString]
        // the question stays until it is answered (an alert, where the user lets it be one)
        content.interruptionLevel = .timeSensitive
        switch q {
        case .offer(let o):
            content.categoryIdentifier = ID.offer
            content.title = String(localized: "\(o.fromName) wants to send you files")
            let what = o.names.count == 1 ? o.names[0] : (o.fileCount == 1 ? String(localized: "1 item") : String(localized: "\(Int(o.fileCount)) items"))
            content.body = "\(what) · \(formatSize(bytes: o.totalBytes))"
                + (o.trusted ? "" : "\n" + String(localized: "Unpaired device: its name can’t be verified"))
        case .pairing(_, let name, let code):
            content.categoryIdentifier = ID.pairing
            content.title = String(localized: "Pair: \(name)")
            let grouped = code.count == 6 ? "\(code.prefix(3)) \(code.suffix(3))" : code
            content.body = String(localized: "Code \(grouped). Make sure both devices show the same code.")
        }
        center.add(UNNotificationRequest(identifier: id.uuidString, content: content, trigger: nil))
    }

    func withdraw(_ id: UUID) {
        center.removeDeliveredNotifications(withIdentifiers: [id.uuidString])
        center.removePendingNotificationRequests(withIdentifiers: [id.uuidString])
    }

    func arrived(from name: String, paths: [String]) {
        guard allowed else { return }
        let content = UNMutableNotificationContent()
        content.categoryIdentifier = ID.arrived
        content.title = String(localized: "\(name) sent you files")
        content.body = paths.map { ($0 as NSString).lastPathComponent }.joined(separator: ", ")
        content.userInfo = ["paths": paths]
        center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }

    // MARK: UNUserNotificationCenterDelegate

    /// Shown even while rustyfiles is in front (the question matters more than the app's focus).
    func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse, withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let info = response.notification.request.content.userInfo
        if let raw = info["question"] as? String, let id = UUID(uuidString: raw) {
            switch response.actionIdentifier {
            case ID.yes: FerryQuestions.shared.answer(id, true)
            case ID.no: FerryQuestions.shared.answer(id, false)
            case UNNotificationDismissActionIdentifier: break
            default: FerryQuestions.shared.moveToWindow(id)
            }
        } else if let paths = info["paths"] as? [String], let first = paths.first {
            // show what arrived: its folder in a rustyfiles window, the first item selected
            let url = URL(fileURLWithPath: first)
            AppDelegate.shared.application(NSApp, open: [url.deletingLastPathComponent().appendingPathComponent(url.lastPathComponent)])
            NSApp.activate()
        }
        completionHandler()
    }
}
