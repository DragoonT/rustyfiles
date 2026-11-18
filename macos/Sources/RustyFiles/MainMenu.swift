import AppKit

/// The menu bar, built in code (no nib). Items with a `nil` target go down the responder
/// chain: the file list, then its window's controller, then the app delegate. Shortcuts are
/// the Mac's own where it has one, the Windows app's otherwise (F2, Backspace in the list).
@MainActor
enum MainMenu {
    /// `install`: also hand the Window and Help menus to AppKit (false: a copy to read).
    static func make(install: Bool = true) -> NSMenu {
        let bar = NSMenu()
        bar.addItem(submenu(appMenu()))
        bar.addItem(submenu(fileMenu()))
        bar.addItem(submenu(editMenu()))
        bar.addItem(submenu(viewMenu()))
        bar.addItem(submenu(goMenu()))
        let window = windowMenu()
        bar.addItem(submenu(window))
        let help = NSMenu(title: String(localized: "Help"))
        bar.addItem(submenu(help))
        if install {
            NSApp.windowsMenu = window
            NSApp.helpMenu = help
        }
        // Settings › Keyboard shortcuts
        Shortcuts.apply(bar)
        return bar
    }

    private static func submenu(_ menu: NSMenu) -> NSMenuItem {
        let item = NSMenuItem(title: menu.title, action: nil, keyEquivalent: "")
        item.submenu = menu
        return item
    }

    private static func item(
        _ title: String, _ action: Selector?, _ key: String = "", _ mods: NSEvent.ModifierFlags = .command
    ) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: action, keyEquivalent: key)
        i.keyEquivalentModifierMask = mods
        return i
    }

    private static func key(_ code: Int) -> String { String(Character(UnicodeScalar(code)!)) }

    private typealias W = BrowserWindowController

    private static func appMenu() -> NSMenu {
        let m = NSMenu(title: "rustyfiles")
        m.addItem(item(String(localized: "About rustyfiles"), #selector(NSApplication.orderFrontStandardAboutPanel(_:))))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Settings…"), #selector(W.openSettings(_:)), ","))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Hide rustyfiles"), #selector(NSApplication.hide(_:)), "h"))
        m.addItem(item(String(localized: "Hide Others"), #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]))
        m.addItem(item(String(localized: "Show All"), #selector(NSApplication.unhideAllApplications(_:))))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Quit rustyfiles"), #selector(NSApplication.terminate(_:)), "q"))
        return m
    }

    private static func fileMenu() -> NSMenu {
        let m = NSMenu(title: String(localized: "File"))
        m.addItem(item(String(localized: "New Window"), #selector(AppDelegate.newWindow(_:)), "n"))
        m.addItem(item(String(localized: "New Tab"), #selector(W.newTab(_:)), "t"))
        m.addItem(item(String(localized: "New Folder"), #selector(W.newFolder(_:)), "n", [.command, .shift]))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Open"), #selector(W.openSelection(_:)), key(NSDownArrowFunctionKey)))
        m.addItem(item(String(localized: "Quick Look"), #selector(W.toggleQuickLook(_:)), "y"))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Properties"), #selector(W.showProperties(_:)), "i"))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Close Tab"), #selector(W.closeTab(_:)), "w"))
        m.addItem(item(String(localized: "Reopen Closed Tab"), #selector(W.reopenClosedTab(_:)), "t", [.command, .shift]))
        m.addItem(item(String(localized: "Close Window"), #selector(NSWindow.performClose(_:)), "w", [.command, .shift]))
        return m
    }

    private static func editMenu() -> NSMenu {
        let m = NSMenu(title: String(localized: "Edit"))
        m.addItem(item(String(localized: "Undo"), Selector(("undo:")), "z"))
        m.addItem(item(String(localized: "Redo"), Selector(("redo:")), "z", [.command, .shift]))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Cut"), #selector(W.cut(_:)), "x"))
        m.addItem(item(String(localized: "Copy"), #selector(W.copy(_:) as (W) -> (Any?) -> Void), "c"))
        m.addItem(item(String(localized: "Paste"), #selector(W.paste(_:)), "v"))
        m.addItem(item(String(localized: "Select All"), #selector(NSResponder.selectAll(_:)), "a"))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Rename"), #selector(W.rename(_:)), key(NSF2FunctionKey), []))
        m.addItem(item(String(localized: "Move to Trash"), #selector(W.moveToTrash(_:)), key(NSBackspaceCharacter), .command))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Filter"), #selector(W.focusSearch(_:)), "f"))
        m.addItem(item(String(localized: "Go to Path"), #selector(W.focusAddress(_:)), "l"))
        return m
    }

    private static func viewMenu() -> NSMenu {
        let m = NSMenu(title: String(localized: "View"))
        m.addItem(item(String(localized: "Details"), #selector(W.showDetails(_:)), "1"))
        m.addItem(item(String(localized: "Tiles"), #selector(W.showTiles(_:)), "2"))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Show Hidden Files"), #selector(W.toggleHidden(_:)), ".", [.command, .shift]))
        m.addItem(item(String(localized: "Refresh"), #selector(W.refresh(_:)), "r"))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Hide Sidebar"), #selector(W.toggleSidebar(_:)), "s", [.command, .control]))
        m.addItem(item(String(localized: "Show Preview"), #selector(W.togglePreviewPane(_:)), "p", [.command, .shift]))
        return m
    }

    private static func goMenu() -> NSMenu {
        let m = NSMenu(title: String(localized: "Go"))
        m.addItem(item(String(localized: "Back"), #selector(W.goBack(_:)), "["))
        m.addItem(item(String(localized: "Forward"), #selector(W.goForward(_:)), "]"))
        m.addItem(item(String(localized: "Up"), #selector(W.goUp(_:)), key(NSUpArrowFunctionKey)))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Home"), #selector(W.goHome(_:)), "h", [.command, .shift]))
        m.addItem(item(String(localized: "Home Folder"), #selector(W.goHomeFolder(_:)), "h", [.command, .option]))
        m.addItem(item(String(localized: "Desktop"), #selector(W.goDesktop(_:)), "d", [.command, .shift]))
        m.addItem(item(String(localized: "Documents"), #selector(W.goDocuments(_:)), "o", [.command, .shift]))
        m.addItem(item(String(localized: "Downloads"), #selector(W.goDownloads(_:)), "l", [.command, .option]))
        m.addItem(item(String(localized: "Applications"), #selector(W.goApplications(_:)), "a", [.command, .shift]))
        return m
    }

    private static func windowMenu() -> NSMenu {
        let m = NSMenu(title: String(localized: "Window"))
        m.addItem(item(String(localized: "Minimize"), #selector(NSWindow.performMiniaturize(_:)), "m"))
        m.addItem(item(String(localized: "Zoom"), #selector(NSWindow.performZoom(_:))))
        m.addItem(.separator())
        m.addItem(item(String(localized: "Next Tab"), #selector(W.selectNextTab(_:)), "\t", .control))
        m.addItem(item(String(localized: "Previous Tab"), #selector(W.selectPreviousTab(_:)), "\t", [.control, .shift]))
        return m
    }
}
