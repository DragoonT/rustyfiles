# The terminal button on a tab

A terminal icon on each folder tab (hover-revealed, same size as the close button). Clicking it opens a terminal in that tab's folder; on a background tab it does so without activating the tab (`stop_propagation` on both mouse-down and click).

The launcher is the sidebar folder menu's `rf_win::shell::open_terminal`, unchanged. The tooltip reuses `t!("Terminalde aç")`.

The button is drawn only for real disk folders: `Home`, Settings, search results, the Recycle Bin, archive entries and FTP/network roots have none (`tab_terminal_dir`, tested). `p.is_dir()` is one stat per tab per frame; the title-bar loop already reads `location().path()` per tab.

Not run: the hover fill on both themes, a strip scrolled past the tab minimum, the tooltip while a fluent menu is open.
