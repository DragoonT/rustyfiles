//! The tray host (shell plan increment D2; design and the steps of a first careful try in `docs/design/tray-host.md`).
//!
//! Programs put icons in the notification area by sending `WM_COPYDATA` to the first top-level window of class `Shell_TrayWnd`. While
//! Explorer runs, this host owns such a window IN FRONT of Explorer's, takes what arrives, and passes EVERY message on to Explorer's
//! window untouched, returning Explorer's answer, so that Explorer's own tray keeps working exactly as before. What it takes it keeps
//! as a model (icons with their picture, tip, owner, callback message and version) that the dock can draw and click.
//!
//! It exists only while the setting `shell_tray_host` is on (default OFF) and only in the shell host process. `rf_win::tray` (the
//! program's own tray icon) is a different thing and is not touched.
//!
//! THE WIRE FORMAT IS NOT MEASURED. Every size and offset comes from `rf_win::tray_wire`, which took them from documents and other
//! shells' sources; nothing was captured on this Windows (the `tray_spy` example, which nobody has run, settles it). They stay in that
//! parser; nothing in this directory knows an offset.
//!
//! | file | what | touches Windows? |
//! |---|---|---|
//! | `model.rs` | icons, merge rules, generations, bounds (256 icons, 64 x 64 pictures, 256-character tips) | no |
//! | `pixels.rs` | colour / mask / monochrome arithmetic, shrinking | no |
//! | `core.rs` | the handler: parse, own handling, pass on untouched, answer as a tray when there is none | through traits |
//! | `relay.rs` | Explorer's tray window: find, cache, check, look again, send | through traits |
//! | `clicks.rs` | host events to the owners' callback messages, dead owners | through traits |
//! | `lifecycle.rs` | start (window, then broadcast), stop (window FIRST, then broadcast), the 250 ms tick | through traits |
//! | `order.rs` | strip / overflow / order from the user's own list | no |
//! | `adopt.rs` | when a modify for an icon the host never saw added may be taken as the add it missed | no |
//! | `own_icons.rs` | which of Windows' own indicators the dock already shows as a button (left out of the strip) | no |
//! | `shared.rs`, `view.rs` | the lock, the generation, the counters, the event queue; what the interface holds | no |
//! | `handshake.rs` | who closes the window when the starter has given up waiting | no |
//! | `stall.rs` | notices a host thread that has stopped ticking (a log line; it cannot fix one) | no |
//! | `traits.rs`, `fakes.rs` | the small traits and their stand-ins | no |
//! | `testkit.rs` | a model + view + fake owners with no window and no thread, for the tests of whoever reads the view | no |
//! | `win.rs` | the real implementations, the window class, the window procedure, the thread | YES, never run by a test |
//! | `host.rs` | `TrayHost::start` / `stop` | through `win.rs` |
//!
//! What the tests never do (scanned in `scans` below): create a window, send or post a message, broadcast, start the host.

pub mod adopt;
pub mod clicks;
pub mod core;
pub mod fakes;
pub mod handshake;
pub mod host;
pub mod lifecycle;
pub mod model;
pub mod order;
pub mod own_icons;
pub mod pixels;
pub mod relay;
pub mod shared;
pub mod stall;
pub mod testkit;
pub mod traits;
pub mod view;
pub mod win;

pub use clicks::{ClickOutcome, ClickRequest, OwnerRef};
pub use host::{TrayHost, TrayHostConfig};
pub use model::{IconBitmap, IconId, Snapshot, StableKey, TrayEvent, TrayIcon};
pub use shared::Counts;
pub use stall::{StallEvent, StallWatch};
pub use view::TrayView;

#[cfg(test)]
mod scans {
    //! Scans of the sources, so that adding a forbidden call later fails here. Only the code before each file's tests is read, and
    //! comment lines are skipped.

    fn sources() -> Vec<(&'static str, &'static str)> {
        vec![
            ("adopt.rs", include_str!("adopt.rs")),
            ("own_icons.rs", include_str!("own_icons.rs")),
            ("clicks.rs", include_str!("clicks.rs")),
            ("core.rs", include_str!("core.rs")),
            ("fakes.rs", include_str!("fakes.rs")),
            ("handshake.rs", include_str!("handshake.rs")),
            ("host.rs", include_str!("host.rs")),
            ("lifecycle.rs", include_str!("lifecycle.rs")),
            ("model.rs", include_str!("model.rs")),
            ("order.rs", include_str!("order.rs")),
            ("pixels.rs", include_str!("pixels.rs")),
            ("relay.rs", include_str!("relay.rs")),
            ("shared.rs", include_str!("shared.rs")),
            ("stall.rs", include_str!("stall.rs")),
            ("traits.rs", include_str!("traits.rs")),
            ("view.rs", include_str!("view.rs")),
            ("win.rs", include_str!("win.rs")),
        ]
    }

    fn code_of(text: &str) -> String {
        text.split("#[cfg(test)]").next().unwrap_or(text).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    fn tests_of(text: &str) -> String {
        text.split("#[cfg(test)]").skip(1).collect::<Vec<_>>().join("\n").lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn only_win_rs_talks_to_windows() {
        for (name, text) in sources() {
            let code = code_of(text);
            let touches = code.contains("windows::") || code.contains("windows_registry");
            assert_eq!(touches, name == "win.rs", "{name}");
        }
    }

    #[test]
    fn nothing_here_can_reach_the_registry_the_shell_identity_hotkeys_hooks_or_other_programs() {
        for (name, text) in sources() {
            let code = code_of(text);
            for banned in [
                "SetShellWindow",
                "SetTaskmanWindow",
                "Progman",
                "RegisterHotKey",
                "SetWindowsHookEx",
                "RegSetValue",
                "RegCreateKey",
                "RegDeleteKey",
                "RegDeleteValue",
                "windows_registry",
                "ShellExecute",
                "CreateProcess",
                "Command::new",
                "TerminateProcess",
                "ExitWindows",
                "SystemParametersInfo",
                "SHAppBarMessage",
                "Shell_NotifyIcon",
                "keybd_event",
                "SendInput",
                "CoCreateInstance",
                "CoInitialize",
                "std::fs::write",
                "File::create",
            ] {
                assert!(!code.contains(banned), "{name} uses {banned}");
            }
        }
    }

    #[test]
    fn windows_that_are_moved_or_shown_are_only_ours() {
        let win = code_of(include_str!("win.rs"));
        // every SetWindowPos call names one of our own windows: `h` is the raise of our window, `me` and `child` the mirror
        let mut calls = 0;
        for call in win.split("SetWindowPos(").skip(1) {
            calls += 1;
            let first = call.split(',').next().unwrap_or("").trim();
            assert!(["h", "me", "child"].contains(&first), "SetWindowPos on `{first}`");
        }
        assert_eq!(calls, 3, "the raise, the window mirror and the child mirror");
        // one ShowWindow, on our own window, only for the visible option
        assert_eq!(win.matches("ShowWindow(").count(), 1);
        assert!(win.contains("ShowWindow(hwnd, SW_SHOWNOACTIVATE)"));
        // the only DestroyWindow calls are on our own handles
        for call in win.split("DestroyWindow(").skip(1) {
            let first = call.split(')').next().unwrap_or("").trim();
            assert!(["h", "hwnd"].contains(&first), "DestroyWindow on `{first}`");
        }
        // Explorer's window is only ever sent messages that came to ours (through the relay), never a message of our own making
        assert_eq!(win.matches("SendMessageTimeoutW(").count(), 1);
        assert!(win.contains("SendMessageTimeoutW(h, raw.msg, WPARAM(raw.wparam), LPARAM(raw.lparam)"));
        // the broadcast is one call
        assert_eq!(win.matches("HWND_BROADCAST,").count(), 2, "the import and the one call");
    }

    #[test]
    fn no_test_starts_the_real_host_or_touches_a_window() {
        for (name, text) in sources() {
            let tests = tests_of(text);
            for banned in ["TrayHost::start(", "win::spawn", "host_thread", "Win32Window", "Win32Sender", "Win32Finder", "Win32Broadcast", "Win32Owners", "CreateWindowEx", "PostMessage", "SendMessage", "SendNotifyMessage"] {
                // (the scan itself, in this file, names them; the sources it reads do not)
                assert!(!tests.contains(banned), "{name}: a test mentions {banned}");
            }
        }
        // the one real thing tests do: read pictures through GDI from icons they make
        let t = tests_of(include_str!("win.rs"));
        assert!(t.contains("CreateIconIndirect") && !t.contains("RegisterClass"));
    }

    /// The text of a log statement split into its string literals and the rest (the arguments).
    fn split_literals(stmt: &str) -> (String, String) {
        let (mut lits, mut rest, mut in_str, mut prev_backslash) = (String::new(), String::new(), false, false);
        for ch in stmt.chars() {
            if in_str {
                if ch == '"' && !prev_backslash {
                    in_str = false;
                    lits.push(' ');
                } else {
                    lits.push(ch);
                }
                prev_backslash = ch == '\\' && !prev_backslash;
            } else if ch == '"' {
                in_str = true;
                prev_backslash = false;
            } else {
                rest.push(ch);
            }
        }
        (lits, rest)
    }

    const LOG_BANNED: [&str; 11] = ["tip", "title", "info", "text", "owner_exe", "exe", "bgra", "pixels", "image", "balloon", "name"];

    /// Why a log statement is refused, if it is: a banned identifier among its ARGUMENTS (as a whole word of the code, so a field
    /// access such as `icon.tip` counts), or one named inside the format string as an inline argument (`{tip}`).
    fn log_problem(stmt: &str) -> Option<String> {
        // (skip the macro's own name: `log::info!(` is not an argument called `info`)
        let after_macro = stmt.find('(').map_or("", |i| &stmt[i + 1..]);
        let (lits, args) = split_literals(after_macro);
        for word in LOG_BANNED {
            if lits.contains(&format!("{{{word}")) {
                return Some(format!("the format string prints `{word}`"));
            }
            if args.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|tok| tok == word) {
                return Some(format!("an argument mentions `{word}`"));
            }
        }
        None
    }

    #[test]
    fn the_log_scan_catches_field_accesses_and_inline_arguments() {
        assert!(log_problem(r#"log::info!("seen {}", icon.tip);"#).is_some(), "a field access, which the old scan let through");
        assert!(log_problem(r#"log::info!("seen {}", x.owner_exe);"#).is_some());
        assert!(log_problem(r#"log::info!("seen {tip}");"#).is_some());
        assert!(log_problem(r#"log::info!("seen {}", snap.icons[0].title.len());"#).is_some());
        assert!(log_problem(r#"log::info!("the tip is a word in a sentence", n);"#).is_none(), "words inside a fixed sentence are judged by eye");
        assert!(log_problem(r#"log::info!("{n} icon(s) dropped", );"#).is_none());
        assert!(log_problem(r#"log::warn!("cannot start: {e}");"#).is_none());
    }

    #[test]
    fn no_log_line_names_a_tip_a_title_a_text_a_program_or_a_picture() {
        let mut lines = 0;
        for (name, text) in sources() {
            let code = code_of(text);
            let mut rest = code.as_str();
            while let Some(at) = rest.find("log::") {
                let tail = &rest[at..];
                let end = tail.find(");").map_or(tail.len(), |e| e + 2);
                let stmt = &tail[..end];
                lines += 1;
                if let Some(why) = log_problem(stmt) {
                    panic!("{name}: {why}: {stmt}");
                }
                rest = &tail[end.min(tail.len())..];
            }
        }
        assert!(lines > 0, "the scan found the log lines it is meant to check");
    }

    #[test]
    fn the_tick_does_not_walk_every_window_and_a_closed_window_is_forgotten_by_its_object() {
        let win = code_of(include_str!("win.rs"));
        // one EnumWindows: the finder that looks for Explorer's window when its cached handle is gone; the 250 ms tick uses FindWindow
        assert_eq!(win.matches("EnumWindows(").count(), 1);
        let first = win.split("fn first_tray_hwnd").nth(1).unwrap_or("").split("
pub struct").next().unwrap_or("");
        assert!(first.contains("FindWindowW(") && !first.contains("EnumWindows"));
        // WM_CLOSE and WM_ENDSESSION destroy through the window object, which removes the properties first and forgets the handle
        for arm in ["WM_ENDSESSION =>", "WM_CLOSE =>"] {
            let body = win.split(arm).nth(1).unwrap_or("").split("LRESULT(0)").next().unwrap_or("");
            assert!(body.contains("destroy_own(hwnd)") && !body.contains("DestroyWindow("), "{arm}");
        }
    }
}
