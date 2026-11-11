//! Documents read into something the previewer can draw without the program that made them:
//! Word, Excel and PowerPoint files (docx, xlsx, pptx), OpenDocument text, RTF and HTML become
//! Markdown or cleaned HTML; SQLite databases, crash dumps and prefetch files (see [`binary`])
//! become a Markdown summary. Text and structure only: no fonts, pictures or
//! page layout. Pure parsing, off the UI thread.

pub mod binary;

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// What a file was turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doc {
    Markdown(String),
    Html(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Docx,
    Xlsx,
    Pptx,
    /// OpenDocument text, spreadsheet or presentation (their text).
    Odf,
    Rtf,
    Html,
    Sqlite,
    Minidump,
    Prefetch,
}

/// Gives a label of a summary in the user's language (the labels are our Turkish texts).
pub type Tr<'a> = &'a dyn Fn(&'static str) -> String;

/// The kind of a file by its extension (lowercase, no dot) and first bytes; None for anything
/// else. Content decides where an extension says little (`.db`, `.dmp`).
pub fn kind_of(ext: &str, head: &[u8]) -> Option<Kind> {
    let zip = head.starts_with(b"PK\x03\x04");
    match ext {
        "docx" | "docm" | "dotx" if zip => Some(Kind::Docx),
        "xlsx" | "xlsm" | "xltx" if zip => Some(Kind::Xlsx),
        "pptx" | "pptm" | "ppsx" | "potx" if zip => Some(Kind::Pptx),
        "odt" | "ods" | "odp" | "ott" if zip => Some(Kind::Odf),
        "rtf" if head.starts_with(b"{\\rtf") => Some(Kind::Rtf),
        "html" | "htm" | "xhtml" | "mht" | "mhtml" => Some(Kind::Html),
        "pf" if head.starts_with(b"MAM") || head.get(4..8) == Some(b"SCCA") => Some(Kind::Prefetch),
        _ if head.starts_with(b"SQLite format 3\0") => Some(Kind::Sqlite),
        _ if head.starts_with(b"MDMP") => Some(Kind::Minidump),
        _ => None,
    }
}

/// Reads the file as its kind.
pub fn read(path: &Path, kind: Kind, tr: Tr) -> Result<Doc> {
    Ok(match kind {
        Kind::Docx => Doc::Markdown(docx(path)?),
        Kind::Xlsx => Doc::Markdown(xlsx(path)?),
        Kind::Pptx => Doc::Markdown(pptx(path)?),
        Kind::Odf => Doc::Markdown(odf(path)?),
        Kind::Rtf => Doc::Markdown(rtf(&read_limited(path, 16 << 20)?)),
        Kind::Html => Doc::Html(html(&read_limited(path, 4 << 20)?)),
        Kind::Sqlite => Doc::Markdown(binary::sqlite(path, tr)?),
        Kind::Minidump => Doc::Markdown(binary::minidump(path, tr)?),
        Kind::Prefetch => Doc::Markdown(binary::prefetch(path, tr)?),
    })
}

/// The most text a preview carries (a longer document ends with a note saying so).
const OUT_MAX: usize = 1 << 20;

fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.take(max).read_to_end(&mut buf)?;
    Ok(buf)
}

/// One member of a zip container as text (UTF-8, as OOXML and ODF parts are); None if absent.
fn zip_text(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = zip.by_name(name).ok()?;
    if f.size() > 64 << 20 {
        return None;
    }
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    zip::ZipArchive::new(std::fs::File::open(path)?).context("Belge açılamadı (zip değil)")
}

// ---------------------------------------------------------------- Markdown writing

/// Escapes what Markdown would read as markup.
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '|' | '#' | '~') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A table cell: one line, no column separators.
pub(crate) fn cell(s: &str) -> String {
    esc(s.trim()).replace(['\r', '\n'], " ")
}

/// A paragraph being built from runs of text with their own bold / italic.
#[derive(Default)]
struct Para {
    out: String,
    run: String,
    style: (bool, bool),
}

impl Para {
    fn text(&mut self, t: &str, bold: bool, italic: bool) {
        if (bold, italic) != self.style {
            self.flush();
            self.style = (bold, italic);
        }
        self.run.push_str(t);
    }

    /// Ends the run: its text goes out with the marks around the words (not around the spaces
    /// at its ends, which would stop them from being marks).
    fn flush(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let run = std::mem::take(&mut self.run);
        let core = run.trim();
        if core.is_empty() {
            self.out.push_str(&run);
            return;
        }
        let lead = &run[..run.len() - run.trim_start().len()];
        let trail = &run[run.trim_end().len()..];
        let mark = match self.style {
            (true, true) => "***",
            (true, false) => "**",
            (false, true) => "*",
            _ => "",
        };
        self.out.push_str(lead);
        self.out.push_str(mark);
        // a line break inside a paragraph is a hard break
        self.out.push_str(&esc(core).replace('\n', "  \n"));
        self.out.push_str(mark);
        self.out.push_str(trail);
    }

    fn finish(mut self) -> String {
        self.flush();
        self.out
    }
}

/// Adds a block and a blank line; false once the preview is full.
fn push_block(out: &mut String, block: &str) -> bool {
    if out.len() > OUT_MAX {
        return false;
    }
    if !block.trim().is_empty() {
        out.push_str(block.trim_end_matches('\n'));
        out.push_str("\n\n");
    }
    true
}

fn end(mut out: String) -> String {
    if out.len() > OUT_MAX {
        out.push_str("\n\n*…*\n");
    }
    out
}

/// A Markdown table (the first row is its head); empty for no rows.
pub(crate) fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (i, r) in rows.iter().enumerate() {
        out.push('|');
        for c in 0..cols {
            out.push(' ');
            out.push_str(&r.get(c).map(|s| cell(s)).unwrap_or_default());
            out.push_str(" |");
        }
        out.push('\n');
        if i == 0 {
            out.push('|');
            out.push_str(&" --- |".repeat(cols));
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------- docx

fn attr<'a>(n: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.tag_name().name() == name)
}

/// A run property that is on: present, and not switched off with val="0" / "false".
fn on(props: Option<roxmltree::Node>, name: &str) -> bool {
    props.and_then(|p| child(p, name)).is_some_and(|b| !matches!(attr(b, "val"), Some("0" | "false" | "none")))
}

/// Heading level of a paragraph style ("Heading2", Word's Turkish "Balk2", "Title").
fn heading_level(style: &str) -> Option<usize> {
    let s = style.to_ascii_lowercase();
    if s == "title" || s == "konubal" {
        return Some(1);
    }
    if s == "subtitle" || s == "altyaz" {
        return Some(2);
    }
    let digits = s.strip_prefix("heading").or_else(|| s.strip_prefix("balk"))?;
    digits.parse::<usize>().ok().filter(|n| (1..=6).contains(n))
}

fn docx_para(p: roxmltree::Node) -> (String, Option<usize>, bool) {
    let props = child(p, "pPr");
    let level = props.and_then(|pp| child(pp, "pStyle")).and_then(|s| attr(s, "val")).and_then(heading_level);
    let listed = props.is_some_and(|pp| child(pp, "numPr").is_some());
    let mut para = Para::default();
    for r in p.descendants().filter(|n| n.tag_name().name() == "r") {
        let rp = child(r, "rPr");
        let (bold, italic) = (on(rp, "b"), on(rp, "i"));
        for c in r.children() {
            match c.tag_name().name() {
                "t" => para.text(c.text().unwrap_or(""), bold, italic),
                "tab" => para.text("\t", bold, italic),
                "br" | "cr" => para.text("\n", bold, italic),
                _ => {}
            }
        }
    }
    (para.finish(), level, listed)
}

fn docx_blocks(parent: roxmltree::Node, out: &mut String) -> bool {
    for n in parent.children() {
        let ok = match n.tag_name().name() {
            "p" => {
                let (text, level, listed) = docx_para(n);
                match (level, listed) {
                    // (a heading carries no marks of its own)
                    (Some(l), _) => push_block(out, &format!("{} {}", "#".repeat(l), text.replace("**", "").trim())),
                    (None, true) => push_block(out, &format!("- {}", text.trim())),
                    _ => push_block(out, &text),
                }
            }
            "tbl" => {
                let rows: Vec<Vec<String>> = n
                    .children()
                    .filter(|r| r.tag_name().name() == "tr")
                    .map(|r| {
                        r.children()
                            .filter(|c| c.tag_name().name() == "tc")
                            .map(|c| c.descendants().filter(|t| t.tag_name().name() == "t").filter_map(|t| t.text()).collect::<Vec<_>>().join(" "))
                            .collect()
                    })
                    .collect();
                push_block(out, &table(&rows))
            }
            // content controls and the like wrap ordinary blocks
            "sdt" | "sdtContent" | "customXml" | "smartTag" => docx_blocks(n, out),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn docx(path: &Path) -> Result<String> {
    let mut zip = open_zip(path)?;
    let xml = zip_text(&mut zip, "word/document.xml").context("Belgede metin bulunamadı")?;
    let doc = roxmltree::Document::parse(&xml).context("Belge okunamadı")?;
    let Some(body) = doc.descendants().find(|n| n.tag_name().name() == "body") else { bail!("Belgede metin bulunamadı") };
    let mut out = String::new();
    docx_blocks(body, &mut out);
    Ok(end(out))
}

// ---------------------------------------------------------------- xlsx

/// "BC12" -> column 54 (0-based).
fn column_of(cell_ref: &str) -> usize {
    cell_ref.bytes().take_while(u8::is_ascii_alphabetic).fold(0usize, |n, b| n * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1).saturating_sub(1)
}

const SHEET_ROWS: usize = 500;
const SHEET_COLS: usize = 40;

fn xlsx_sheet(xml: &str, shared: &[String]) -> Result<(Vec<Vec<String>>, bool)> {
    let doc = roxmltree::Document::parse(xml).context("Sayfa okunamadı")?;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut cut = false;
    for row in doc.descendants().filter(|n| n.tag_name().name() == "row") {
        if rows.len() >= SHEET_ROWS {
            cut = true;
            break;
        }
        let mut cells: Vec<String> = Vec::new();
        for c in row.children().filter(|n| n.tag_name().name() == "c") {
            let col = attr(c, "r").map_or(cells.len(), column_of);
            if col >= SHEET_COLS {
                cut = true;
                continue;
            }
            let raw = child(c, "v").and_then(|v| v.text()).unwrap_or("");
            let value = match attr(c, "t") {
                Some("s") => raw.parse::<usize>().ok().and_then(|i| shared.get(i)).cloned().unwrap_or_default(),
                Some("inlineStr") => c.descendants().filter(|t| t.tag_name().name() == "t").filter_map(|t| t.text()).collect(),
                Some("b") => if raw == "1" { "TRUE" } else { "FALSE" }.to_string(),
                _ => raw.to_string(),
            };
            if cells.len() <= col {
                cells.resize(col + 1, String::new());
            }
            cells[col] = value;
        }
        if cells.iter().any(|c| !c.is_empty()) {
            rows.push(cells);
        }
    }
    Ok((rows, cut))
}

fn xlsx(path: &Path) -> Result<String> {
    let mut zip = open_zip(path)?;
    // the texts of the cells are kept once, apart from the sheets
    let shared: Vec<String> = match zip_text(&mut zip, "xl/sharedStrings.xml") {
        Some(xml) => {
            let doc = roxmltree::Document::parse(&xml).context("Çalışma kitabı okunamadı")?;
            doc.descendants()
                .filter(|n| n.tag_name().name() == "si")
                .map(|si| {
                    // (phonetic guides of East Asian text are not part of the cell's text)
                    si.descendants().filter(|t| t.tag_name().name() == "t" && !t.ancestors().any(|a| a.tag_name().name() == "rPh")).filter_map(|t| t.text()).collect()
                })
                .collect()
        }
        None => Vec::new(),
    };
    let names: Vec<String> = zip_text(&mut zip, "xl/workbook.xml")
        .and_then(|xml| {
            let doc = roxmltree::Document::parse(&xml).ok()?;
            Some(doc.descendants().filter(|n| n.tag_name().name() == "sheet").filter_map(|s| attr(s, "name").map(str::to_string)).collect())
        })
        .unwrap_or_default();
    let mut out = String::new();
    for i in 1..=16usize {
        let Some(xml) = zip_text(&mut zip, &format!("xl/worksheets/sheet{i}.xml")) else { break };
        let (rows, cut) = xlsx_sheet(&xml, &shared)?;
        let name = names.get(i - 1).cloned().unwrap_or_else(|| format!("{i}"));
        if !push_block(&mut out, &format!("## {}", esc(&name))) {
            break;
        }
        push_block(&mut out, &table(&rows));
        if cut {
            push_block(&mut out, "*…*");
        }
    }
    if out.is_empty() {
        bail!("Çalışma kitabında sayfa bulunamadı");
    }
    Ok(end(out))
}

// ---------------------------------------------------------------- pptx

fn pptx(path: &Path) -> Result<String> {
    let mut zip = open_zip(path)?;
    let mut out = String::new();
    for i in 1..=2000usize {
        let Some(xml) = zip_text(&mut zip, &format!("ppt/slides/slide{i}.xml")) else { break };
        let doc = roxmltree::Document::parse(&xml).context("Slayt okunamadı")?;
        let mut lines: Vec<String> = Vec::new();
        for p in doc.descendants().filter(|n| n.tag_name().name() == "p") {
            let text: String = p.descendants().filter(|t| t.tag_name().name() == "t").filter_map(|t| t.text()).collect();
            if !text.trim().is_empty() {
                lines.push(text.trim().to_string());
            }
        }
        // the first text of a slide is, nearly always, its title
        let title = lines.first().cloned().unwrap_or_default();
        if !push_block(&mut out, &format!("## {i}. {}", esc(&title))) {
            break;
        }
        let rest: Vec<String> = lines.iter().skip(1).map(|l| format!("- {}", esc(l))).collect();
        push_block(&mut out, &rest.join("\n"));
    }
    if out.is_empty() {
        bail!("Sunuda slayt bulunamadı");
    }
    Ok(end(out))
}

// ---------------------------------------------------------------- OpenDocument

fn odf(path: &Path) -> Result<String> {
    let mut zip = open_zip(path)?;
    let xml = zip_text(&mut zip, "content.xml").context("Belgede metin bulunamadı")?;
    let doc = roxmltree::Document::parse(&xml).context("Belge okunamadı")?;
    let mut out = String::new();
    for n in doc.descendants() {
        let name = n.tag_name().name();
        if name != "h" && name != "p" {
            continue;
        }
        // (a paragraph inside a paragraph-like parent is that parent's text already)
        let text: String = n.descendants().filter(|t| t.is_text()).filter_map(|t| t.text()).collect();
        let block = if name == "h" {
            let level = attr(n, "outline-level").and_then(|l| l.parse::<usize>().ok()).unwrap_or(1).clamp(1, 6);
            format!("{} {}", "#".repeat(level), esc(text.trim()))
        } else {
            esc(text.trim())
        };
        if !push_block(&mut out, &block) {
            break;
        }
    }
    Ok(end(out))
}

// ---------------------------------------------------------------- RTF

/// Decodes one byte of an RTF `\'hh` escape with the document's code page.
fn ansi_char(byte: u8, codepage: u32) -> char {
    let label = format!("windows-{codepage}");
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252);
    enc.decode_without_bom_handling(&[byte]).0.chars().next().unwrap_or('?')
}

/// Groups whose content is not the document's text.
const RTF_SKIP: &[&str] = &[
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "object", "header", "footer", "headerl", "headerr", "footerl", "footerr", "footnote", "themedata", "colorschememapping",
    "latentstyles", "datastore", "listtable", "listoverridetable", "rsidtbl", "generator", "xmlnstbl", "mmathPr", "fldinst", "revtbl", "bkmkstart", "bkmkend",
];

/// RTF as Markdown: the text with its paragraphs, bold and italic. Tables come out as their
/// cells' text, one row a line.
fn rtf(bytes: &[u8]) -> String {
    #[derive(Clone, Copy)]
    struct State {
        bold: bool,
        italic: bool,
        skip: bool,
        /// How many characters follow a `\uN` for readers that don't know Unicode.
        uc: usize,
    }
    let mut stack: Vec<State> = Vec::new();
    let mut st = State { bold: false, italic: false, skip: false, uc: 1 };
    let mut codepage = 1252u32;
    let mut out = String::new();
    let mut para = Para::default();
    let mut pending_skip = 0usize;
    let mut i = 0;
    let n = bytes.len();
    macro_rules! put {
        ($s:expr) => {
            if !st.skip {
                if pending_skip > 0 {
                    pending_skip -= 1;
                } else {
                    para.text($s, st.bold, st.italic);
                }
            }
        };
    }
    while i < n && out.len() <= OUT_MAX {
        match bytes[i] {
            b'{' => {
                stack.push(st);
                i += 1;
            }
            b'}' => {
                st = stack.pop().unwrap_or(st);
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= n {
                    break;
                }
                let c = bytes[i];
                if c.is_ascii_alphabetic() {
                    let start = i;
                    while i < n && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                    let num_start = i;
                    if i < n && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
                        i += 1;
                        while i < n && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let num: Option<i32> = std::str::from_utf8(&bytes[num_start..i]).ok().and_then(|s| s.parse().ok());
                    // one space after a control word belongs to it
                    if i < n && bytes[i] == b' ' {
                        i += 1;
                    }
                    match word {
                        "par" | "sect" | "page" | "row" => {
                            if !st.skip {
                                let done = std::mem::take(&mut para).finish();
                                push_block(&mut out, &done);
                            }
                        }
                        "line" => put!("\n"),
                        "tab" | "cell" => put!("\t"),
                        "b" => st.bold = num != Some(0),
                        "i" => st.italic = num != Some(0),
                        "plain" => (st.bold, st.italic) = (false, false),
                        "ansicpg" => codepage = num.unwrap_or(1252).max(0) as u32,
                        "uc" => st.uc = num.unwrap_or(1).max(0) as usize,
                        "u" => {
                            if let Some(v) = num {
                                // (negative: 16-bit values written signed)
                                let code = if v < 0 { (v + 65536) as u32 } else { v as u32 };
                                if let Some(ch) = char::from_u32(code) {
                                    let mut b = [0u8; 4];
                                    put!(ch.encode_utf8(&mut b));
                                }
                                if !st.skip {
                                    pending_skip = st.uc;
                                }
                            }
                        }
                        "emdash" => put!("—"),
                        "endash" => put!("–"),
                        "bullet" => put!("•"),
                        "lquote" => put!("‘"),
                        "rquote" => put!("’"),
                        "ldblquote" => put!("“"),
                        "rdblquote" => put!("”"),
                        w if RTF_SKIP.contains(&w) => st.skip = true,
                        _ => {}
                    }
                } else {
                    i += 1;
                    match c {
                        b'\'' => {
                            let hex = bytes.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                            i = (i + 2).min(n);
                            if let Some(b) = hex {
                                let mut buf = [0u8; 4];
                                put!(ansi_char(b, codepage).encode_utf8(&mut buf));
                            }
                        }
                        // "\*": a group newer readers know and older ones are told to ignore
                        b'*' => st.skip = true,
                        b'~' => put!("\u{a0}"),
                        b'\\' | b'{' | b'}' => {
                            let s = [c];
                            put!(std::str::from_utf8(&s).unwrap_or(""));
                        }
                        b'\r' | b'\n' => {
                            if !st.skip {
                                let done = std::mem::take(&mut para).finish();
                                push_block(&mut out, &done);
                            }
                        }
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                // plain text up to the next control character (ASCII in RTF; anything else as
                // the code page has it)
                let start = i;
                while i < n && !matches!(bytes[i], b'{' | b'}' | b'\\' | b'\r' | b'\n') {
                    i += 1;
                }
                if !st.skip {
                    for &b in &bytes[start..i] {
                        let mut buf = [0u8; 4];
                        let ch = if b < 0x80 { b as char } else { ansi_char(b, codepage) };
                        put!(ch.encode_utf8(&mut buf));
                    }
                }
            }
        }
    }
    push_block(&mut out, &para.finish());
    end(out)
}

// ---------------------------------------------------------------- HTML

/// Cuts every `<tag ...>...</tag>` block out of `html` (case-insensitive).
fn strip_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}"));
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(i) = lower[at..].find(&open) {
        let start = at + i;
        // "<style" but not "<styles"
        let after = lower.as_bytes().get(start + open.len()).copied().unwrap_or(b'>');
        if after.is_ascii_alphanumeric() {
            out.push_str(&html[at..start + open.len()]);
            at = start + open.len();
            continue;
        }
        out.push_str(&html[at..start]);
        match lower[start..].find(&close) {
            Some(j) => {
                let end = start + j;
                at = lower[end..].find('>').map_or(html.len(), |k| end + k + 1);
            }
            None => {
                at = html.len();
            }
        }
    }
    out.push_str(&html[at..]);
    out
}

/// The page as text in its own encoding (a byte order mark, else the `charset` it names, else
/// UTF-8), without scripts and style sheets: what is left is drawn as a document.
fn html(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let named = head.find("charset=").map(|i| {
        let rest = head[i + 8..].trim_start_matches(['"', '\'', ' ']);
        rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).next().unwrap_or("").to_string()
    });
    let enc = encoding_rs::Encoding::for_bom(bytes)
        .map(|(e, _)| e)
        .or_else(|| named.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())))
        .unwrap_or(encoding_rs::UTF_8);
    let text = enc.decode(bytes).0.into_owned();
    let mut out = text;
    for tag in ["script", "style", "noscript", "template", "svg", "head"] {
        out = strip_blocks(&out, tag);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rf-docs-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn word_document_becomes_markdown() {
        let p = tmp("a.docx");
        let xml = r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Rapor</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Bu </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>önemli</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve"> bir </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>not</w:t></w:r><w:r><w:t>: 2*3</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>madde</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Ad</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Tutar</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>a|b</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>5</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#;
        zip_of(&p, &[("word/document.xml", xml)]);
        assert_eq!(kind_of("docx", b"PK\x03\x04"), Some(Kind::Docx));
        let Doc::Markdown(md) = read(&p, Kind::Docx, &|s| s.to_string()).unwrap() else { panic!() };
        assert!(md.starts_with("# Rapor\n\n"), "{md}");
        // adjacent bold runs are one; the spaces stay outside the marks; "*" is escaped
        assert!(md.contains("Bu **önemli bir** *not*: 2\\*3\n\n"), "{md}");
        assert!(md.contains("- madde\n\n"), "{md}");
        assert!(md.contains("| Ad | Tutar |\n| --- | --- |\n| a\\|b | 5 |\n"), "{md}");
    }

    #[test]
    fn workbook_and_slides_become_markdown() {
        let p = tmp("a.xlsx");
        zip_of(
            &p,
            &[
                ("xl/sharedStrings.xml", r#"<sst xmlns="x"><si><t>Ürün</t></si><si><r><t>Fi</t></r><r><t>yat</t></r></si><si><t>Elma</t></si></sst>"#),
                ("xl/workbook.xml", r#"<workbook xmlns="x"><sheets><sheet name="Satış" sheetId="1"/></sheets></workbook>"#),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet xmlns="x"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>2</v></c><c r="C2"><v>12.5</v></c></row></sheetData></worksheet>"#,
                ),
            ],
        );
        let Doc::Markdown(md) = read(&p, Kind::Xlsx, &|s| s.to_string()).unwrap() else { panic!() };
        // (C2 with B2 empty: the cell stays in its column)
        assert_eq!(md, "## Satış\n\n| Ürün | Fiyat |  |\n| --- | --- | --- |\n| Elma |  | 12.5 |\n\n");
        assert_eq!(column_of("A1"), 0);
        assert_eq!(column_of("AB7"), 27);

        let p = tmp("a.pptx");
        let slide = |t: &str, b: &str| format!(r#"<p:sld xmlns:p="p" xmlns:a="a"><p:sp><p:txBody><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:txBody><a:p><a:r><a:t>{b}</a:t></a:r></a:p><a:p/></p:txBody></p:sp></p:sld>"#);
        zip_of(&p, &[("ppt/slides/slide1.xml", &slide("Giriş", "ilk madde")), ("ppt/slides/slide2.xml", &slide("Sonuç", "son"))]);
        let Doc::Markdown(md) = read(&p, Kind::Pptx, &|s| s.to_string()).unwrap() else { panic!() };
        assert_eq!(md, "## 1. Giriş\n\n- ilk madde\n\n## 2. Sonuç\n\n- son\n\n");
    }

    #[test]
    fn rtf_becomes_markdown() {
        let src = br"{\rtf1\ansi\ansicpg1254\deff0{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\*\generator Riched20;}\pard Merhaba {\b kal\'fdn} ve {\i e\u287?ik}\par \'dd\'fe \u8364?5 \{tamam\}\line ikinci sat\u305?r\par}";
        let md = rtf(src);
        assert_eq!(md, "Merhaba **kalın** ve *eğik*\n\nİş €5 {tamam}  \nikinci satır\n\n");
        assert_eq!(kind_of("rtf", br"{\rtf1\ansi"), Some(Kind::Rtf));
    }

    #[test]
    fn html_loses_scripts_and_keeps_its_encoding() {
        let page = b"<html><head><meta charset=\"windows-1254\"><style>p{color:red}</style></head><body><SCRIPT>alert(1)</SCRIPT><p>\xdd\xfe <styles>x</styles></p></body></html>";
        let out = html(page);
        assert!(!out.to_lowercase().contains("alert") && !out.contains("color:red"), "{out}");
        assert!(out.contains("<p>İş <styles>x</styles></p>"), "{out}");
        // UTF-8 by default
        assert!(html("<p>çğ</p>".as_bytes()).contains("çğ"));
    }
}
