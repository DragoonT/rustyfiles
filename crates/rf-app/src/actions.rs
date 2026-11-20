use gpui_kit::*;

gpui_kit::actions!(
    rustyfiles,
    [
        GoBack,
        BackspaceBack,
        GoForward,
        LeaveTree,
        FocusNextPane,
        FocusPrevPane,
        DialogNext,
        DialogPrev,
        GoUp,
        GoHome,
        Refresh,
        OpenSelected,
        SelectNext,
        SelectPrev,
        SelectFirst,
        SelectLast,
        SelectAll,
        ExtendNext,
        ExtendPrev,
        PageDown,
        PageUp,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        ReopenClosedTab,
        FocusAddress,
        FocusSearch,
        ToggleHidden,
        OpenTerminal,
        Properties,
        ClearSelection,
        CopyItems,
        CutItems,
        PasteItems,
        PasteIntoSelected,
        PasteShortcut,
        DeleteItems,
        DeletePermanently,
        DuplicateItems,
        CopyToOtherPane,
        MoveToOtherPane,
        RenameItem,
        NewFolder,
        NewTextFile,
        DownloadFromLink,
        ShowContextMenu,
        CopyPath,
        OpenWith,
        SelectLeft,
        SelectRight,
        ViewDetails,
        ViewGrid,
        TogglePreview,
        ToggleSidebar,
        ToggleDualPane,
        SwitchPane,
        ToggleCommandPalette,
        SearchFolderHistory,
        SearchTabs,
        GoSettings,
        GoRecycleBin,
        ViewColumns,
        ViewCards,
        ViewList,
        Undo,
        MapNetworkDrive,
        RestartApp,
        SaveSession,
        DisconnectNetworkDrive,
        NewWindow,
        ZoomIn,
        ZoomOut,
        AddToShelf,
        InvertSelection,
        ToggleSelection,
        ToggleShelf,
        ToggleFullscreen,
        ToggleAlwaysOnTop,
        ToggleCompactOverlay,
        GoToFolderAlias,
        RotateRight,
        RotateLeft,
        ToggleToolbar,
        Redo,
        DuplicateTab,
        CloseOtherTabs,
        CopyPathQuoted,
        OpenInIde,
        ToggleTheme,
        CloseAllTabs,
        SplitPaneSideBySide,
        SplitPaneStacked,
        CloseActivePane,
        OpenInNewWindow,
        OpenTerminalAdmin,
        OrganizeFolder,
        ToggleAllGroups,
        OpenIconThemesFolder,
    ]
);

pub const FOLDER_CTX: &str = "FolderView";
/// The folder tree's key context. Tab is bound in it (to [`LeaveTree`]) because the component library binds Tab at the window's
/// root to "focus the next tab stop": a binding is matched before any key listener is asked, so the tree's own key handler never
/// saw Tab, and the keyboard stayed in the tree (issue 38: "TAB seems to be buggy, mine is not going anywhere"). A binding in a
/// context nearer to the focus wins over the root's.
pub const TREE_CTX: &str = "FolderTree";
/// The question dialog's key context: Tab and Shift+Tab move between its buttons there (bound here for the same reason as the
/// tree's Tab: the root's Tab binding would take the key first).
pub const DIALOG_CTX: &str = "Dialog";

/// User overrides from settings.toml `[keybindings]`. Invalid entries are logged and skipped.
pub fn bind_user(cx: &mut App) {
    let map = cx.global::<crate::settings::Settings>().keybindings.clone();
    let mut bindings = Vec::new();
    for (keys, action) in map {
        let name = if action.contains("::") { action.clone() } else { format!("rustyfiles::{action}") };
        match cx.build_action(&name, None) {
            Ok(a) => match KeyBinding::load(&keys, a, None, false, None, cx) {
                Ok(b) => bindings.push(b),
                Err(e) => log::warn!("keybinding {keys:?}: {e}"),
            },
            Err(e) => log::warn!("keybinding {keys:?} -> {action:?}: {e}"),
        }
    }
    cx.bind_keys(bindings);
}

pub fn bind(cx: &mut App) {
    let f = Some(FOLDER_CTX);
    cx.bind_keys([KeyBinding::new("tab", LeaveTree, Some(TREE_CTX))]);
    // Tab and Shift+Tab walk the window's panes: the address bar, the folder tree, the file list (issue 38: "TAB does not work
    // anywhere"). In the list's own context, nearer than the root's Tab; the tree's Shift+Tab goes up to the window's handler.
    cx.bind_keys([
        KeyBinding::new("tab", FocusNextPane, Some(FOLDER_CTX)),
        KeyBinding::new("shift-tab", FocusPrevPane, Some(FOLDER_CTX)),
        KeyBinding::new("shift-tab", FocusPrevPane, Some(TREE_CTX)),
    ]);
    cx.bind_keys([KeyBinding::new("tab", DialogNext, Some(DIALOG_CTX)), KeyBinding::new("shift-tab", DialogPrev, Some(DIALOG_CTX))]);
    cx.bind_keys([
        KeyBinding::new("alt-left", GoBack, None),
        KeyBinding::new("alt-right", GoForward, None),
        KeyBinding::new("alt-up", GoUp, None),
        KeyBinding::new("alt-home", GoHome, None),
        // Backspace goes back only from the list itself (issue 171: "after deleting letters rustyfiles navigates up"): a text box
        // in the view (the rename box, the filter) passes Backspace on when its caret is at the start, and that went back
        KeyBinding::new("backspace", BackspaceBack, f),
        KeyBinding::new("f5", Refresh, None),
        KeyBinding::new("ctrl-r", Refresh, None),
        KeyBinding::new("enter", OpenSelected, f),
        KeyBinding::new("down", SelectNext, f),
        KeyBinding::new("up", SelectPrev, f),
        KeyBinding::new("home", SelectFirst, f),
        KeyBinding::new("end", SelectLast, f),
        KeyBinding::new("ctrl-a", SelectAll, f),
        KeyBinding::new("shift-down", ExtendNext, f),
        KeyBinding::new("shift-up", ExtendPrev, f),
        KeyBinding::new("pagedown", PageDown, f),
        KeyBinding::new("pageup", PageUp, f),
        KeyBinding::new("escape", ClearSelection, f),
        KeyBinding::new("alt-enter", Properties, f),
        KeyBinding::new("ctrl-c", CopyItems, f),
        KeyBinding::new("ctrl-x", CutItems, f),
        KeyBinding::new("ctrl-v", PasteItems, f),
        KeyBinding::new("ctrl-shift-v", PasteIntoSelected, f),
        KeyBinding::new("delete", DeleteItems, f),
        KeyBinding::new("ctrl-d", DeleteItems, f),
        // (Ctrl+D deletes, as in Explorer; a copy beside the item has its own chord, issue 161)
        KeyBinding::new("ctrl-alt-d", DuplicateItems, f),
        // the selection into the other pane of a split tab (issue 152; F5 and F6 are Refresh and the pane switch here)
        KeyBinding::new("shift-f5", CopyToOtherPane, f),
        KeyBinding::new("shift-f6", MoveToOtherPane, f),
        KeyBinding::new("shift-delete", DeletePermanently, f),
        KeyBinding::new("f2", RenameItem, f),
        KeyBinding::new("ctrl-shift-n", NewFolder, f),
        KeyBinding::new("shift-f10", ShowContextMenu, f),
        KeyBinding::new("menu", ShowContextMenu, f),
        KeyBinding::new("ctrl-shift-c", CopyPath, f),
        KeyBinding::new("left", SelectLeft, f),
        KeyBinding::new("right", SelectRight, f),
        KeyBinding::new("ctrl-shift-1", ViewDetails, None),
        KeyBinding::new("ctrl-shift-2", ViewGrid, None),
        KeyBinding::new("ctrl-shift-3", ViewColumns, None),
        KeyBinding::new("ctrl-shift-4", ViewCards, None),
        KeyBinding::new("ctrl-shift-5", ViewList, None),
        KeyBinding::new("ctrl-z", Undo, f),
        KeyBinding::new("alt-p", TogglePreview, None),
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-shift-b", ToggleToolbar, None),
        KeyBinding::new("ctrl-y", Redo, f),
        KeyBinding::new("ctrl-shift-k", DuplicateTab, None),
        KeyBinding::new("ctrl-alt-c", CopyPathQuoted, f),
        KeyBinding::new("ctrl-alt-t", ToggleTheme, None),
        KeyBinding::new("ctrl-shift-d", ToggleDualPane, None),
        KeyBinding::new("f6", SwitchPane, None),
        KeyBinding::new("ctrl-shift-p", ToggleCommandPalette, None),
        KeyBinding::new("ctrl-alt-shift-r", RestartApp, None),
        KeyBinding::new("f1", ToggleCommandPalette, None),
        KeyBinding::new("ctrl-p", SearchFolderHistory, None),
        KeyBinding::new("ctrl-shift-a", SearchTabs, None),
        KeyBinding::new("ctrl-,", GoSettings, None),
        KeyBinding::new("ctrl-t", NewTab, None),
        KeyBinding::new("ctrl-n", NewWindow, None),
        KeyBinding::new("ctrl-shift-s", AddToShelf, f),
        KeyBinding::new("ctrl-space", ToggleSelection, f),
        // every group folded, or every group open again (issue 181)
        KeyBinding::new("ctrl-shift-g", ToggleAllGroups, f),
        KeyBinding::new("f11", ToggleFullscreen, None),
        KeyBinding::new("ctrl-=", ZoomIn, f),
        KeyBinding::new("ctrl-+", ZoomIn, f),
        KeyBinding::new("ctrl--", ZoomOut, f),
        KeyBinding::new("ctrl-w", CloseTab, None),
        KeyBinding::new("ctrl-shift-w", CloseActivePane, None),
        KeyBinding::new("ctrl-alt-enter", OpenInNewWindow, f),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("ctrl-shift-t", ReopenClosedTab, None),
        KeyBinding::new("ctrl-l", FocusAddress, None),
        KeyBinding::new("alt-d", FocusAddress, None),
        KeyBinding::new("ctrl-f", FocusSearch, None),
        KeyBinding::new("ctrl-e", FocusSearch, None),
        KeyBinding::new("ctrl-h", ToggleHidden, None),
        KeyBinding::new("ctrl-shift-`", OpenTerminal, None),
    ]);
    bind_layout_shifted(cx);
}

/// GPUI reports Shift+digit as the layout's shifted character without Shift (Turkish Q:
/// Ctrl+Shift+2 arrives as `ctrl-'`), so `ctrl-shift-2` alone never fires. Bind the
/// layout-specific form too.
fn bind_layout_shifted(cx: &mut App) {
    // (virtual key, action)
    let wanted: [(u16, Box<dyn Fn() -> Box<dyn Action>>); 6] = [
        (0x31, Box::new(|| Box::new(ViewDetails))),
        (0x32, Box::new(|| Box::new(ViewGrid))),
        (0x33, Box::new(|| Box::new(ViewColumns))),
        (0x34, Box::new(|| Box::new(ViewCards))),
        (0x35, Box::new(|| Box::new(ViewList))),
        (0xC0, Box::new(|| Box::new(OpenTerminal))), // VK_OEM_3 (` on US layouts)
    ];
    let mut bindings = Vec::new();
    for (vk, action) in wanted {
        let Some(ch) = rf_win::keys::shifted_char(vk) else { continue };
        // AZERTY: Shift+& gives "1"; binding ctrl-1 would steal a plain ctrl+digit shortcut
        if ch.chars().any(|c| c.is_alphanumeric()) {
            continue;
        }
        // Turkish Q: Shift+4 is "+" — Ctrl++ stays zoom
        if matches!(ch.as_str(), "+" | "-" | "=") {
            continue;
        }
        let keys = format!("ctrl-{ch}");
        match KeyBinding::load(&keys, action(), None, false, None, cx) {
            Ok(b) => bindings.push(b),
            Err(e) => log::debug!("layout binding {keys:?}: {e}"),
        }
    }
    cx.bind_keys(bindings);
}
