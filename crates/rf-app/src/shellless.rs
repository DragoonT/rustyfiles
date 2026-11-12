//! What the app says when something fails because the Windows shell (Explorer) is not running, or does not answer.
//!
//! Before this, the Start panel said "Başlatılamadı. Kaldırılmış ya da taşınmış olabilir." ("may have been uninstalled or moved") for
//! EVERY failed start, including "Sınıf kaydedilmemiş (0x80040154)" from a shell that is not there: the user's trial without Explorer
//! was told the apps had been removed. And a file operation that needs the shell's engine showed the engine's raw English text. The
//! texts here say what happened. The choice of text is pure ([`classify_start`], tested); the words are Turkish with an English pair.

use rf_win::engine_guard::{EngineStalled, NoShellEngine, Op, PartlyDone};

/// What a failed start of an app was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartKind {
    /// "Class not registered" (0x80040154) and no shell window exists: the shell's part of the start is not there.
    NoShell,
    /// "Class not registered" with a shell running: a component this entry needs is missing.
    ClassMissing,
    /// File or path not found: the program or shortcut is gone.
    Gone,
    /// The shell did not answer within the limit.
    Busy,
    Other,
}

/// The kind of a failed start, from the error's text (the start code joins several attempts' errors into one text) and whether a shell
/// window exists now.
pub fn classify_start(error_text: &str, shell_present: bool) -> StartKind {
    let l = error_text.to_ascii_lowercase();
    if l.contains("0x80040154") {
        return if shell_present { StartKind::ClassMissing } else { StartKind::NoShell };
    }
    if l.contains("0x80070002") || l.contains("0x80070003") || l.contains("0x8007007b") {
        return StartKind::Gone;
    }
    if l.contains("did not answer") {
        return StartKind::Busy;
    }
    StartKind::Other
}

/// The sentence for the Start panel's notice line after a failed start.
pub fn start_failure_text(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    match classify_start(&text, rf_win::engine_guard::shell_present()) {
        StartKind::NoShell => crate::t!("Başlatılamadı: Windows kabuğu (Explorer) çalışmıyor ve Windows bu girdiyi kabuksuz açamadı.").to_string(),
        StartKind::ClassMissing => crate::t!("Başlatılamadı: Windows bu girdi için gereken bileşeni bulamadı (sınıf kayıtlı değil).").to_string(),
        StartKind::Gone => crate::t!("Başlatılamadı. Kaldırılmış ya da taşınmış olabilir.").to_string(),
        StartKind::Busy => crate::t!("Başlatılamadı: Windows kabuğu zamanında yanıt vermedi.").to_string(),
        StartKind::Other => format!("{} {}", crate::t!("Başlatılamadı:"), brief(&text)),
    }
}

/// The part of an error text that fits a notice line: its last attempt's reason, without the machinery in front.
fn brief(text: &str) -> String {
    let last = text.rsplit("; ").next().unwrap_or(text);
    // (the start code labels each attempt: "the shell: ...", "directly: ...")
    let last = ["the shell: ", "directly: "].iter().find_map(|p| last.strip_prefix(p)).unwrap_or(last);
    let one_line: String = last.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 90 { format!("{}…", one_line.chars().take(89).collect::<String>()) } else { one_line }
}

/// What the Status Center shows for a failed file operation: a true sentence for the failures the shell's absence causes, the error's
/// own text for everything else.
pub fn op_failure_text(err: &anyhow::Error) -> String {
    if let Some(n) = err.downcast_ref::<NoShellEngine>() {
        return match n.op {
            Op::RecycleDelete => crate::t!("Geri Dönüşüm Kutusu'na taşıma Windows kabuğunu (Explorer) gerektiriyor ve kabuk çalışmıyor. Hiçbir şey silinmedi; kalıcı silme kullanılabilir.").to_string(),
            Op::Copy | Op::Move => crate::t!("Bu kopyalama ya da taşıma Windows kabuğunu (Explorer) gerektiriyor ve kabuk çalışmıyor. Hiçbir şey değiştirilmedi.").to_string(),
            Op::RestoreFromBin | Op::DeleteFromBin => crate::t!("Geri Dönüşüm Kutusu işlemi Windows kabuğunu (Explorer) gerektiriyor ve kabuk çalışmıyor. Hiçbir şey değiştirilmedi.").to_string(),
            Op::PermanentDelete | Op::Rename => crate::t!("Bazı öğeler silinemedi (kilitli, salt okunur ya da izin yok) ve ayrıntıyı gösterecek Windows kabuğu (Explorer) çalışmıyor.").to_string(),
        };
    }
    // our own engine did its part first: the sentence says how much is left and that the rest was done (never "nothing was changed")
    if let Some(p) = err.downcast_ref::<PartlyDone>() {
        let n = p.left.unwrap_or(0).to_string();
        return match p.op {
            Op::Copy => crate::t!("{n} öğe kopyalanamadı: bunun için Windows kabuğu (Explorer) gerekiyor ve kabuk çalışmıyor. Geri kalan her şey yapıldı.").replace("{n}", &n),
            Op::Move => crate::t!("{n} öğe taşınamadı: bunun için Windows kabuğu (Explorer) gerekiyor ve kabuk çalışmıyor. Geri kalan her şey yapıldı.").replace("{n}", &n),
            _ => crate::t!("Bazı öğeler silinemedi (kilitli, salt okunur ya da izin yok); geri kalanı silindi. Ayrıntıyı gösterecek Windows kabuğu (Explorer) çalışmıyor.").to_string(),
        };
    }
    if let Some(s) = err.downcast_ref::<EngineStalled>() {
        return crate::t!("Windows'un dosya motoru {n} saniyede başlamadı; Explorer kapanmış ya da takılmış olabilir. İşlem yine de bitebilir: Windows'un kendi penceresine bakın.").replace("{n}", &s.waited.as_secs().to_string());
    }
    format!("{err:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trials_error_with_no_shell_is_a_missing_shell_and_not_a_moved_app() {
        let text = "Sınıf kaydedilmemiş. (0x80040154); directly: activation: Sınıf kaydedilmemiş. (0x80040154)";
        assert_eq!(classify_start(text, false), StartKind::NoShell);
        // with a shell the same code means a component is missing
        assert_eq!(classify_start(text, true), StartKind::ClassMissing);
    }

    #[test]
    fn a_missing_file_is_the_only_start_that_says_moved_or_removed() {
        assert_eq!(classify_start("Sistem belirtilen dosyayı bulamıyor. (0x80070002)", true), StartKind::Gone);
        assert_eq!(classify_start("The system cannot find the path specified. (0x80070003)", false), StartKind::Gone);
        assert_eq!(classify_start("Erişim engellendi. (0x80070005)", true), StartKind::Other);
        assert_eq!(classify_start("the shell did not answer in 15s", true), StartKind::Busy);
    }

    #[test]
    fn an_unknown_failure_shows_its_own_last_reason_in_one_short_line() {
        assert_eq!(brief("first attempt failed; the shell: Erişim engellendi. (0x80070005)"), "Erişim engellendi. (0x80070005)");
        let long = "x ".repeat(200);
        assert!(brief(&long).chars().count() <= 90);
        assert_eq!(brief("a\n  b"), "a b");
    }

    #[test]
    fn the_file_engine_errors_have_their_own_sentences() {
        let e: anyhow::Error = NoShellEngine { op: Op::RecycleDelete }.into();
        assert_ne!(op_failure_text(&e), format!("{e:#}"), "the raw English text is not shown as it is");
        let s: anyhow::Error = EngineStalled { op: Op::Copy, waited: std::time::Duration::from_secs(60) }.into();
        assert!(op_failure_text(&s).contains("60"));
        // an operation our engine did part of is told as partly done, with the count
        let p: anyhow::Error = PartlyDone { op: Op::Move, left: Some(3) }.into();
        let text = op_failure_text(&p);
        assert!(text.contains('3') && !text.contains("{n}") && text.contains("Geri kalan"), "{text}");
        assert!(!text.contains("Hiçbir şey değiştirilmedi"), "{text}");
        let d: anyhow::Error = PartlyDone { op: Op::PermanentDelete, left: None }.into();
        assert!(op_failure_text(&d).contains("geri kalanı silindi"));
        // anything else is its own text
        assert_eq!(op_failure_text(&anyhow::anyhow!("disk full")), "disk full");
    }
}
