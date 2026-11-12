//! The shell host's opt-in keyboard layer (shell plan increment B7; design in `docs/design/shell-keys.md`).
//!
//! Two settings, both OFF by default: `shell_win_key` (a tap of the Windows key alone opens OUR Start panel instead of Windows')
//! and `shell_own_switcher` (Alt+Tab opens OUR switcher). Beside Explorer both need a low-level keyboard hook, and there is ONE:
//! `switcher::hook::LowLevelHook`, started by [`KeyService`] only inside the shell host and only while at least one setting is on.
//!
//! | file | what | rules it obeys |
//! |---|---|---|
//! | `machine.rs` | the Windows-key machine: which release is a clean tap | pure, `Copy`, no allocation |
//! | `layer.rs` | the hook callback: swallow or pass each event, post what happened | no lock, no allocation, no logging, no COM, nothing that waits |
//! | `shared.rs` | the atomics shared with the interface thread: settings, heartbeat, full-screen, resets | atomics only |
//! | `queue.rs` | the ring through which the callback posts to the interface | one writer, one reader, never blocks |
//! | `mask.rs` | the mask key: the ONE `SendInput` of the layer | virtual key 0xE8 only |
//! | `service.rs` | install, remove, heartbeat, notice a dropped hook, dispatcher thread | on the interface thread |
//! | `fakes.rs` | stand-ins for tests and the windowless probe | send and read nothing |
//!
//! What it never does: register a hotkey, send any key other than the mask, log or store or forward a key, or swallow a Windows key,
//! Alt or a modifier. The tests below scan the sources for the first four, each as its own test.

pub mod fakes;
pub mod layer;
pub mod machine;
pub mod mask;
pub mod queue;
pub mod service;
pub mod shared;

pub use layer::KeyLayer;
pub use machine::{WinMachine, WinOutcome};
pub use mask::{MASK_VK, MaskSender, SendInputMask};
pub use queue::Posted;
pub use service::{Beat, Deliver, KeyProbe, KeyService, SystemProbe, Why};
pub use shared::{Clock, HEARTBEAT_EVERY_MS, STALE_AFTER_MS, Shared, Tracked};

#[cfg(test)]
mod scans {
    //! Scans of the sources, so that adding a forbidden call later fails here. Only the code before each file's tests is read, and
    //! comment lines are skipped.

    /// (name, text) of every source of the layer, including the two switcher files it is built on.
    fn sources() -> Vec<(&'static str, &'static str)> {
        vec![
            ("machine.rs", include_str!("machine.rs")),
            ("layer.rs", include_str!("layer.rs")),
            ("shared.rs", include_str!("shared.rs")),
            ("queue.rs", include_str!("queue.rs")),
            ("mask.rs", include_str!("mask.rs")),
            ("service.rs", include_str!("service.rs")),
            ("fakes.rs", include_str!("fakes.rs")),
            ("mod.rs", include_str!("mod.rs")),
            ("hook.rs", include_str!("../switcher/hook.rs")),
            ("chord.rs", include_str!("../switcher/chord.rs")),
        ]
    }

    /// The code of a source: before its tests, comment lines removed.
    fn code_of(text: &str) -> String {
        text.split("#[cfg(test)]").next().unwrap_or(text).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    /// The text of every `log::` macro call in `code`, from the macro name to its closing `);`.
    fn log_statements(code: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = code;
        while let Some(at) = rest.find("log::") {
            let tail = &rest[at..];
            let end = tail.find(");").map_or(tail.len(), |e| e + 2);
            out.push(tail[..end].to_string());
            rest = &tail[end.min(tail.len())..];
        }
        out
    }

    #[test]
    fn the_only_send_input_in_the_keyboard_layer_is_the_mask_keys() {
        let mut calls = Vec::new();
        for (name, text) in sources() {
            let code = code_of(text);
            let n = code.matches("SendInput(").count();
            if n > 0 {
                calls.push((name, n));
            }
            for other in ["keybd_event", "mouse_event", "PostMessageW", "SendMessageW", "SendNotifyMessage", "PostThreadMessageW(self.thread_id, WM_KEY", "RegisterHotKey"] {
                // (hook.rs posts WM_QUIT to its own thread to stop it: that is the one PostThreadMessageW, and not a key)
                assert!(!code.contains(other), "{name} uses {other}");
            }
        }
        assert_eq!(calls, vec![("mask.rs", 1)], "exactly one SendInput call, in mask.rs");
        let mask = code_of(include_str!("mask.rs"));
        assert_eq!(mask.matches("wVk:").count(), 1, "one key is described");
        assert!(mask.contains("wVk: VIRTUAL_KEY(MASK_VK)"), "and it is the mask key");
        assert_eq!(mask.matches("VIRTUAL_KEY(").count(), 1, "no other virtual key is built there");
    }

    #[test]
    fn no_key_code_is_formatted_into_a_log_line() {
        let banned = ["vk", "scan", "flags", "RawKey", "KeyInput", "Key::", "mods", "input", "tracked", "believed", "Posted"];
        let mut lines = 0;
        for (name, text) in sources() {
            for stmt in log_statements(&code_of(text)) {
                lines += 1;
                for word in banned {
                    assert!(!stmt.contains(word), "{name}: a log line mentions `{word}`: {stmt}");
                }
            }
        }
        assert!(lines > 0, "the scan found the log lines it is meant to check");
    }

    #[test]
    fn the_hook_callback_files_do_not_log_at_all() {
        // the callback path: nothing here may call into the logger, which can take a lock and allocate
        for (name, text) in sources() {
            if ["machine.rs", "layer.rs", "shared.rs", "queue.rs", "mask.rs", "hook.rs", "chord.rs"].contains(&name) {
                let code = code_of(text);
                for banned in ["log::", "println!", "eprintln!", "dbg!"] {
                    assert!(!code.contains(banned), "{name} uses {banned}");
                }
            }
        }
    }

    #[test]
    fn the_hook_callback_files_have_no_allocation_no_locks_and_nothing_that_waits() {
        for (name, text) in sources() {
            if !["machine.rs", "layer.rs", "shared.rs", "queue.rs", "mask.rs", "chord.rs"].contains(&name) {
                continue;
            }
            let code = code_of(text);
            for banned in [
                "Vec", "String", "format!", "vec!", "Box::new", "Arc::new", "Rc::new", ".to_string()", ".to_vec()", ".collect", ".clone()", "Mutex", "RwLock", ".lock()",
                "thread::sleep", ".recv(", ".join(", "thread::park", "Condvar", "Instant::now", "SystemTime", "CoCreateInstance", "CoInitialize",
            ] {
                // (`Clock::real` and `Shared::new` build their Instant and atoms once, on the interface thread, before the hook exists)
                if name == "shared.rs" && banned == "Instant::now" {
                    continue;
                }
                assert!(!code.contains(banned), "{name} uses {banned}");
            }
        }
    }

    #[test]
    fn nothing_but_hook_rs_installs_a_keyboard_hook() {
        for (name, text) in sources() {
            let code = code_of(text);
            let installs = code.contains("SetWindowsHookEx");
            assert_eq!(installs, name == "hook.rs", "{name}");
        }
    }

    #[test]
    fn no_test_of_the_layer_starts_the_real_hook() {
        // (the needle is built at run time, so that this file does not contain it)
        let real = concat!("Low", "LevelHook");
        for (name, text) in sources() {
            if name == "hook.rs" || name == "mod.rs" {
                continue;
            }
            let tests = text.split("#[cfg(test)]").skip(1).collect::<Vec<_>>().join("\n");
            assert!(!tests.contains(real), "{name}: a test mentions the real hook");
        }
        // and the one test that builds the real service builds it without any setting on
        let svc = include_str!("service.rs");
        let tests = svc.split("#[cfg(test)]").nth(1).unwrap_or_default();
        assert_eq!(tests.matches("KeyService::system(").count(), 1);
        let after = tests.split("KeyService::system(").nth(1).unwrap_or_default();
        assert!(!after.contains("set_wanted"), "the real service is never switched on in a test");
    }
}
