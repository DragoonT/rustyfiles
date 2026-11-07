//! UI language. The Turkish source string is the key; English comes from `i18n_en::EN`.
//!
//! `t!("Yeni klasör")` returns the string for the active language. Everything is looked up at
//! render time (nothing is cached at startup), so switching the language only needs a window
//! refresh. Strings produced on worker threads (e.g. date-group labels in a finished listing)
//! are translated again where they are drawn, via [`td`].

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// Follow the Windows UI language (Turkish → Tr, anything else → En).
    #[default]
    System,
    Tr,
    En,
}

// 1 = Tr, 2 = En (resolved; never System). 0 = not set yet → Tr.
static ACTIVE: AtomicU8 = AtomicU8::new(1);

/// Windows UI language: Turkish → `Tr`, everything else → `En`.
fn system_lang() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    const LANG_TURKISH: u16 = 0x1F;
    let id = unsafe { GetUserDefaultUILanguage() };
    if id & 0x3FF == LANG_TURKISH { Lang::Tr } else { Lang::En }
}

impl Lang {
    /// `System` resolved to a concrete language.
    pub fn resolve(self) -> Lang {
        match self {
            Lang::System => system_lang(),
            other => other,
        }
    }
}

/// Sets the UI language (resolving `System`) and the number-format words in rf-core.
pub fn set(lang: Lang) {
    let l = lang.resolve();
    ACTIVE.store(if l == Lang::En { 2 } else { 1 }, Ordering::Relaxed);
    rf_core::format::set_english(l == Lang::En);
}

/// The active (resolved) language: `Tr` or `En`.
pub fn active() -> Lang {
    if ACTIVE.load(Ordering::Relaxed) == 2 { Lang::En } else { Lang::Tr }
}

pub fn is_en() -> bool {
    active() == Lang::En
}

static EN: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| crate::i18n_en::EN.iter().copied().collect());

/// Looks `tr` up in the English table (None when missing).
fn lookup(tr: &str) -> Option<&'static str> {
    EN.get(tr).copied()
}

/// Translates a Turkish literal. Missing English entries fall back to the Turkish text.
pub fn t(tr: &'static str) -> &'static str {
    if !is_en() {
        return tr;
    }
    match lookup(tr) {
        Some(en) => en,
        None => {
            log::debug!("i18n: missing English text for {tr:?}");
            tr
        }
    }
}

/// Inline pair for the rare Turkish word that needs two different English translations
/// (e.g. "Açık" = "Light" theme vs "On" toggle).
pub fn tx(tr: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { tr }
}

/// Translates a dynamic string that may be a known Turkish text (error messages from the
/// lower crates, group labels, built-in tag names...). `"Klasör bulunamadı: C:\x"` is handled by
/// translating the part before the first `": "`. Unknown strings are returned unchanged.
pub fn td(s: &str) -> String {
    if !is_en() {
        return s.to_string();
    }
    if let Some(en) = lookup(s) {
        return en.to_string();
    }
    if let Some((head, rest)) = s.split_once(": ") {
        if let Some(en) = lookup(head) {
            return format!("{en}: {rest}");
        }
    }
    s.to_string()
}

/// "1 öğe" / "5 öğe" (Tr), "1 item" / "5 items" (En).
/// Percentage in the UI language's order: Turkish "%53", English "53%".
pub fn pct(v: f32) -> String {
    if is_en() { format!("{v:.0}%") } else { format!("%{v:.0}") }
}

pub fn items(n: usize) -> String {
    if is_en() {
        format!("{n} {}", if n == 1 { "item" } else { "items" })
    } else {
        format!("{n} öğe")
    }
}

/// "3 öğe seçildi" / "3 items selected".
pub fn items_selected(n: usize) -> String {
    if is_en() {
        format!("{n} {} selected", if n == 1 { "item" } else { "items" })
    } else {
        format!("{n} öğe seçildi")
    }
}

#[macro_export]
macro_rules! t {
    ($s:literal) => {
        $crate::i18n::t($s)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_keys_unique_and_nonempty() {
        let mut seen = std::collections::HashSet::new();
        for (tr, en) in crate::i18n_en::EN {
            assert!(seen.insert(*tr), "duplicate key {tr:?}");
            assert!(!en.is_empty(), "empty English for {tr:?}");
        }
        assert_eq!(EN.len(), crate::i18n_en::EN.len());
    }

    #[test]
    fn turkish_is_identity_and_english_translates() {
        // One test mutates the global language; keep everything in it.
        set(Lang::Tr);
        for (tr, _) in crate::i18n_en::EN {
            assert_eq!(t(tr), *tr);
        }
        assert_eq!(items(1), "1 öğe");
        set(Lang::En);
        assert_eq!(t("Yeni klasör"), "New folder");
        assert_eq!(items(1), "1 item");
        assert_eq!(items(3), "3 items");
        assert_eq!(items_selected(2), "2 items selected");
        assert_eq!(td("Klasör bulunamadı: C:/x"), "Folder not found: C:/x");
        assert_eq!(td("kullanıcı dosyası.txt"), "kullanıcı dosyası.txt");
        assert_eq!(rf_core::format::size(1536), "1.50 KB");
        set(Lang::Tr);
        assert_eq!(rf_core::format::size(1536), "1,50 KB");
    }
}
