# Recycle Bin: delete for good, the bin's settings, the zip icon, and the shortcut key

Four small gaps from `notes/EXPLORER-EKSIKLERI.md` section 3 (items 5 and 6). Nothing changes for a user who does not use
them: every one is a new menu entry, button or field, and nothing runs at start-up.

## What is where

| Part | Pure logic (tested) | Windows side | Interface |
|---|---|---|---|
| Delete for good | `rf-core/recycle_bin.rs`: `command_targets`, `delete_facts`, `key_of`, `delete_through` + trait `BinBackend`, fake in the tests | `rf-win/recycle.rs`: `delete_by_keys`, `ShellBin` | `rf-app/recycle_view.rs` (button, row menu, Delete / Shift+Delete), `recycle_text.rs` (the words) |
| The bin's settings | `rf-core/recycle_bin.rs`: `BinSettings`, `plan` (validation, only the differences), `save` + trait `BinSettingsStore`, fake in the tests | `rf-win/recycle_settings.rs`: `SystemBinSettings` | `rf-app/recycle_settings_view.rs` (dialog over the bin page) |
| Zip drawn as a folder | `rf-core/recycle_bin.rs`: `is_real_folder` | `rf-win/recycle.rs`: `read_item` | `recycle_view.rs`: `row_icon` |
| Shortcut key | `rf-core/hotkey.rs`: `decide`, `text`, `parse`, `vk_from_key_name` | `rf-win/shortcut.rs` (the existing save, unchanged) | `rf-app/hotkey_field.rs`, three small hooks in `properties_view.rs` |

## (a) Delete for good

* The command applies to the selected rows that are shown (`command_targets`; a row the search filter hides is never acted on).
* Our confirmation (`prompt::show`): the title "Kalıcı olarak silinsin mi?", the item's name (or the count), folders among them,
  the size, "Bu işlem geri alınamaz.". There is no primary button, so Enter does nothing; the red button deletes.
* The deletion is the bin's own `delete` verb, invoked through `IContextMenu` on the ID lists of exactly the chosen items
  (found by their parsing path inside the bin), with `SEE_MASK_FLAG_NO_UI` because our dialog has asked. This is the route
  `restore` already takes with `undelete`. Why this and not `IFileOperation` on the bin item: the verb is the bin's own
  implementation, so it removes the item's `$R` data and its `$I` record together, and it is the exact thing Explorer's
  "Delete" in the bin runs; `IFileOperation` on a `$R` path would be a plain file delete behind the bin's back and would leave the `$I` record.
* How we know it cannot touch anything else, in two layers. (1) By construction: the ID lists handed to `GetUIObjectOf` are only
  those whose key matched; none matched is an error and nothing is invoked. (2) By check: `delete_through` lists the bin before and
  after and counts the requested items gone, the requested items left, and any item that was NOT requested and is gone
  ("collateral"). That count is shown in the status line as a problem and logged. It cannot undo anything; it makes a surprise visible.
* The list is read again afterwards and the status line (under the page title) says "N öğe kalıcı olarak silindi", and the number
  that could not be deleted, from the two listings, not from the shell's answer alone.
* Both calls go through `rf_win::sta::call` (limit 10 minutes), so the bin work runs on the shell thread and the UI thread never waits.
  The before / after listing reads only the parsing names (`list_keys_only`, one name per item; no sizes, dates or attributes), and
  a test checks that it sees the same items as the full listing.
* If the shell thread does not answer within its limit, the job may still run later. `delete_through` then reports the outcome as
  **unknown** (`DeleteReport::unknown`, the status line says "Sonuç bilinmiyor; öğeler silinmiş olabilir"), claims neither deleted nor
  left, and does not queue a second listing behind the stuck job. The same holds when the bin cannot be listed after the deletion.
* Right click on a row opens one menu (the row's handler stops the event before the page's background handler sees it). Delete /
  Shift+Delete / Ctrl+D are ignored while the search box has the keyboard (typing there is not a command on the rows).
* A row shows the shell's own icon for the item's stored copy (`$R...` keeps the extension, so a zip gets the zip icon), through the
  shared icon cache the folder list uses; the glyph (tinted as in the folder list) is drawn until the icon has arrived.

**Not run:** the real `delete` verb on a real bin (the rule was never to delete anything in the user's real bin). What is
unknown: how the bin's verb treats `SEE_MASK_FLAG_NO_UI`. Three outcomes are possible: it skips its own "are you sure" (what we
want); it ignores the flag and asks (a second question, shown by the shell with no owner window, so it may sit behind the app);
or it refuses because it may not ask (every deletion then ends as "N öğe silinemedi": a safe failure, but the feature does
nothing). The environment variable `RUSTYFILES_BIN_DELETE_SHELL_UI=1` leaves the flag out, so the first live run can try both
without a rebuild. Also unknown: whether a failure of one item (in use) makes the verb return an error or silently skip it (both are
handled: the report counts what is left).

## (b) The bin's own settings

Opened from the bin page: the "Ayarlar" button or the right-click menu ("Geri Dönüşüm Kutusu ayarları…"). It READS when it opens
(off the UI thread) and writes ONLY when Kaydet is pressed, and then only what differs from what it read (`plan` / `save`);
a problem (not a whole number, below 1 MB, above the drive's size) is shown under the drive and nothing is written.

Source of the values (how sure):

* Per drive: `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{volume GUID}`, `MaxCapacity` (REG_DWORD,
  MB) and `NukeOnDelete` (REG_DWORD, 1 = remove immediately). **Fairly sure** about the key and the value names: the read side was
  checked on this machine (read-only probe `recycle_probe`: every one of the 5 fixed drives has a `MaxCapacity` under that key).
  **Not checked:** that a value written there is what Explorer's own dialog would write and that the shell picks it up without
  being told (Explorer may also send a notification or restart the bin's cache). The default size of a drive without
  `MaxCapacity` is not known to us, so the box is empty then ("Varsayılan").
* "Display delete confirmation": `SHGetSetSettings` with `SSF_NOCONFIRMRECYCLE` (the shell's `fNoConfirmRecycle`, kept in the
  `ShellState` value), and the policy `Policies\Explorer\ConfirmFileDelete = 1` shows it as on and fixed. **Cross-checked
  read-only:** the API answer on this machine matches the raw `ShellState` byte (bit 0x04 of byte 4) in the probe. **Unrun:**
  the write (`SHGetSetSettings(.., true)`), and whether Windows 11's own checkbox keeps this setting in the same place. The API is
  marked "no longer supported" by Microsoft, though it works.
* The write path is `SystemBinSettings::write`. It refuses an id that is not a volume GUID or not a drive the system lists. **It
  has never been run.** No test calls it; the dialog's Kaydet is the only caller. It writes the drives one after the other and the
  confirmation last, so a failure half way leaves the earlier drives written: the dialog then reads the settings again (its
  "now"), and a second Kaydet writes only what is still different. After setting the confirmation it reads it back and reports
  an error when the system did not take it (the API gives no answer of its own).
* A size the user did not touch is not validated (`plan` compares the text with the value read first): a value above the drive's
  size (a shrunk partition, a stale value) does not stop the rest from being saved. A size the user types is always checked.

## (c) A zip file in the bin drawn as a folder

Cause, found with the read-only probe: for compressed (zip) folders the shell reports `SFGAO_FOLDER` and also `SFGAO_STREAM`. On
this machine's bin: 10 items, 4 the shell calls folders, all 4 with the stream flag, 0 directories for the file system. The old
code took `SFGAO_FOLDER` as "folder". Now `is_real_folder`: the file system's answer wins when the shell says folder (one
`symlink_metadata`, only for what the shell calls a folder), otherwise folder and not stream. The row icon is the file's own
kind by its name (the same table the folder list uses), so a zip is an archive icon. After the fix the probe says 0 of 10 drawn as folders.
A junction or a directory link deleted into the bin is not a directory to `symlink_metadata` (which does not follow links); for such a
link the file system gives no answer (`fs_dir_answer`) and the shell's folder-and-not-stream decides, so it stays a folder.
The drawn picture is the shell's own icon of the stored copy (see (a)); the archive glyph is only the stand-in until it arrives.

## (d) The shortcut key

Click the field and press the combination. The rules (`rf-core/hotkey.rs`) are those of the Windows hot-key control that
Explorer's tab uses (`HKM_SETRULES`): a key alone, or with Shift only, is completed to Ctrl + Alt; Backspace or Delete alone
empties the field; letters, digits, F1 to F24, the number pad, Space and the navigation keys are accepted; Esc, Tab, Enter, Print
Screen and layout-dependent punctuation are refused; Alt+F4, Alt+Space and Ctrl+Alt+Del are refused as Windows' own; the
Windows key is refused. A refusal is one line under the field. Tab and an unmodified Esc leave the field. The keys are caught with
a keystroke interceptor that is installed the first time a field is clicked and acts only while that field has the focus, so the
pressed keys never reach their bound actions. The word is stored through the existing `shortcut::save`; a test
(`a_decided_hotkey_is_saved_and_nothing_else_changes`) saves a combination on a temp link and checks that only the hot key differs, and that
emptying it restores the original. The same test also compares the whole `.lnk` file byte by byte before and after setting the key: the size
is the same and only the two header bytes that hold the key (offset 64) differ, so the ID list, link info, extra data and property
store are untouched by the key. (It cannot say what the existing `save` does to a link that has parts `Details` does not model,
on a link that was never saved by it; that is the exposure Apply always had, not something this change adds.)

**Not run / not known:** which combinations Windows then honours at run time (a link's hot key only works for links in the Start
menu or on the desktop, and the shell decides); the refusal list is our reading of the control's rules and Windows' reserved
combinations, not something verified against Explorer. Whether Explorer completes a lone function key to Ctrl+Alt+F-key (our
assumption, from `HKCOMB_NONE`) is not verified. Number-pad keys are shown and kept when they exist already, but cannot be typed
in (GPUI does not tell them from the main row).

## Cost

* Feature off: nothing runs at start-up. The bin listing asks the shell for one more attribute bit in the call it already made,
  plus one `symlink_metadata` per item the shell calls a folder (folders and zips only), on the worker thread that lists the bin.
  The folder list (100 000 entries) is not touched; its two Delete action handlers first test one `match` on the location.
* On: one shell-thread job per deletion; the dialog reads the registry for the fixed drives once when it opens; the hot-key
  interceptor costs one global lookup per keystroke after the first click on a field.

## Probe

`cargo run --offline -p rf-win --example recycle_probe` prints counts only (bin items, folder flags, settings, the ShellState
cross-check). It creates no window and writes nothing.
