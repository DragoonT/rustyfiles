//! The words of the guard's one message box. The guard has no UI toolkit and does not link the app,
//! so it carries its own Turkish / English pair instead of the app's `t!` keys; the language is
//! the Windows display language.

use crate::machine::Message;

/// The offline repair line (plan 6.3, route 4).
pub const REPAIR_LINE: &str = r#"reg delete "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell /f"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Tr,
    En,
}

pub const TITLE: &str = "rustyfiles";

pub fn body(m: Message, lang: Lang) -> String {
    let value = match lang {
        Lang::En => format!(
            "rustyfiles could not remove its Shell setting, so the next sign-in would start it again. To switch it off, open a command prompt and run:\n\n{REPAIR_LINE}\n\nthen sign out."
        ),
        Lang::Tr => format!(
            "rustyfiles Kabuk ayarını kaldıramadı; bir sonraki oturum açışta yeniden başlatılır. Kapatmak için bir komut istemi açıp şunu çalıştırın:\n\n{REPAIR_LINE}\n\nSonra oturumu kapatın."
        ),
    };
    let explorer = match lang {
        Lang::En => "Explorer did not start. Press Ctrl+Shift+Esc, choose Run new task and run explorer.exe.",
        Lang::Tr => "Gezgin (Explorer) başlamadı. Ctrl+Shift+Esc tuşlarına basın, Yeni görev çalıştır'ı seçin ve explorer.exe çalıştırın.",
    };
    match (m.value_left, m.explorer_down) {
        (true, true) => format!("{explorer}\n\n{value}"),
        (true, false) => value,
        (false, true) => explorer.to_string(),
        (false, false) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repair_line_is_in_every_text_that_leaves_the_value_set() {
        for lang in [Lang::Tr, Lang::En] {
            for down in [false, true] {
                let t = body(Message { value_left: true, explorer_down: down }, lang);
                assert!(t.contains(REPAIR_LINE), "{lang:?} {down}");
            }
            let only_explorer = body(Message { value_left: false, explorer_down: true }, lang);
            assert!(!only_explorer.contains("reg delete") && only_explorer.contains("explorer.exe"));
        }
    }

    #[test]
    fn the_two_languages_differ_and_nothing_is_empty_when_a_box_is_shown() {
        let m = Message { value_left: true, explorer_down: true };
        assert_ne!(body(m, Lang::Tr), body(m, Lang::En));
        assert!(body(Message { value_left: false, explorer_down: false }, Lang::En).is_empty());
    }

    #[test]
    fn the_repair_line_is_what_the_plan_prints() {
        assert_eq!(REPAIR_LINE, "reg delete \"HKCU\\Software\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon\" /v Shell /f");
    }
}
