//! Summaries of binary files as Markdown: SQLite databases (tables, their first rows), Windows
//! crash dumps (what crashed and where), prefetch files (how often a program ran and what it
//! loaded). Own readers of the documented file formats; nothing is executed or written.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{Tr, cell, esc, table};

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A Windows FILETIME (100 ns since 1601) as Unix milliseconds.
fn filetime_ms(ft: u64) -> i64 {
    (ft / 10_000) as i64 - 11_644_473_600_000
}

fn utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

// ---------------------------------------------------------------- SQLite

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(usize),
}

impl Value {
    fn show(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Int(i) => i.to_string(),
            Value::Real(f) => f.to_string(),
            Value::Text(s) => {
                let short: String = s.chars().take(80).collect();
                if short.len() < s.len() { format!("{short}…") } else { short }
            }
            Value::Blob(n) => format!("[{n} B]"),
        }
    }
}

struct Db {
    file: File,
    page_size: usize,
    /// Bytes of a page that hold data (the rest is reserved for extensions).
    usable: usize,
    /// 1 = UTF-8, 2 = UTF-16 LE, 3 = UTF-16 BE.
    encoding: u32,
    pages: u32,
    /// Pages read so far (a damaged file with a loop in it stops here).
    budget: u32,
}

/// A big-endian variable-length integer: (value, bytes used).
fn varint(b: &[u8]) -> (u64, usize) {
    let mut v = 0u64;
    for (i, &byte) in b.iter().take(9).enumerate() {
        if i == 8 {
            return ((v << 8) | byte as u64, 9);
        }
        v = (v << 7) | (byte & 0x7f) as u64;
        if byte & 0x80 == 0 {
            return (v, i + 1);
        }
    }
    (v, b.len().min(9))
}

impl Db {
    fn open(path: &Path) -> Result<Db> {
        let mut file = File::open(path)?;
        let mut h = [0u8; 100];
        file.read_exact(&mut h).context("SQLite veritabanı okunamadı")?;
        if &h[..16] != b"SQLite format 3\0" {
            bail!("SQLite veritabanı değil");
        }
        let page_size = match u16::from_be_bytes([h[16], h[17]]) {
            1 => 65536,
            n => n as usize,
        };
        if page_size < 512 || !page_size.is_power_of_two() {
            bail!("SQLite veritabanı okunamadı");
        }
        let len = file.metadata()?.len();
        Ok(Db { file, page_size, usable: page_size - h[20] as usize, encoding: u32::from_be_bytes([h[56], h[57], h[58], h[59]]), pages: (len / page_size as u64) as u32, budget: 20_000 })
    }

    /// Page `n` (the first is 1).
    fn page(&mut self, n: u32) -> Result<Vec<u8>> {
        if n == 0 || n > self.pages || self.budget == 0 {
            bail!("SQLite veritabanı okunamadı");
        }
        self.budget -= 1;
        let mut buf = vec![0u8; self.page_size];
        self.file.seek(SeekFrom::Start((n as u64 - 1) * self.page_size as u64))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn text(&self, b: &[u8]) -> String {
        match self.encoding {
            2 => utf16le(b),
            3 => String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>()),
            _ => String::from_utf8_lossy(b).into_owned(),
        }
    }

    /// The whole payload of a table leaf cell: what is on the page plus its overflow pages.
    fn payload(&mut self, page: &[u8], at: usize, len: usize) -> Result<Vec<u8>> {
        let u = self.usable;
        let x = u - 35;
        let local = if len <= x {
            len
        } else {
            let m = ((u - 12) * 32 / 255) - 23;
            let k = m + (len - m) % (u - 4);
            if k <= x { k } else { m }
        };
        let mut out = page.get(at..at + local).context("SQLite veritabanı okunamadı")?.to_vec();
        if local < len {
            let mut next = u32::from_be_bytes(page.get(at + local..at + local + 4).context("SQLite veritabanı okunamadı")?.try_into()?);
            while next != 0 && out.len() < len {
                let p = self.page(next)?;
                next = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
                let take = (len - out.len()).min(u - 4);
                out.extend_from_slice(&p[4..4 + take]);
            }
        }
        Ok(out)
    }

    fn record(&self, payload: &[u8]) -> Vec<Value> {
        let (hdr, n) = varint(payload);
        let hdr = (hdr as usize).min(payload.len());
        let mut types = Vec::new();
        let mut at = n;
        while at < hdr {
            let (t, used) = varint(&payload[at..]);
            types.push(t);
            at += used;
        }
        let mut body = hdr;
        let mut out = Vec::with_capacity(types.len());
        for t in types {
            let int = |len: usize, body: usize| -> Value {
                let Some(b) = payload.get(body..body + len) else { return Value::Null };
                // sign-extended big-endian
                let mut v: i64 = if b.first().is_some_and(|x| x & 0x80 != 0) { -1 } else { 0 };
                for &x in b {
                    v = (v << 8) | x as i64;
                }
                Value::Int(v)
            };
            let (value, len) = match t {
                0 => (Value::Null, 0),
                1 => (int(1, body), 1),
                2 => (int(2, body), 2),
                3 => (int(3, body), 3),
                4 => (int(4, body), 4),
                5 => (int(6, body), 6),
                6 => (int(8, body), 8),
                7 => (payload.get(body..body + 8).map_or(Value::Null, |b| Value::Real(f64::from_be_bytes(b.try_into().unwrap_or([0; 8])))), 8),
                8 => (Value::Int(0), 0),
                9 => (Value::Int(1), 0),
                t if t >= 12 && t % 2 == 0 => {
                    let len = ((t - 12) / 2) as usize;
                    (Value::Blob(len), len)
                }
                t if t >= 13 => {
                    let len = ((t - 13) / 2) as usize;
                    (payload.get(body..body + len).map_or(Value::Null, |b| Value::Text(self.text(b))), len)
                }
                _ => (Value::Null, 0),
            };
            out.push(value);
            body += len;
        }
        out
    }

    /// Walks the table whose root page is `root`: returns its number of rows, and gives the
    /// first `first` of them (row id, values) to `row`.
    fn table(&mut self, root: u32, first: usize, row: &mut dyn FnMut(i64, Vec<Value>)) -> Result<u64> {
        let mut count = 0u64;
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            let page = self.page(n)?;
            // (page 1 starts with the file header)
            let h = if n == 1 { 100 } else { 0 };
            let kind = page[h];
            let cells = u16::from_be_bytes([page[h + 3], page[h + 4]]) as usize;
            let ptrs = h + if kind == 5 { 12 } else { 8 };
            let cell_at = |i: usize| page.get(ptrs + i * 2..ptrs + i * 2 + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
            match kind {
                // interior: children left to right (pushed in reverse)
                5 => {
                    stack.push(u32::from_be_bytes([page[h + 8], page[h + 9], page[h + 10], page[h + 11]]));
                    for i in (0..cells).rev() {
                        if let Some(b) = cell_at(i).and_then(|at| page.get(at..at + 4)) {
                            stack.push(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                        }
                    }
                }
                13 => {
                    for i in 0..cells {
                        if (count as usize) < first {
                            if let Some(at) = cell_at(i).filter(|at| *at < page.len()) {
                                let (len, a) = varint(&page[at..]);
                                let (rowid, b) = varint(&page[(at + a).min(page.len())..]);
                                if let Ok(payload) = self.payload(&page, at + a + b, len as usize) {
                                    let values = self.record(&payload);
                                    row(rowid as i64, values);
                                }
                            }
                        }
                        count += 1;
                    }
                }
                _ => {}
            }
        }
        Ok(count)
    }
}

/// Column names of a `CREATE TABLE` statement, and which one is the row id (an INTEGER PRIMARY
/// KEY column is stored as the row's id, not in the row).
fn columns(sql: &str) -> (Vec<String>, Option<usize>) {
    let Some(open) = sql.find('(') else { return (Vec::new(), None) };
    let body = &sql[open + 1..sql.rfind(')').unwrap_or(sql.len())];
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    let mut names = Vec::new();
    let mut rowid = None;
    for p in parts {
        let p = p.trim();
        let first = p.split_whitespace().next().unwrap_or("");
        if matches!(first.to_ascii_uppercase().as_str(), "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN" | "CONSTRAINT" | "") {
            continue;
        }
        let upper = p.to_ascii_uppercase();
        if upper.contains("INTEGER PRIMARY KEY") {
            rowid = Some(names.len());
        }
        names.push(first.trim_matches(['"', '`', '[', ']', '\'']).to_string());
    }
    (names, rowid)
}

const SQLITE_TABLES: usize = 40;
const SQLITE_ROWS: usize = 8;

pub(super) fn sqlite(path: &Path, tr: Tr) -> Result<String> {
    let mut db = Db::open(path)?;
    // the schema is itself a table, at page 1: (type, name, table, root page, SQL)
    let mut schema: Vec<Vec<Value>> = Vec::new();
    db.table(1, 2000, &mut |_, v| schema.push(v))?;
    let text = |v: Option<&Value>| match v {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    };
    let tables: Vec<(String, u32, String)> = schema
        .iter()
        .filter(|r| text(r.first()) == "table")
        .map(|r| (text(r.get(1)), match r.get(3) { Some(Value::Int(n)) => *n as u32, _ => 0 }, text(r.get(4))))
        .collect();
    let others = schema.len() - tables.len();
    let mut out = format!("**SQLite** · {} {} · {} {} · {} {}\n\n", tables.len(), tr("tablo"), db.pages, tr("sayfa"), db.page_size, tr("bayt/sayfa"));
    if others > 0 {
        out.push_str(&format!("{others} {}\n\n", tr("dizin, görünüm ve tetikleyici")));
    }
    for (name, root, sql) in tables.iter().take(SQLITE_TABLES) {
        let (names, rowid_col) = columns(sql);
        let mut rows: Vec<Vec<String>> = vec![names.clone()];
        // a virtual table has no pages of its own
        let count = if *root == 0 {
            None
        } else {
            db.table(*root, SQLITE_ROWS, &mut |rowid, values| {
                let mut r: Vec<String> = values.iter().map(Value::show).collect();
                if let Some(c) = rowid_col.filter(|c| *c < r.len()) {
                    r[c] = rowid.to_string();
                }
                rows.push(r);
            })
            .ok()
        };
        out.push_str(&match count {
            Some(n) => format!("## {} · {n} {}\n\n", esc(name), tr("satır")),
            None => format!("## {}\n\n", esc(name)),
        });
        out.push_str(&table(&rows));
        if count.is_some_and(|n| n as usize > SQLITE_ROWS) {
            out.push_str("\n*…*\n");
        }
        out.push('\n');
    }
    if tables.len() > SQLITE_TABLES {
        out.push_str("*…*\n");
    }
    Ok(out)
}

// ---------------------------------------------------------------- minidump

/// What Windows calls the common exception codes.
fn exception_name(code: u32) -> &'static str {
    match code {
        0xC000_0005 => "Access violation",
        0x8000_0003 => "Breakpoint",
        0xC000_00FD => "Stack overflow",
        0xC000_0409 => "Stack buffer overrun / fail fast",
        0xC000_0374 => "Heap corruption",
        0xC000_0094 => "Integer division by zero",
        0xC000_001D => "Illegal instruction",
        0xC000_0096 => "Privileged instruction",
        0xE06D_7363 => "C++ exception",
        0xE043_4352 => ".NET exception",
        0xC000_0420 => "Assertion failure",
        0xC000_0135 => "DLL not found",
        0x4000_0015 => "Fatal app exit (abort)",
        _ => "",
    }
}

pub(super) fn minidump(path: &Path, tr: Tr) -> Result<String> {
    // the directory and the small streams are near the start; names can be anywhere
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = vec![0u8; len.min(8 << 20) as usize];
    f.read_exact(&mut head)?;
    if !head.starts_with(b"MDMP") {
        bail!("Bellek dökümü okunamadı");
    }
    let read_at = |f: &mut File, at: u64, n: usize| -> Option<Vec<u8>> {
        if at.checked_add(n as u64)? > len {
            return None;
        }
        let mut b = vec![0u8; n];
        f.seek(SeekFrom::Start(at)).ok()?;
        f.read_exact(&mut b).ok()?;
        Some(b)
    };
    // a MINIDUMP_STRING: its length in bytes, then UTF-16
    let string_at = |f: &mut File, rva: u32| -> String {
        let n = read_at(f, rva as u64, 4).and_then(|b| u32le(&b, 0)).unwrap_or(0).min(2048) as usize;
        read_at(f, rva as u64 + 4, n).map(|b| utf16le(&b)).unwrap_or_default()
    };
    let streams = u32le(&head, 8).unwrap_or(0).min(256) as usize;
    let dir = u32le(&head, 12).unwrap_or(0) as usize;
    let time = u32le(&head, 20).unwrap_or(0);
    let stream = |kind: u32| -> Option<(u32, u32)> {
        (0..streams).find_map(|i| {
            let at = dir + i * 12;
            (u32le(&head, at)? == kind).then_some((u32le(&head, at + 4)?, u32le(&head, at + 8)?))
        })
    };
    let mut rows: Vec<Vec<String>> = vec![vec![tr("Özellik"), tr("Değer")]];
    let mut add = |k: String, v: String| {
        if !v.is_empty() {
            rows.push(vec![k, v]);
        }
    };
    // modules: (base, size, name)
    let mut modules: Vec<(u64, u32, String)> = Vec::new();
    if let Some((size, rva)) = stream(4) {
        if let Some(list) = read_at(&mut f, rva as u64, size as usize) {
            let n = u32le(&list, 0).unwrap_or(0).min(4096) as usize;
            for i in 0..n {
                let at = 4 + i * 108;
                let (Some(base), Some(sz), Some(name_rva)) = (u64le(&list, at), u32le(&list, at + 8), u32le(&list, at + 20)) else { break };
                modules.push((base, sz, string_at(&mut f, name_rva)));
            }
        }
    }
    let short = |p: &str| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string();
    if let Some((_, _, name)) = modules.first() {
        add(tr("İşlem"), short(name));
    }
    if time != 0 {
        add(tr("Zaman"), rf_core::format::datetime_full(time as i64 * 1000));
    }
    if let Some((_, rva)) = stream(7) {
        let at = rva as usize;
        let arch = match u16le(&head, at) {
            Some(9) => "x64",
            Some(0) => "x86",
            Some(12) => "ARM64",
            Some(5) => "ARM",
            _ => "?",
        };
        let cpus = head.get(at + 6).copied().unwrap_or(0);
        if let (Some(major), Some(minor), Some(build)) = (u32le(&head, at + 8), u32le(&head, at + 12), u32le(&head, at + 16)) {
            add(tr("Sistem"), format!("Windows {major}.{minor}.{build} · {arch} · {cpus} {}", tr("işlemci")));
        }
    }
    if let Some((_, rva)) = stream(6) {
        let at = rva as usize;
        // thread id, alignment, then the exception record: code, flags, record, address
        if let (Some(code), Some(addr)) = (u32le(&head, at + 8), u64le(&head, at + 24)) {
            let name = exception_name(code);
            add(tr("Hata"), if name.is_empty() { format!("0x{code:08X}") } else { format!("0x{code:08X} ({name})") });
            let place = modules
                .iter()
                .find(|(base, size, _)| addr >= *base && addr < *base + *size as u64)
                .map_or_else(|| format!("0x{addr:X}"), |(base, _, name)| format!("{} + 0x{:X}", short(name), addr - base));
            add(tr("Hatanın yeri"), place);
        }
    }
    if let Some((_, rva)) = stream(3) {
        if let Some(n) = u32le(&head, rva as usize) {
            add(tr("İş parçacıkları"), n.to_string());
        }
    }
    add(tr("Boyut"), rf_core::format::size(len));
    let mut out = table(&rows);
    if !modules.is_empty() {
        out.push_str(&format!("\n## {} · {}\n\n", tr("Modüller"), modules.len()));
        for (_, _, name) in modules.iter().take(200) {
            out.push_str(&format!("- {}\n", cell(&short(name))));
        }
        if modules.len() > 200 {
            out.push_str("- …\n");
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- prefetch

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetCompressionWorkSpaceSize(format: u16, buffer_size: *mut u32, fragment_size: *mut u32) -> i32;
    fn RtlDecompressBufferEx(format: u16, out: *mut u8, out_size: u32, input: *const u8, in_size: u32, final_size: *mut u32, workspace: *mut core::ffi::c_void) -> i32;
}

/// Windows 8 and later store prefetch files compressed ("MAM", Xpress Huffman): Windows' own
/// decompressor opens them.
fn prefetch_bytes(raw: Vec<u8>) -> Result<Vec<u8>> {
    if !raw.starts_with(b"MAM") {
        return Ok(raw);
    }
    const XPRESS_HUFF: u16 = 4;
    let size = u32le(&raw, 4).context("Prefetch dosyası okunamadı")?;
    if size > 64 << 20 {
        bail!("Prefetch dosyası okunamadı");
    }
    let mut out = vec![0u8; size as usize];
    let (mut ws, mut frag, mut done) = (0u32, 0u32, 0u32);
    unsafe {
        if RtlGetCompressionWorkSpaceSize(XPRESS_HUFF, &mut ws, &mut frag) != 0 {
            bail!("Prefetch dosyası açılamadı");
        }
        let mut workspace = vec![0u8; ws as usize];
        let st = RtlDecompressBufferEx(XPRESS_HUFF, out.as_mut_ptr(), size, raw[8..].as_ptr(), (raw.len() - 8) as u32, &mut done, workspace.as_mut_ptr().cast());
        if st != 0 {
            bail!("Prefetch dosyası açılamadı");
        }
    }
    out.truncate(done as usize);
    Ok(out)
}

/// (program, run count, last runs as Unix ms, files it loaded) of a decompressed prefetch file.
fn prefetch_parse(b: &[u8]) -> Result<(String, u32, Vec<i64>, Vec<String>)> {
    if b.get(4..8) != Some(b"SCCA") {
        bail!("Prefetch dosyası okunamadı");
    }
    let version = u32le(b, 0).unwrap_or(0);
    let name = utf16le(b.get(16..76).unwrap_or(&[]));
    let name = name.split('\0').next().unwrap_or("").to_string();
    // where the versions keep the last run times and the run count
    let (times_at, times_n, count_at) = match version {
        17 => (120, 1, 144),
        23 => (128, 1, 152),
        26 => (128, 8, 208),
        // Windows 10 / 11: two layouts, told apart by where the first table starts
        _ => (128, 8, if u32le(b, 84) == Some(296) { 200 } else { 208 }),
    };
    let times: Vec<i64> = (0..times_n).filter_map(|i| u64le(b, times_at + i * 8)).filter(|t| *t != 0).map(filetime_ms).collect();
    let count = u32le(b, count_at).unwrap_or(0);
    let (names_at, names_len) = (u32le(b, 100).unwrap_or(0) as usize, u32le(b, 104).unwrap_or(0) as usize);
    let files: Vec<String> = b.get(names_at..names_at.saturating_add(names_len)).map(utf16le).unwrap_or_default().split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
    Ok((name, count, times, files))
}

pub(super) fn prefetch(path: &Path, tr: Tr) -> Result<String> {
    let raw = super::read_limited(path, 32 << 20)?;
    let (name, count, times, files) = prefetch_parse(&prefetch_bytes(raw)?)?;
    let mut rows: Vec<Vec<String>> = vec![vec![tr("Özellik"), tr("Değer")], vec![tr("Program"), name], vec![tr("Çalıştırılma sayısı"), count.to_string()]];
    for (i, t) in times.iter().enumerate() {
        rows.push(vec![if i == 0 { tr("Son çalıştırma") } else { String::new() }, rf_core::format::datetime_full(*t)]);
    }
    let mut out = table(&rows);
    out.push_str(&format!("\n## {} · {}\n\n", tr("Yüklediği dosyalar"), files.len()));
    for f in files.iter().take(300) {
        out.push_str(&format!("- {}\n", cell(f)));
    }
    if files.len() > 300 {
        out.push_str("- …\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(s: &'static str) -> String {
        s.to_string()
    }

    #[test]
    fn variable_length_integers_and_columns() {
        assert_eq!(varint(&[0x7f]), (127, 1));
        assert_eq!(varint(&[0x81, 0x00]), (128, 2));
        assert_eq!(varint(&[0xff; 9]), (u64::MAX, 9));
        let (names, rowid) = columns("CREATE TABLE \"kişi\" (id INTEGER PRIMARY KEY, \"ad\" TEXT NOT NULL, puan REAL DEFAULT (1.5), PRIMARY KEY (id), CHECK (puan > 0))");
        assert_eq!(names, vec!["id", "ad", "puan"]);
        assert_eq!(rowid, Some(0));
    }

    /// A database made by hand, one page of schema and one of rows: read back as a table.
    #[test]
    fn sqlite_tables_and_rows() {
        // (a serial type over 127 takes two bytes)
        fn vi(n: u64) -> Vec<u8> {
            if n < 128 { vec![n as u8] } else { vec![(n >> 7) as u8 | 0x80, (n & 0x7f) as u8] }
        }
        fn record(values: &[(u64, Vec<u8>)]) -> Vec<u8> {
            let types: Vec<u8> = values.iter().flat_map(|(t, _)| vi(*t)).collect();
            let mut r = vec![types.len() as u8 + 1];
            r.extend(types);
            for (_, b) in values {
                r.extend_from_slice(b);
            }
            r
        }
        fn text(s: &str) -> (u64, Vec<u8>) {
            (13 + 2 * s.len() as u64, s.as_bytes().to_vec())
        }
        fn leaf(page: &mut [u8], h: usize, cells: &[(u8, Vec<u8>)]) {
            let size = page.len();
            page[h] = 13;
            page[h + 3..h + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
            let mut end = size;
            for (i, (rowid, rec)) in cells.iter().enumerate() {
                let cell: Vec<u8> = [vec![rec.len() as u8, *rowid], rec.clone()].concat();
                end -= cell.len();
                page[end..end + cell.len()].copy_from_slice(&cell);
                page[h + 8 + i * 2..h + 10 + i * 2].copy_from_slice(&(end as u16).to_be_bytes());
            }
        }
        const PAGE: usize = 512;
        let mut db = vec![0u8; PAGE * 2];
        db[..16].copy_from_slice(b"SQLite format 3\0");
        db[16..18].copy_from_slice(&(PAGE as u16).to_be_bytes());
        db[56..60].copy_from_slice(&1u32.to_be_bytes());
        let sql = "CREATE TABLE notlar (id INTEGER PRIMARY KEY, ad TEXT, puan INT)";
        let schema = record(&[text("table"), text("notlar"), text("notlar"), (1, vec![2]), text(sql)]);
        let (first, second) = db.split_at_mut(PAGE);
        leaf(first, 100, &[(1, schema)]);
        // the id column is the row id: stored as NULL in the row
        leaf(second, 0, &[(7, record(&[(0, vec![]), text("Özer"), (1, vec![90])])), (9, record(&[(0, vec![]), text("a|b"), (2, vec![0xff, 0xfe])]))]);
        let p = std::env::temp_dir().join(format!("rf-sqlite-{}.db", std::process::id()));
        std::fs::write(&p, &db).unwrap();
        let md = sqlite(&p, &same).unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(md.starts_with("**SQLite** · 1 tablo · 2 sayfa · 512 bayt/sayfa\n\n## notlar · 2 satır\n\n"), "{md}");
        assert!(md.contains("| id | ad | puan |\n| --- | --- | --- |\n| 7 | Özer | 90 |\n| 9 | a\\|b | -2 |\n"), "{md}");
    }

    #[test]
    fn crash_dump_summary() {
        // header, a directory of three streams, system info, exception, module list + a name
        let mut d = vec![0u8; 0x400];
        d[..4].copy_from_slice(b"MDMP");
        d[8..12].copy_from_slice(&3u32.to_le_bytes());
        d[12..16].copy_from_slice(&0x20u32.to_le_bytes());
        d[20..24].copy_from_slice(&1_794_000_000u32.to_le_bytes());
        let mut entry = |i: usize, kind: u32, size: u32, rva: u32| {
            let at = 0x20 + i * 12;
            d[at..at + 4].copy_from_slice(&kind.to_le_bytes());
            d[at + 4..at + 8].copy_from_slice(&size.to_le_bytes());
            d[at + 8..at + 12].copy_from_slice(&rva.to_le_bytes());
        };
        entry(0, 7, 56, 0x80);
        entry(1, 6, 168, 0x100);
        entry(2, 4, 4 + 108, 0x200);
        d[0x80..0x82].copy_from_slice(&9u16.to_le_bytes());
        d[0x86] = 16;
        d[0x88..0x8c].copy_from_slice(&10u32.to_le_bytes());
        d[0x90..0x94].copy_from_slice(&26100u32.to_le_bytes());
        d[0x108..0x10c].copy_from_slice(&0xC000_0005u32.to_le_bytes());
        d[0x118..0x120].copy_from_slice(&0x7ff6_0000_1234u64.to_le_bytes());
        d[0x200..0x204].copy_from_slice(&1u32.to_le_bytes());
        d[0x204..0x20c].copy_from_slice(&0x7ff6_0000_0000u64.to_le_bytes());
        d[0x20c..0x210].copy_from_slice(&0x10000u32.to_le_bytes());
        d[0x218..0x21c].copy_from_slice(&0x300u32.to_le_bytes());
        let name: Vec<u8> = r"C:\Program Files\x\oyun.exe".encode_utf16().flat_map(u16::to_le_bytes).collect();
        d[0x300..0x304].copy_from_slice(&(name.len() as u32).to_le_bytes());
        d[0x304..0x304 + name.len()].copy_from_slice(&name);
        let p = std::env::temp_dir().join(format!("rf-dump-{}.dmp", std::process::id()));
        std::fs::write(&p, &d).unwrap();
        let md = minidump(&p, &same).unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(md.contains("| İşlem | oyun.exe |"), "{md}");
        assert!(md.contains("| Sistem | Windows 10.0.26100 · x64 · 16 işlemci |"), "{md}");
        assert!(md.contains("| Hata | 0xC0000005 (Access violation) |"), "{md}");
        assert!(md.contains("| Hatanın yeri | oyun.exe + 0x1234 |"), "{md}");
        assert!(md.contains("## Modüller · 1\n\n- oyun.exe\n"), "{md}");
    }

    #[test]
    fn prefetch_summary() {
        // a version 30 file as Windows 10 / 11 write them (uncompressed here)
        let mut b = vec![0u8; 0x400];
        b[..4].copy_from_slice(&30u32.to_le_bytes());
        b[4..8].copy_from_slice(b"SCCA");
        let name: Vec<u8> = "NOTEPAD.EXE".encode_utf16().flat_map(u16::to_le_bytes).collect();
        b[16..16 + name.len()].copy_from_slice(&name);
        b[84..88].copy_from_slice(&304u32.to_le_bytes());
        let files: Vec<u8> = "\\VOLUME{1}\\WINDOWS\\NOTEPAD.EXE\0\\VOLUME{1}\\WINDOWS\\SYSTEM32\\NTDLL.DLL\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        b[100..104].copy_from_slice(&0x300u32.to_le_bytes());
        b[104..108].copy_from_slice(&(files.len() as u32).to_le_bytes());
        b[0x300..0x300 + files.len()].copy_from_slice(&files);
        // 2026-01-01 00:00:00 UTC as a FILETIME
        let ft = (1_767_225_600_000i64 + 11_644_473_600_000) as u64 * 10_000;
        b[128..136].copy_from_slice(&ft.to_le_bytes());
        b[208..212].copy_from_slice(&42u32.to_le_bytes());
        let (name, count, times, files) = prefetch_parse(&b).unwrap();
        assert_eq!((name.as_str(), count, times, files.len()), ("NOTEPAD.EXE", 42, vec![1_767_225_600_000], 2));
        assert!(files[1].ends_with("NTDLL.DLL"));
        assert!(prefetch_parse(b"not a prefetch file").is_err());
    }
}
