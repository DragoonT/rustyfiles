//! The words of the Recycle Bin's new commands, as pure functions of the counts and results they tell
//! (tested without a window): the "delete for good" confirmation, the line that says what happened, the
//! bin settings' problems, and the refusal lines of the shortcut key field.

use crate::t;
use rf_core::format;
use rf_core::hotkey::Refusal;
use rf_core::recycle_bin::{DeleteFacts, DeleteReport, Problem};

pub fn delete_title() -> &'static str {
    t!("Kalıcı olarak silinsin mi?")
}

/// The confirmation's message: how many items (the name when it is one), how much data, that it cannot be undone.
/// `name` is the item's name when `facts.count == 1`.
pub fn delete_message(facts: DeleteFacts, name: Option<&str>) -> String {
    let subject = match name {
        Some(n) if facts.count == 1 => format!("\"{n}\""),
        _ => crate::i18n::items(facts.count),
    };
    let mut s = format!("{subject} {}", t!("Geri Dönüşüm Kutusu'ndan kalıcı olarak silinecek."));
    let mut detail = Vec::new();
    if facts.count > 1 {
        detail.push(crate::i18n::items(facts.count));
    }
    if facts.folders > 0 {
        detail.push(format!("{} {}", t!("Aralarındaki klasör sayısı:"), facts.folders));
    }
    if facts.bytes > 0 {
        detail.push(format::size(facts.bytes));
    }
    if !detail.is_empty() {
        s.push('\n');
        s.push_str(&detail.join(" · "));
    }
    s.push('\n');
    s.push_str(t!("Bu işlem geri alınamaz."));
    s
}

/// What the bin view's status line says after a deletion, and whether it is a problem (shown as one).
pub fn delete_status(r: &DeleteReport) -> (String, bool) {
    if r.requested == 0 && r.error.is_none() {
        return (t!("Silinecek öğe Geri Dönüşüm Kutusu'nda bulunamadı").to_string(), true);
    }
    let mut parts = Vec::new();
    if r.unknown {
        // the request may still run, or may have run: neither "deleted" nor "not deleted" is said
        parts.push(t!("Sonuç bilinmiyor; öğeler silinmiş olabilir").to_string());
    }
    if r.deleted > 0 {
        parts.push(format!("{} {}", crate::i18n::items(r.deleted), t!("kalıcı olarak silindi")));
    }
    if r.left > 0 {
        parts.push(format!("{} {}", crate::i18n::items(r.left), t!("silinemedi")));
    }
    if r.collateral > 0 {
        parts.push(format!("{} {}", crate::i18n::items(r.collateral), t!("istenmediği halde kutudan kalktı")));
    }
    if parts.is_empty() {
        parts.push(t!("Hiçbir öğe silinmedi").to_string());
    }
    let mut s = parts.join(" · ");
    if let Some(e) = &r.error {
        s.push_str(": ");
        s.push_str(e);
    }
    (s, !r.all_done())
}

/// One line for a size that cannot be saved. `capacity_mb` is the drive's size in MB (0 = unknown).
pub fn problem_text(p: Problem) -> String {
    match p {
        Problem::NotANumber => t!("Boyut, megabayt olarak bir tam sayı olmalı").to_string(),
        Problem::TooSmall => t!("Boyut en az 1 MB olmalı").to_string(),
        Problem::TooBig(0) => t!("Boyut çok büyük").to_string(),
        Problem::TooBig(cap) => format!("{} {}", t!("Boyut sürücüden büyük olamaz:"), format::size(cap * 1024 * 1024)),
    }
}

/// The one line the shortcut key field shows when a combination is not taken.
pub fn refusal_text(r: Refusal) -> &'static str {
    match r {
        Refusal::OnlyModifier => t!("Ctrl, Alt ya da Shift tek başına olmaz; bir tuşa da basın"),
        Refusal::WinKey => t!("Windows tuşu kısayol tuşunda kullanılamaz"),
        Refusal::KeyNotAllowed => t!("Bu tuş kısayol tuşu olamaz"),
        Refusal::Reserved => t!("Windows bu tuş birleşimini kendine ayırmış"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_confirmation_names_one_item_and_counts_many() {
        let one = delete_message(DeleteFacts { count: 1, folders: 0, bytes: 0 }, Some("rapor.pdf"));
        assert!(one.starts_with("\"rapor.pdf\" "), "{one}");
        assert_eq!(one.lines().count(), 2, "{one}");
        let many = delete_message(DeleteFacts { count: 12, folders: 3, bytes: 5 * 1024 * 1024 }, None);
        assert!(many.starts_with("12 "), "{many}");
        assert!(many.contains(" · "), "{many}");
        assert!(many.contains('3'), "{many}");
        assert_eq!(many.lines().count(), 3, "{many}");
        // a count of one with no name (the name is unknown): still the count
        let nameless = delete_message(DeleteFacts { count: 1, folders: 0, bytes: 0 }, None);
        assert!(nameless.starts_with("1 "), "{nameless}");
        // a name given for many items is not used
        let many_named = delete_message(DeleteFacts { count: 2, folders: 0, bytes: 0 }, Some("x"));
        assert!(!many_named.contains("\"x\""));
    }

    #[test]
    // (the numbers are looked for without the word that follows them: another test walks through every interface language at the
    // same time, and in some of them the count is not followed by a space: "항목 2개")
    fn the_status_line_counts_what_happened() {
        let ok = DeleteReport { requested: 3, deleted: 3, left: 0, collateral: 0, error: None, unknown: false };
        let (s, problem) = delete_status(&ok);
        assert!(s.starts_with("3 "), "{s}");
        assert!(!problem);
        let part = DeleteReport { requested: 3, deleted: 2, left: 1, collateral: 0, error: Some("in use".into()), unknown: false };
        let (s, problem) = delete_status(&part);
        assert!(s.contains('2') && s.contains('1') && s.ends_with(": in use"), "{s}");
        assert!(problem);
        let none = DeleteReport::default();
        assert!(delete_status(&none).1);
        let unknown = DeleteReport { requested: 2, deleted: 0, left: 0, collateral: 0, error: Some("timeout".into()), unknown: true };
        let (s, problem) = delete_status(&unknown);
        assert!(problem && s.ends_with(": timeout") && !s.contains('0'), "{s}");
        let extra = DeleteReport { requested: 1, deleted: 1, left: 0, collateral: 2, error: None, unknown: false };
        let (s, problem) = delete_status(&extra);
        assert!(problem && s.contains('2'), "{s}");
    }

    #[test]
    fn every_problem_and_refusal_has_a_line() {
        for p in [Problem::NotANumber, Problem::TooSmall, Problem::TooBig(0), Problem::TooBig(2048)] {
            let s = problem_text(p);
            assert!(!s.is_empty() && !s.contains('\n'), "{s}");
        }
        assert!(problem_text(Problem::TooBig(2048)).contains("2"));
        for r in [Refusal::OnlyModifier, Refusal::WinKey, Refusal::KeyNotAllowed, Refusal::Reserved] {
            assert!(!refusal_text(r).is_empty() && !refusal_text(r).contains('\n'));
        }
    }
}
