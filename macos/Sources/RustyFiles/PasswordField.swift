import AppKit

/// A password box with an eye button that shows what was typed (#204 on Windows). While hidden,
/// the text cannot be copied or cut out of it (NSSecureTextField's own rule); shown, it is an
/// ordinary field.
@MainActor
final class PasswordField: NSView {
    private let secure = NSSecureTextField(string: "")
    private let plain = NSTextField(string: "")
    private let eye = IconButton(icon: "eye", size: NSSize(width: 22, height: 22), iconSize: 14, tooltip: String(localized: "Show password"))
    private var shown = false

    var stringValue: String {
        get { shown ? plain.stringValue : secure.stringValue }
        set { secure.stringValue = newValue; plain.stringValue = newValue }
    }

    var placeholderString: String? {
        get { secure.placeholderString }
        set { secure.placeholderString = newValue; plain.placeholderString = newValue }
    }

    /// The field that takes the keyboard (for a window's initial first responder).
    var field: NSTextField { shown ? plain : secure }

    override init(frame: NSRect) {
        super.init(frame: frame)
        plain.isHidden = true
        eye.tint = { $0.textSecondary }
        eye.action = { [weak self] _ in self?.toggle() }
        [secure, plain, eye].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 22) }

    override func layout() {
        super.layout()
        // the eye beside the field, so text never runs under it
        let f = NSRect(x: 0, y: 0, width: bounds.width - 26, height: bounds.height)
        secure.frame = f
        plain.frame = f
        eye.frame = NSRect(x: bounds.width - 22, y: (bounds.height - 22) / 2, width: 22, height: 22)
    }

    private func toggle() {
        let text = stringValue
        let hadFocus = window?.firstResponder is NSTextView && (window?.firstResponder as? NSTextView)?.delegate === field
        shown.toggle()
        secure.stringValue = text
        plain.stringValue = text
        secure.isHidden = shown
        plain.isHidden = !shown
        eye.icon = shown ? "eye-off" : "eye"
        eye.toolTip = shown ? String(localized: "Hide password") : String(localized: "Show password")
        if hadFocus || window?.firstResponder == nil { window?.makeFirstResponder(field) }
        // the eye stays above the field shown
        addSubview(eye, positioned: .above, relativeTo: nil)
    }
}
