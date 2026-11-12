# The preview pane at the bottom

Explorer's details pane can sit at the bottom of the window; rustyfiles' preview pane was always at the right. This adds the
bottom position as a setting. **The default is the right, and nothing about it changes**: a user who does not ask for the bottom
sees the same pixels, the settings file gets no new key (`preview_position` and `preview_height` are written only when they are
not the defaults), and the only cost is one settings read per frame (the position is read once, the pane is not touched).

Code: the rules are `crates/rf-core/src/previewpane.rs` (pure, tested without a window); the window's side is
`crates/rf-app/src/preview_dock.rs` (a child module of `workspace`, so `workspace.rs` only gets small hooks); the bottom layout of
the pane itself is `crates/rf-app/src/preview_wide.rs` (a child module of `preview`).

## What it does

* Settings: `preview_position` ("right", the default, or "bottom") and `preview_height` (kept; 200 px until it is dragged).
  Where to set it: Settings > Layout, the row "Önizleme paneli yeri" next to "Önizleme paneli"; and the window's tab-actions menu
  (the corner button's menu that has "Kenar çubuğu" and "Klasör ağacı"), a pair "Önizleme sağda" / "Önizleme altta" with a check
  on the current one. Choosing one of them also shows the pane. Alt+P and the command bar's button still show and hide the pane,
  in whichever position it is. An unknown word in the file reads as "right".
* At the bottom the pane spans the width of the content card under the file list, and under the shelf when that is open (the shelf
  narrows only the list). A border on its top edge, like the right pane's left edge.
* **Split tab (two panes).** Today the pane sits at the right of the whole content area, not inside either file pane, and shows the
  focused item of the active pane. At the bottom it sits under the whole content area the same way (under both file panes,
  whether they are side by side or stacked) and follows the active pane. That is what "where it is today" gives, so it was kept.
  When the two panes are stacked there are two lists above the pane, and each must keep its 180 px: the pane is then allowed
  `area - 360` px at most (shown from an area of 480 px; with a single list or two side by side `area - 180`, from 300 px).
* **File dialogs (Open/Save windows)** keep the pane at the right whatever the setting is: those windows keep their own on/off state
  for the pane (the settings observer does not copy `preview_open` into them), are small, and the position is a choice for the file
  manager's windows. The two menu entries are not offered in them.
* The pane's height is dragged from its top edge (a 6 px strip over the edge with a row-resize cursor). The height is limited to
  120..520 px and so that the list keeps at least 180 px (`bottom_max`); the wish is kept in `preview_height` even when the window
  is too small to show it (a later, taller window shows it). The setting is written 400 ms after the drag settles, as the right
  pane's width is. Every window follows the saved height (the settings observer copies it), so a drag in one window shows in the
  others and a hand edit or reset needs no restart.
* Room: the pane is not shown (as the right one is not shown in a narrow window) when the content area is narrower than 560 px (an
  icon, a 240 px column of properties and a 160 px text block side by side) or too low for 120 px of pane above its list(s). The area's height is measured from the previous frame's layout by an
  invisible canvas in the row (until then the window's height minus 190 px stands in); a changed height lays the window out once
  more.

## The layout inside the wide pane

The pane's content is the same as at the right, in a row (`preview_wide.rs`):

1. **Picture or icon box at the left**, as high as the pane's content. 4:3 for a picture or a type icon (96..280 px, never more than
   a third of the pane). A video or a Windows preview handler (a document page) gets more width (two fifths of the pane, 16:9 of the
   height at least, at most 55 % and 640 px). An audio file gets at least 220 px so that its player bar has room. A file that has a
   text, archive or shortcut block of its own gets only a 96 px icon. Whatever the kind, the box never takes the width the properties
   need: they always keep a 240 px column.
   The video, the handler and the thumbnail are the same code paths as at the right (the hosted windows are placed over the
   box by the same `place_video` / `place_host`).
2. **A player bar under the picture** of an audio / video file (play, seek, time).
3. **The name and the properties** beside it, in columns. `property_plan(width, height, count)` keeps the properties in their order,
   fills columns left to right, and uses as few columns as the height needs (a column is 240 px at least, four at most); if even
   the widest arrangement is too tall, the rows are shared evenly and the block scrolls. The height counts the name line (24 px),
   the gaps of the stack and the hash button when it stands under the properties (38 px). A value that wraps (a long path, a long
   media value) makes its column taller than planned; the block then scrolls, nothing is cut. The name is one line with an ellipsis,
   and a tooltip with the whole name is attached when it may not fit. The rows are the same function for both
   positions (`Preview::property_rows`: type, size, the media properties, dates, location, attributes).
4. **A text / archive / shortcut block** after the properties, using the whole height (text scrolls; markdown renders as at the
   right). The properties then keep to about half of the room, and the text block keeps at least 160 px.
5. **The hash action** in a 200 px column at the right when the pane is 1100 px wide or more, else under the properties.

The empty state ("select an item") is the same message with a bottom-panel icon.

## Cost

* Position right (default): one `cx.global::<Settings>()` read per frame (and the settings observer compares one number); no entity
  update, no allocation, no extra element, no window query. The right layout's rows come from the same function the bottom uses
  (`property_rows`), which builds the same list the inline code built.
* Position bottom: the thumbnail, the properties, the text and the hosted windows cost what they cost at the right. In addition a
  canvas in the row measures its height every frame (a `Cell` write); only a change of more than half a pixel asks for one more
  layout. Nothing reads the disk or calls the shell on the UI thread; all listing, media properties, thumbnails and text reading
  still run off it as before.

## Tests and what is not tested

`rf_core::previewpane` tests: the stored word; the right-hand rule stays `viewport_w >= pane_w + 380 + sidebar` (written out as
numbers); the bottom limits at every area height, for one list and for two stacked ones; the splitter arithmetic; the pane and the
list tile the area (`bottom_layout`, which the window calls for the pane's height); the inner layout, with the properties and the
text block keeping a readable column at every width the pane can be shown at, for every kind; the audio box; the stack height of
name + properties + button; the property columns (every property in exactly one column, in order, for many sizes); a table for
common window sizes (`cargo test -p rf-core previewpane -- --nocapture`). `rf-app`: the settings round trip and "no key until
asked for". The right position has no rectangles of its own to test: its layout is the window's flexbox, unchanged.

Not run by anyone: the window itself. Everything that needs a screen (the splitter drag, the look of the wide pane with a real
thumbnail, a video or a Windows preview handler in the low box, the dual-pane case, a file dialog's window) is for the first live
try; its steps are in the report of the change.
