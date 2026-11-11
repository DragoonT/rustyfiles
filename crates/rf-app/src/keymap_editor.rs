//! Shortcut editor (Settings → Keyboard shortcuts): click a command, press the new keys.
//!
//! Capturing uses a keystroke interceptor, so the pressed keys never reach their current action
//! (pressing Ctrl+N while recording does not open a window). Escape cancels. The new binding is
//! stored in `settings.toml` → `[keybindings]` and bound immediately; bindings are additive, so
//! the previous keys keep working until restart after a reset.

use gpui_kit::*;

/// The command (short action name, e.g. "NewFolder") waiting for keys and when recording started (a forgotten capture expires).
#[derive(Default)]
pub struct KeyCapture(pub Option<(String, std::time::Instant)>);
impl Global for KeyCapture {}

const MODIFIER_KEYS: [&str; 6] = ["control", "shift", "alt", "platform", "function", ""];

/// GPUI keystroke string (`ctrl-shift-k`) from a keystroke, or `None` for a modifier-only press.
fn keys_of(k: &Keystroke) -> Option<String> {
    if MODIFIER_KEYS.contains(&k.key.as_str()) {
        return None;
    }
    let m = &k.modifiers;
    let mut s = String::new();
    if m.control {
        s.push_str("ctrl-");
    }
    if m.alt {
        s.push_str("alt-");
    }
    if m.shift {
        s.push_str("shift-");
    }
    if m.platform {
        s.push_str("win-");
    }
    s.push_str(&k.key);
    Some(s)
}

/// "ctrl-shift-k" → "Ctrl+Shift+K" (as shown in the UI).
pub fn pretty(keys: &str) -> String {
    // "ctrl--" means Ctrl + minus: keep the trailing '-' as the key
    if let Some(mods) = keys.strip_suffix("--") {
        let head = pretty(mods);
        return if head.is_empty() { "-".into() } else { format!("{head}+-") };
    }
    keys.split('-')
        .map(|p| match p {
            "ctrl" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            "win" => "Win".to_string(),
            k if k.chars().count() == 1 => k.to_uppercase(),
            k => {
                let mut c = k.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Installs the interceptor once at startup.
pub fn init(cx: &mut App) {
    cx.set_global(KeyCapture::default());
    cx.intercept_keystrokes(|ev, window, cx| {
        let Some((action, started)) = cx.global::<KeyCapture>().0.clone() else { return };
        let quick_search = action == crate::quick_search::CAPTURE_ACTION;
        if started.elapsed() > std::time::Duration::from_secs(15) {
            // left Settings without pressing anything: give the keyboard back
            cx.global_mut::<KeyCapture>().0 = None;
            if quick_search {
                crate::quick_search::resync(cx);
            }
            return;
        }
        cx.stop_propagation();
        if ev.keystroke.key == "escape" && !ev.keystroke.modifiers.modified() {
            cx.global_mut::<KeyCapture>().0 = None;
            if quick_search {
                crate::quick_search::resync(cx);
            }
            window.refresh();
            return;
        }
        let Some(keys) = keys_of(&ev.keystroke) else { return };
        cx.global_mut::<KeyCapture>().0 = None;
        if quick_search {
            // the quick search's global shortcut: a setting, not an action binding
            crate::settings::update(cx, |s| s.quick_search_keys = keys.clone());
            crate::quick_search::resync(cx);
            window.refresh();
            return;
        }
        crate::settings::update(cx, |s| {
            // one custom shortcut per command: replace an earlier one
            s.keybindings.retain(|_, a| *a != action);
            s.keybindings.insert(keys.clone(), action.clone());
        });
        // bind just the new key (re-binding the whole map would stack duplicates)
        match cx.build_action(&format!("rustyfiles::{action}"), None) {
            Ok(a) => match KeyBinding::load(&keys, a, None, false, None, cx) {
                Ok(b) => cx.bind_keys([b]),
                Err(e) => log::warn!("keybinding {keys:?}: {e}"),
            },
            Err(e) => log::warn!("keybinding action {action:?}: {e}"),
        }
        window.refresh();
    })
    .detach();
}

pub fn start(action: String, cx: &mut App) {
    let quick_search = action == crate::quick_search::CAPTURE_ACTION;
    // recording the quick search's shortcut was given up for another command: it comes back
    if !quick_search && capturing(cx).as_deref() == Some(crate::quick_search::CAPTURE_ACTION) {
        crate::quick_search::resync(cx);
    }
    let started = std::time::Instant::now();
    if quick_search {
        // the current global shortcut would swallow the keys being recorded
        crate::quick_search::release_for_capture();
        // left without pressing anything (the window closed, another app came up): the
        // shortcut comes back by itself, it would otherwise stay off until a key reaches us
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(16)).await;
            cx.update(|cx| {
                let still = cx.global::<KeyCapture>().0.as_ref().is_some_and(|(a, t)| a == crate::quick_search::CAPTURE_ACTION && *t == started);
                if still {
                    cx.global_mut::<KeyCapture>().0 = None;
                    crate::quick_search::resync(cx);
                    cx.refresh_windows();
                }
            });
        })
        .detach();
    }
    cx.global_mut::<KeyCapture>().0 = Some((action, started));
}

pub fn capturing(cx: &App) -> Option<String> {
    cx.try_global::<KeyCapture>().and_then(|c| c.0.as_ref().map(|(a, _)| a.clone()))
}

/// User override for a command, if any (keys in GPUI syntax).
pub fn custom_keys(action: &str, cx: &App) -> Option<String> {
    cx.global::<crate::settings::Settings>()
        .keybindings
        .iter()
        .find(|(_, a)| a.as_str() == action || a.strip_prefix("rustyfiles::") == Some(action))
        .map(|(k, _)| k.clone())
}

/// A shortcut as compared between commands: "Ctrl+Shift+K" and "ctrl-shift-k" are the same,
/// as are the arrows and Del of the built-in list and the names GPUI gives those keys.
pub fn canon(shown: &str) -> String {
    shown
        .to_lowercase()
        .split('+')
        .map(|k| match k.trim() {
            "←" => "left",
            "→" => "right",
            "↑" => "up",
            "↓" => "down",
            "del" => "delete",
            k => k,
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Commands whose keys another command has too: action name → the other command's label.
/// `entries` is (label, built-in keys as shown, action name); a custom shortcut replaces the
/// built-in one in the comparison.
pub fn conflicts(entries: &[(&'static str, &'static str, String)], custom: &dyn Fn(&str) -> Option<String>) -> std::collections::HashMap<String, &'static str> {
    let keys: Vec<Option<String>> = entries
        .iter()
        .map(|(_, built_in, action)| match custom(action) {
            Some(k) => Some(canon(&pretty(&k))),
            None if built_in.is_empty() => None,
            None => Some(canon(built_in)),
        })
        .collect();
    let mut out = std::collections::HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        let Some(k) = k else { continue };
        // only shortcuts the user set are reported: the built-in list may share a key between
        // commands that apply in different places
        if let Some(j) = (0..keys.len()).find(|&j| j != i && keys[j].as_ref() == Some(k) && (custom(&entries[i].2).is_some() || custom(&entries[j].2).is_some())) {
            out.insert(entries[i].2.clone(), entries[j].0);
        }
    }
    out
}

pub fn reset(action: &str, cx: &mut App) {
    let action = action.to_string();
    crate::settings::update(cx, |s| {
        s.keybindings.retain(|_, a| *a != action && a.strip_prefix("rustyfiles::") != Some(action.as_str()))
    });
}

#[cfg(test)]
mod tests {
    use super::pretty;

    #[test]
    fn a_custom_shortcut_on_another_commands_keys_is_reported() {
        let entries = [("Yeni sekme", "Ctrl+T", "NewTab".to_string()), ("Geri", "Alt+←", "GoBack".to_string()), ("Rafı göster/gizle", "", "ToggleShelf".to_string())];
        let none = |_: &str| None;
        assert!(super::conflicts(&entries, &none).is_empty());
        // the shelf gets Ctrl+T: both commands are marked, each naming the other
        let shelf = |a: &str| (a == "ToggleShelf").then(|| "ctrl-t".to_string());
        let c = super::conflicts(&entries, &shelf);
        assert_eq!(c.get("ToggleShelf"), Some(&"Yeni sekme"));
        assert_eq!(c.get("NewTab"), Some(&"Rafı göster/gizle"));
        assert_eq!(c.len(), 2);
        // an arrow key, written the way GPUI names it
        let back = |a: &str| (a == "ToggleShelf").then(|| "alt-left".to_string());
        assert_eq!(super::conflicts(&entries, &back).get("GoBack"), Some(&"Rafı göster/gizle"));
        // moving a command away from its built-in keys frees them
        let moved = |a: &str| match a {
            "NewTab" => Some("ctrl-alt-n".to_string()),
            "ToggleShelf" => Some("ctrl-t".to_string()),
            _ => None,
        };
        assert!(super::conflicts(&entries, &moved).is_empty());
    }

    #[test]
    fn pretty_keys() {
        assert_eq!(pretty("ctrl-shift-k"), "Ctrl+Shift+K");
        assert_eq!(pretty("alt-f4"), "Alt+F4");
        assert_eq!(pretty("ctrl-pageup"), "Ctrl+Pageup");
        assert_eq!(pretty("ctrl--"), "Ctrl+-");
    }
}
