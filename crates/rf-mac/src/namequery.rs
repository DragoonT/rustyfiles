//! What a typed search or filter text asks of a name, for the open folder's filter and the
//! search of the subfolders alike (rf-app does the same with rf_core::namefilter and its
//! search_kinds.rs): `*` / `?` make the name part a pattern ("*.pdf"), else every word must be
//! in the name; `ext:pdf,docx` asks for extensions; `kind:video` (also `type:`, `tür:`, `art:`…,
//! with or without "=") stands for the extensions of that kind, so it works in every folder.

use rf_core::entry::{contains_ignore_case, matches_patterns};

/// The words for "kind" (as Windows' search spells it in a few languages).
const KEYS: [&str; 13] = ["kind", "type", "tür", "tur", "art", "typ", "tipo", "clase", "genre", "soort", "rodzaj", "вид", "тип"];

const IMAGE: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "svg", "ico", "icns", "raw", "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "psd", "psb"];
const VIDEO: &[&str] = &["mp4", "m4v", "mov", "mkv", "avi", "wmv", "webm", "flv", "mpg", "mpeg", "m2ts", "mts", "ts", "3gp", "ogv", "vob"];
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "flac", "wav", "aiff", "aif", "ogg", "opus", "wma", "alac", "mid", "midi", "caf"];
const DOCUMENT: &[&str] = &["doc", "docx", "odt", "rtf", "pages", "key", "ppt", "pptx", "odp", "epub"];
const SHEET: &[&str] = &["xls", "xlsx", "xlsm", "ods", "numbers", "csv", "tsv"];
const PDF: &[&str] = &["pdf"];
const TEXT: &[&str] = &["txt", "md", "markdown", "log", "text", "nfo"];
const ARCHIVE: &[&str] = &["zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "iso", "dmg", "cab", "lz", "lzma"];
const APP: &[&str] = &["app", "pkg", "dmg", "exe", "msi", "command", "sh"];
const CODE: &[&str] = &[
    "rs", "swift", "c", "h", "cpp", "hpp", "cc", "m", "mm", "js", "ts", "tsx", "jsx", "py", "rb", "go", "java", "kt", "cs", "php", "html",
    "css", "scss", "json", "toml", "yaml", "yml", "xml", "sql", "lua", "dart", "zig",
];

fn kind_exts(word: &str) -> Option<Vec<&'static str>> {
    let lists: &[&[&str]] = match word {
        "picture" | "pictures" | "image" | "images" | "photo" | "photos" | "pic" | "pics" | "resim" | "resimler" | "fotoğraf" | "fotograf"
        | "görüntü" | "görsel" | "bild" | "bilder" | "foto" | "fotos" | "imagen" | "imagem" | "immagine" | "afbeelding" | "obraz"
        | "изображение" | "фото" => &[IMAGE],
        "video" | "videos" | "movie" | "movies" | "film" | "filme" | "vídeo" | "видео" | "wideo" => &[VIDEO],
        "music" | "audio" | "song" | "songs" | "sound" | "müzik" | "muzik" | "ses" | "şarkı" | "musik" | "música" | "musica" | "musique"
        | "muziek" | "muzyka" | "музыка" => &[AUDIO],
        "document" | "documents" | "doc" | "docs" | "belge" | "belgeler" | "doküman" | "dokument" | "dokumente" | "documento"
        | "documentos" | "документ" => &[DOCUMENT, SHEET, PDF, TEXT],
        "archive" | "archives" | "zip" | "compressed" | "arşiv" | "arsiv" | "archiv" | "archivo" | "arquivo" | "архив" => &[ARCHIVE],
        "program" | "programs" | "app" | "apps" | "application" | "uygulama" | "programm" | "programme" | "programa" | "программа" => &[APP],
        "code" | "kod" | "código" | "codigo" | "код" => &[CODE],
        "text" | "txt" | "metin" | "texto" | "texte" | "текст" => &[TEXT],
        "spreadsheet" | "spreadsheets" | "sheet" | "table" | "tablo" | "tabelle" => &[SHEET],
        "pdf" => &[PDF],
        _ => return None,
    };
    Some(lists.iter().flat_map(|l| l.iter().copied()).collect())
}

/// `key:value` or `key:=value` naming a kind: that kind's extensions.
fn kind_word(word: &str) -> Option<Vec<&'static str>> {
    let (key, value) = word.split_once(':')?;
    if !KEYS.contains(&key.to_lowercase().as_str()) {
        return None;
    }
    kind_exts(&value.trim_start_matches('=').to_lowercase())
}

/// The text with each kind word turned into its `ext:` list.
pub(crate) fn expand(text: &str) -> String {
    if !text.contains(':') || !text.split_whitespace().any(|w| kind_word(w).is_some()) {
        return text.to_string();
    }
    text.split_whitespace().map(|w| kind_word(w).map_or_else(|| w.to_string(), |e| format!("ext:{}", e.join(",")))).collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug)]
pub(crate) struct NameQuery {
    pattern: Option<String>,
    words: Vec<String>,
    exts: rf_core::namefilter::NameFilter,
}

impl NameQuery {
    pub(crate) fn parse(text: &str) -> NameQuery {
        let f = rf_core::namefilter::parse(&expand(text.trim()));
        let name = f.name.trim().to_string();
        let pattern = name.contains(['*', '?']).then(|| name.clone());
        let words = if pattern.is_some() { Vec::new() } else { name.split_whitespace().map(str::to_string).collect() };
        NameQuery { pattern, words, exts: f }
    }

    /// Nothing asked for: every name.
    pub(crate) fn is_empty(&self) -> bool {
        self.pattern.is_none() && self.words.is_empty() && self.exts.exts.is_empty()
    }

    pub(crate) fn matches(&self, name: &str) -> bool {
        if !self.exts.ext_ok(name) {
            return false;
        }
        match &self.pattern {
            Some(p) => matches_patterns(p, name),
            None => self.words.iter().all(|w| contains_ignore_case(name, w)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_extensions_patterns_and_words() {
        let q = NameQuery::parse("kind:video tatil");
        assert!(q.matches("Tatil 2024.MP4") && !q.matches("tatil.jpg") && !q.matches("iş.mov"));
        for t in ["art:=video", "tür:video", "Kind:=Videos", "TYPE:film"] {
            assert!(NameQuery::parse(t).matches("x.mov") && !NameQuery::parse(t).matches("x.png"), "{t}");
        }
        assert!(NameQuery::parse("ext:pdf,docx rapor").matches("Rapor 3.PDF"));
        assert!(!NameQuery::parse("ext:pdf").matches("rapor.txt"));
        assert!(NameQuery::parse("*.md").matches("README.md") && !NameQuery::parse("*.md").matches("a.mdx"));
        assert!(NameQuery::parse("rapor 2024").matches("2024 yıllık rapor.pdf"));
        assert!(NameQuery::parse("  ").is_empty() && NameQuery::parse("kind:unknownthing").matches("my kind:unknownthing file"));
    }
}
