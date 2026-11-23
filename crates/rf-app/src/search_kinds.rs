//! `kind:video` in a search text, as Explorer has it (asked for on Discord: "search by kind like Explorer's art:=video").
//!
//! Windows' own search understands it only inside folders its index covers, and only with the keywords of the system's language.
//! Here a kind word is turned into the list of that kind's extensions (`ext:mp4,mov,…`, see `rf_core::namefilter`), which the
//! filter of the open folder, our name index and the walk of the disk all understand: it works in every folder. The keyword and the
//! kind are known in English and in a handful of other languages (Windows' own localized keywords among them: German `art:`,
//! Turkish `tür:`), with or without the `=` Windows writes after the colon.

use crate::icons::{Category, KNOWN_EXTS, category_for_ext};

/// "kind" in the languages we know it in (lower case, as `fold` leaves them).
const KEYS: [&str; 13] = ["kind", "type", "tür", "tur", "art", "typ", "tipo", "clase", "genre", "soort", "rodzaj", "вид", "тип"];

/// The kinds a word names. "document" is what Explorer means by it: texts, sheets and PDFs too.
fn kinds_of(word: &str) -> &'static [Category] {
    use Category::*;
    match word {
        "picture" | "pictures" | "image" | "images" | "photo" | "photos" | "pic" | "pics" | "resim" | "resimler" | "fotoğraf" | "fotograf"
        | "görüntü" | "görsel" | "bild" | "bilder" | "foto" | "fotos" | "imagen" | "imagem" | "immagine" | "afbeelding" | "obraz"
        | "изображение" | "фото" => &[Image],
        "video" | "videos" | "movie" | "movies" | "film" | "filme" | "vídeo" | "видео" | "wideo" => &[Video],
        "music" | "audio" | "song" | "songs" | "sound" | "müzik" | "muzik" | "ses" | "şarkı" | "musik" | "música" | "musica" | "musique"
        | "muziek" | "muzyka" | "музыка" => &[Audio],
        "document" | "documents" | "doc" | "docs" | "belge" | "belgeler" | "doküman" | "dokument" | "dokumente" | "documento"
        | "documentos" | "документ" => &[Document, Spreadsheet, Pdf, Text],
        "archive" | "archives" | "zip" | "compressed" | "arşiv" | "arsiv" | "archiv" | "archivo" | "arquivo" | "архив" => &[Archive],
        "program" | "programs" | "app" | "apps" | "exe" | "uygulama" | "programm" | "programme" | "programa" | "программа" => &[Executable],
        "code" | "kod" | "código" | "codigo" | "код" => &[Code],
        "text" | "txt" | "metin" | "texto" | "texte" | "текст" => &[Text],
        "spreadsheet" | "spreadsheets" | "sheet" | "table" | "tablo" | "tabelle" => &[Spreadsheet],
        "pdf" => &[Pdf],
        _ => &[],
    }
}

/// `key:value` or `key:=value` with a key of [`KEYS`] and a value that names a kind: its extensions.
fn kind_word(word: &str) -> Option<Vec<&'static str>> {
    let (key, value) = word.split_once(':')?;
    let key = key.to_lowercase();
    if !KEYS.contains(&key.as_str()) {
        return None;
    }
    let value = value.trim_start_matches('=').to_lowercase();
    let kinds = kinds_of(&value);
    if kinds.is_empty() {
        return None;
    }
    Some(KNOWN_EXTS.iter().copied().filter(|e| kinds.contains(&category_for_ext(e))).collect())
}

/// `text` with each kind word replaced by the `ext:` list of that kind. A text with none comes back as it is (no copy made of its
/// words); a kind we do not know stays a word of the name, as before.
pub fn expand(text: &str) -> String {
    if !text.contains(':') || !text.split_whitespace().any(|w| kind_word(w).is_some()) {
        return text.to_string();
    }
    text.split_whitespace().map(|w| kind_word(w).map_or_else(|| w.to_string(), |exts| format!("ext:{}", exts.join(",")))).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_becomes_its_extensions() {
        let f = rf_core::namefilter::parse(&expand("kind:video tatil"));
        assert_eq!(f.name, "tatil");
        assert!(f.ext_ok("a.MP4") && f.ext_ok("b.mkv") && !f.ext_ok("c.jpg") && !f.ext_ok("d.pdf"));
        // Windows' own spellings: with "=", in German and in Turkish
        for q in ["art:=video", "tür:video", "Kind:=Videos", "TYPE:film"] {
            let f = rf_core::namefilter::parse(&expand(q));
            assert!(f.name.is_empty() && f.ext_ok("x.mov") && !f.ext_ok("x.png"), "{q}");
        }
        let docs = rf_core::namefilter::parse(&expand("kind:document"));
        assert!(docs.ext_ok("a.docx") && docs.ext_ok("a.pdf") && docs.ext_ok("a.xlsx") && docs.ext_ok("a.txt") && !docs.ext_ok("a.mp3"));
        let pics = rf_core::namefilter::parse(&expand("tür:resim"));
        assert!(pics.ext_ok("a.jpg") && pics.ext_ok("a.heic") && !pics.ext_ok("a.mp4"));
    }

    #[test]
    fn other_texts_stay_as_they_are() {
        for q in ["", "rapor", "C:\\a b", "kind:", "kind:nothing-we-know", "note: video", "tag:video", "ext:pdf", "*.mp4"] {
            assert_eq!(expand(q), q, "{q}");
        }
    }

    #[test]
    fn every_kind_has_extensions_and_every_listed_extension_a_kind() {
        use Category::*;
        for k in [Image, Video, Audio, Archive, Code, Document, Spreadsheet, Pdf, Executable, Text] {
            assert!(KNOWN_EXTS.iter().any(|e| category_for_ext(e) == k), "{k:?}");
        }
        for e in KNOWN_EXTS {
            assert_ne!(category_for_ext(e), Other, "{e} is listed but has no kind");
        }
    }
}
