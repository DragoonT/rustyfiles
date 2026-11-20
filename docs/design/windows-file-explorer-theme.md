# Windows File Explorer appearance

The optional **Windows File Explorer look** switch is in Settings > Appearance. It is
off by default, stored with the app's existing settings, and can be switched at any time.
With it off, RustyFiles uses its original UI: the pre-Explorer navigation and command bars,
sidebar and file-list spacing, panel surfaces, and original app icons. No Explorer-look
styling is applied.

With the switch on, the existing Explorer look remains intact and is extended with the
recent navigation-pane and file-list styling, Explorer-like navigation and command-bar
surfaces, separators, spacing, proportions, and toolbar placement. Applicable files,
folders, drives, tabs, and Recycle Bin states use Windows Shell artwork at runtime. Common
toolbar and context-menu actions use system glyphs: Segoe Fluent Icons on Windows 11 and
Segoe MDL2 Assets on Windows 10. Icons without a matching system glyph keep their original
meaning and appearance; context menus also keep the Explorer-style presentation. The tab
strip keeps its glass plus button and existing slide-in animation.

The sidebar width is remembered separately for the two appearances: 232 px by default
with the look off and 366 px by default with it on. On upgrade, a non-default saved width
carries forward into the Explorer look until it is resized there. Resizing the sidebar in
one appearance does not change its width in the other.

With the look enabled, the command-bar More, Layout, New, and Sort & group menus use the
same Explorer-style opaque popup presentation as the file right-click menu. New and
Sort & group keep the app's original icons even when the look is enabled; icons supplied
by Windows for New-menu templates remain as supplied. With the switch off, the existing
menu styling and icons are unchanged. Switching the look off restores the original app
icons; the theme does not replace their definitions or bundle the user's separate ICO
collection.

Some theme artwork is adapted from Icons8. The app links to Icons8 in Settings > About.
