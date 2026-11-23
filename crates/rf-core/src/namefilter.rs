//! What a search text asks of a file's name: a part of the name (or a `*`/`?` pattern), and, with `ext:`, its extension.
//!
//! `ext:pdf`, `ext:.pdf`, `ext:jpg,png` and `ext:jpg;png` name extensions; the words beside it are the name part as before
//! (`rapor ext:pdf,docx`). A text may hold several `ext:` words: their extensions add up. The filter of the open folder, the search
//! of the subfolders on the disk and the one in the name index all read a text through this, so the three agree. (`kind:video` is
//! turned into such a list by the app before it gets here: which extensions a kind has is the app's table.)

/// The parts of a search text (see the module).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameFilter {
    /// What is left for the name, trimmed: a substring, or a pattern when it holds `*` or `?`. Empty: every name.
    pub name: String,
    /// The extensions asked for, lower case, without the dot. Empty: any.
    pub exts: Vec<String>,
}

/// Does `text` hold an `ext:` word? (The cheap question, asked before [`parse`] where every name of a folder is tested.)
pub fn has_ext_word(text: &str) -> bool {
    text.split_whitespace().any(|w| ext_list(w).is_some())
}

/// The list of an `ext:` word (`None` for any other word, and for `ext:` with nothing after it: that is a name being typed).
fn ext_list(word: &str) -> Option<&str> {
    let head = word.get(..4)?;
    let rest = &word[4..];
    (head.eq_ignore_ascii_case("ext:") && !rest.trim_matches(['.', ',', ';', '=']).is_empty()).then_some(rest)
}

pub fn parse(text: &str) -> NameFilter {
    let mut exts: Vec<String> = Vec::new();
    let mut name: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        match ext_list(word) {
            Some(list) => {
                // ("ext:=pdf" too: Windows' own search writes its filters with "=")
                for e in list.trim_start_matches('=').split([',', ';']) {
                    let e = e.trim().trim_start_matches(['*', '.']).to_lowercase();
                    if !e.is_empty() && !exts.contains(&e) {
                        exts.push(e);
                    }
                }
            }
            None => name.push(word),
        }
    }
    NameFilter { name: name.join(" "), exts }
}

impl NameFilter {
    /// Is the extension of `name` one of those asked for (any, when none was)? A name with no extension, and one that is only an
    /// extension (`.gitignore`), has none.
    pub fn ext_ok(&self, name: &str) -> bool {
        if self.exts.is_empty() {
            return true;
        }
        let Some((stem, ext)) = name.rsplit_once('.') else { return false };
        !stem.is_empty() && self.exts.iter().any(|e| e.len() == ext.len() && e.chars().zip(ext.chars()).all(|(a, b)| b.to_lowercase().eq(std::iter::once(a))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_words_are_taken_out_of_the_text() {
        assert_eq!(parse("rapor ext:pdf"), NameFilter { name: "rapor".into(), exts: vec!["pdf".into()] });
        assert_eq!(parse("ext:.JPG,png;*.gif  yaz tatili"), NameFilter { name: "yaz tatili".into(), exts: vec!["jpg".into(), "png".into(), "gif".into()] });
        assert_eq!(parse("EXT:=mp4 ext:mov ext:mp4").exts, ["mp4", "mov"], "several words add up, each extension once");
        // nothing to take out
        assert_eq!(parse("  my text.txt "), NameFilter { name: "my text.txt".into(), exts: vec![] });
        assert_eq!(parse("ext:"), NameFilter { name: "ext:".into(), exts: vec![] }, "still being typed: a name");
        assert_eq!(parse("next:pdf").exts, Vec::<String>::new());
        assert!(has_ext_word("a ext:pdf") && !has_ext_word("a.ext b") && !has_ext_word("ext:") && !has_ext_word("ext:."));
    }

    #[test]
    fn the_extension_of_a_name_is_compared_without_case() {
        let f = parse("ext:pdf,jpeg");
        assert!(f.ext_ok("Rapor.PDF") && f.ext_ok("a.b.Jpeg"));
        assert!(!f.ext_ok("rapor.pdf.txt") && !f.ext_ok("pdf") && !f.ext_ok(".pdf") && !f.ext_ok("rapor.pd"));
        assert!(parse("rapor").ext_ok("anything"), "no extension asked for: any");
        let tr = parse("ext:İŞ");
        assert!(tr.ext_ok("a.i̇ş"));
    }
}
