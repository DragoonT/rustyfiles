//! Windows' own catalogue of settings, for the search: every Settings page, the settings inside the pages, and the Control Panel's
//! tasks and tools, with their names and search words in the user's display language.
//!
//! A user's report: the dock's search found no Device Manager, no sound or mouse settings, no installed apps, no Windows Update.
//! Windows' search finds them because Windows ships a catalogue for exactly that: the files `AllSystemSettings_*.xml` in
//! `%WINDIR%\ImmersiveControlPanel\Settings` (three of about 2 MB each on Windows 11). Nothing here is a list typed into our
//! source: the names come from those files, resolved by the system itself, so they are right for the build and the language.
//!
//! WHAT AN ENTRY IS (read from the files of build 26300, and measured: 1750 entries in the largest file, about 4800 names resolved
//! over the three, 1494 of them distinct, 2.8 s for all of it on one thread):
//!
//! * a MODERN entry has a path of nodes, the first of them a `Page` (`SettingsPageInstalledApps`), then groups. An entry whose path
//!   is the page alone is the page; the others are settings on that page. A page is opened by activating the Settings app with the
//!   argument `page=<page id>` (tried on the user's PC: `page=SettingsPageInstalledApps` opened Apps > Installed apps);
//! * a CLASSIC entry has a `DeepLink`: a Control Panel item by its canonical name, with an optional page after a backslash
//!   (`Microsoft.DefaultPrograms\pageFileAssoc`), a command line (`%windir%\system32\mmc.exe ...`), or a shell location (`shell:::{...}`);
//! * names (`Description`) and search words (`HighKeywords`, `Keywords`) are indirect strings (`@{package?ms-resource://...}`,
//!   `@shell32.dll,-24202`, several joined as `@@a@b`) that `SHLoadIndirectString` resolves; the words are separated by `;`.
//!
//! Left out: an entry whose name does not resolve (among them Windows Security's own pages, whose strings need the package's full
//! name), and one with a link of a form not listed above.
//!
//! COST. Reading and resolving takes seconds, so it happens once, on a low-priority thread, the first time the search asks
//! ([`get_or_start`] answers `None` until it is done and never waits), and the result is kept in a small file in the app's settings
//! folder that the next start reads instead. The file is thrown away when the catalogue files' sizes or dates or the display
//! language differ.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How an entry is opened.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Launch {
    /// A page of the Settings app, by its page id.
    Page(String),
    /// A Control Panel item by its canonical name, and the page inside it after a backslash, if any.
    Control(String),
    /// A command line: a program (with environment variables) and its arguments.
    Command(String),
    /// A shell location or a URI the shell opens.
    Shell(String),
}

impl Launch {
    /// One string for it (what a search row carries and remembers).
    pub fn encode(&self) -> String {
        match self {
            Launch::Page(p) => format!("page:{p}"),
            Launch::Control(c) => format!("cpl:{c}"),
            Launch::Command(c) => format!("cmd:{c}"),
            Launch::Shell(s) => format!("shell:{s}"),
        }
    }

    pub fn decode(s: &str) -> Option<Launch> {
        let (kind, rest) = s.split_once(':')?;
        if rest.is_empty() {
            return None;
        }
        Some(match kind {
            "page" => Launch::Page(rest.to_string()),
            "cpl" => Launch::Control(rest.to_string()),
            "cmd" => Launch::Command(rest.to_string()),
            "shell" => Launch::Shell(rest.to_string()),
            _ => return None,
        })
    }

    /// The entry belongs to the classic Control Panel and its tools (not to the Settings app).
    pub fn is_classic(&self) -> bool {
        !matches!(self, Launch::Page(_))
    }
}

/// One thing the search can find.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name in the display language ("Yüklü uygulamalar", "Aygıt Yöneticisi").
    pub name: String,
    /// For a setting on a Settings page: the page's name. Empty for a page itself and for a classic entry.
    pub place: String,
    /// Search words in the display language, separated by `;` (as the system gives them).
    pub keywords: String,
    pub launch: Launch,
}

// ---------------------------------------------------------------- reading the files (pure)

/// An entry as the file has it: the references are not resolved yet.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Raw {
    pub name_ref: String,
    pub keyword_refs: Vec<String>,
    /// The first node of the first path, when it is a page.
    pub page: Option<String>,
    /// How many nodes the first path has (1: the entry is the page itself).
    pub depth: usize,
    pub deep_link: Option<String>,
    pub page_id: Option<String>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The text of the first `<tag ...>text</tag>` in `block`.
fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(i) = block[from..].find(&open) {
        let at = from + i + open.len();
        // (not a longer tag that begins the same: `<Path` in `<PathX`)
        let next = block[at..].chars().next()?;
        if next == '>' || next == ' ' {
            let start = at + block[at..].find('>')? + 1;
            let end = start + block[start..].find(&format!("</{name}>"))?;
            return Some(block[start..end].trim());
        }
        from = at;
    }
    None
}

/// The entries of one catalogue file. A file of another shape gives fewer entries or none; nothing here can fail.
pub fn parse(xml: &str) -> Vec<Raw> {
    let mut out = Vec::new();
    for block in xml.split("<SearchableContent").skip(1) {
        let block = block.split("</SearchableContent>").next().unwrap_or("");
        let Some(name_ref) = tag(block, "Description").filter(|d| !d.is_empty()) else { continue };
        let mut raw = Raw { name_ref: unescape(name_ref), ..Raw::default() };
        for key in ["HighKeywords", "Keywords"] {
            if let Some(k) = tag(block, key).filter(|k| !k.is_empty()) {
                raw.keyword_refs.push(unescape(k));
            }
        }
        if let Some(path) = tag(block, "Path") {
            let nodes: Vec<(&str, &str)> = path
                .split("<Node").skip(1)
                .filter_map(|n| {
                    let kind = n.split("Type=\"").nth(1)?.split('"').next()?;
                    let text = n.split('>').nth(1)?.split('<').next()?.trim();
                    (!text.is_empty()).then_some((kind, text))
                })
                .collect();
            raw.depth = nodes.len();
            raw.page = nodes.first().filter(|(kind, _)| *kind == "Page").map(|(_, text)| unescape(text));
        }
        raw.deep_link = tag(block, "DeepLink").filter(|d| !d.is_empty()).map(unescape);
        raw.page_id = tag(block, "PageID").filter(|d| !d.is_empty()).map(|p| p.to_ascii_uppercase());
        out.push(raw);
    }
    out
}

/// Windows Security's own pages: their names need the package's full name to resolve and their links are that app's.
const DEFENDER_PAGE: &str = "{6E6DDBCB-9C89-434B-A994-D5F22239523B}";

/// How an entry is opened, or `None` for one this code does not know how to open.
pub fn classify(raw: &Raw) -> Option<Launch> {
    if let Some(link) = raw.deep_link.as_deref().map(str::trim) {
        if raw.page_id.as_deref() == Some(DEFENDER_PAGE) {
            return None;
        }
        let lower = link.to_ascii_lowercase();
        if lower.contains(".exe") {
            return Some(Launch::Command(link.to_string()));
        }
        if lower.starts_with("shell:") {
            return Some(Launch::Shell(link.to_string()));
        }
        // a canonical name: letters, digits and dots, then an optional page after a backslash
        let name = link.split('\\').next().unwrap_or("");
        if name.starts_with("Microsoft.") && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
            return Some(Launch::Control(link.to_string()));
        }
        return None;
    }
    raw.page.clone().filter(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).map(Launch::Page)
}

/// The references a keyword field holds: one indirect string, or several joined as `@@a@b`.
pub fn keyword_parts(field: &str) -> Vec<String> {
    match field.strip_prefix("@@") {
        Some(rest) => rest.split('@').filter(|p| !p.is_empty()).map(|p| format!("@{p}")).collect(),
        None => vec![field.to_string()],
    }
}

/// The entries of raw ones, with `resolve` turning a reference into text (the system's call in the app, a table in the tests):
/// names resolved, the page's name put on its settings, duplicates (the same name opening the same thing) dropped.
pub fn build(raws: &[Raw], resolve: &dyn Fn(&str) -> Option<String>) -> Vec<Entry> {
    let text = |r: &str| if r.starts_with('@') { resolve(r) } else { Some(r.to_string()) }.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    // the pages' own names first
    let mut pages: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for r in raws.iter().filter(|r| r.depth == 1 && r.deep_link.is_none()) {
        if let (Some(page), Some(name)) = (r.page.as_deref(), text(&r.name_ref)) {
            pages.entry(page).or_insert(name);
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in raws {
        let Some(launch) = classify(r) else { continue };
        let Some(name) = text(&r.name_ref) else { continue };
        if !seen.insert((name.to_lowercase(), launch.clone())) {
            continue;
        }
        let keywords: Vec<String> = r.keyword_refs.iter().flat_map(|k| keyword_parts(k)).filter_map(|k| text(&k)).collect();
        let place = match (&launch, r.depth) {
            (Launch::Page(p), d) if d > 1 => pages.get(p.as_str()).cloned().unwrap_or_default(),
            _ => String::new(),
        };
        out.push(Entry { name: clean(&name), place: clean(&place), keywords: clean(&keywords.join(";")), launch });
    }
    out
}

// ---------------------------------------------------------------- the small file

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// The file's text: a first line with the key the file is valid for, then one entry a line.
pub fn to_text(key: &str, entries: &[Entry]) -> String {
    let mut s = format!("rustyfiles-settings-catalogue\t1\t{}\n", clean(key));
    for e in entries {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", clean(&e.name), clean(&e.place), clean(&e.keywords), clean(&e.launch.encode())));
    }
    s
}

/// The entries of the file's text, when it is this version's and for this `key`.
pub fn from_text(key: &str, text: &str) -> Option<Vec<Entry>> {
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next()?.split('\t').collect();
    if head.len() != 3 || head[0] != "rustyfiles-settings-catalogue" || head[1] != "1" || head[2] != clean(key) {
        return None;
    }
    let mut out = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 4 {
            continue;
        }
        let Some(launch) = Launch::decode(f[3]) else { continue };
        out.push(Entry { name: f[0].to_string(), place: f[1].to_string(), keywords: f[2].to_string(), launch });
    }
    Some(out)
}

// ---------------------------------------------------------------- the system

fn catalogue_dir() -> Option<PathBuf> {
    std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map(|w| PathBuf::from(w).join("ImmersiveControlPanel").join("Settings"))
}

fn catalogue_files() -> Vec<PathBuf> {
    let Some(dir) = catalogue_dir() else { return Vec::new() };
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| { let n = n.to_string_lossy().to_ascii_lowercase(); n.starts_with("allsystemsettings_") && n.ends_with(".xml") }))
        .collect();
    v.sort();
    v
}

/// What the small file is valid for: the catalogue files' names, sizes and dates, and the display language.
fn key_now(files: &[PathBuf]) -> String {
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    let mut key = format!("lang{lang}");
    for f in files {
        let (len, time) = std::fs::metadata(f)
            .map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())))
            .unwrap_or((0, 0));
        key.push_str(&format!("|{}:{len}:{time}", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    key
}

fn resolve(reference: &str) -> Option<String> {
    use windows::Win32::UI::Shell::SHLoadIndirectString;
    use windows::core::PCWSTR;
    let from = crate::wide::wide(reference);
    let mut buf = vec![0u16; 4096];
    // SAFETY: a NUL-terminated input and a buffer of the stated length.
    unsafe { SHLoadIndirectString(PCWSTR(from.as_ptr()), &mut buf, None).ok()? };
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]);
    // (a reference that comes back unchanged was not resolved)
    (!s.is_empty() && !s.starts_with('@')).then_some(s)
}

/// Reads the catalogue: from the small file in `cache_dir` when it is valid, else from Windows' files (seconds), writing the small
/// file for the next time. Blocks: never on the UI thread.
pub fn load(cache_dir: &Path) -> Vec<Entry> {
    let files = catalogue_files();
    if files.is_empty() {
        return Vec::new();
    }
    let key = key_now(&files);
    let cache = cache_dir.join("settings-catalogue.tsv");
    if let Some(entries) = std::fs::read_to_string(&cache).ok().and_then(|t| from_text(&key, &t)) {
        return entries;
    }
    let began = std::time::Instant::now();
    let mut raws = Vec::new();
    for f in &files {
        // (a file larger than 32 MB is not the catalogue we know)
        if std::fs::metadata(f).is_ok_and(|m| m.len() <= 32 * 1024 * 1024) && let Ok(bytes) = std::fs::read(f) {
            raws.extend(parse(&String::from_utf8_lossy(&bytes)));
        }
    }
    let entries = build(&raws, &resolve);
    log::info!("settings catalogue: {} entries from {} raw ones of {} file(s) in {} ms", entries.len(), raws.len(), files.len(), began.elapsed().as_millis());
    if !entries.is_empty() {
        let tmp = cache.with_extension("tsv.tmp");
        if std::fs::write(&tmp, to_text(&key, &entries)).is_ok() {
            let _ = std::fs::rename(&tmp, &cache);
        }
    }
    entries
}

enum State {
    Idle,
    Loading,
    Ready(Arc<Vec<Entry>>),
}

static STATE: Mutex<State> = Mutex::new(State::Idle);

/// The catalogue, if it has been read; the first call starts the reading on a low-priority thread and answers `None`, and so does
/// every call until it is done. Never waits.
pub fn get_or_start(cache_dir: &Path) -> Option<Arc<Vec<Entry>>> {
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    match &*st {
        State::Ready(v) => return Some(v.clone()),
        State::Loading => return None,
        State::Idle => {}
    }
    *st = State::Loading;
    let dir = cache_dir.to_path_buf();
    let spawned = std::thread::Builder::new().name("rf-settings-catalogue".into()).spawn(move || {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
        // SAFETY: the calling thread's own pseudo handle.
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST) };
        let entries = Arc::new(load(&dir));
        *STATE.lock().unwrap_or_else(|p| p.into_inner()) = State::Ready(entries);
    });
    if spawned.is_err() {
        *st = State::Idle;
    }
    None
}

const SETTINGS_APP: &str = "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel";

/// A command line's program and arguments: the program ends with its `.exe`.
pub fn split_command(line: &str) -> (String, String) {
    let lower = line.to_ascii_lowercase();
    match lower.find(".exe") {
        Some(i) => {
            let end = i + 4;
            (line[..end].trim().trim_matches('"').to_string(), line[end..].trim_start_matches('"').trim().to_string())
        }
        None => (line.trim().to_string(), String::new()),
    }
}

/// A Control Panel link's canonical name and the page inside it, if any.
pub fn split_control(link: &str) -> (&str, Option<&str>) {
    match link.split_once('\\') {
        Some((name, page)) if !page.is_empty() => (name, Some(page)),
        Some((name, _)) => (name, None),
        None => (link, None),
    }
}

fn exec(file: &str, params: &str) -> anyhow::Result<()> {
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;
    let (f, p) = (crate::wide::wide(file), crate::wide::wide(params));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpFile: PCWSTR(f.as_ptr()),
        lpParameters: if params.is_empty() { PCWSTR::null() } else { PCWSTR(p.as_ptr()) },
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call.
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

/// Opens what the entry stands for. Blocks while the system starts it: off the UI thread.
pub fn open(launch: &Launch) -> anyhow::Result<()> {
    match launch {
        Launch::Page(page) => crate::activate::packaged(SETTINGS_APP, &format!("page={page}")),
        Launch::Control(link) => {
            let (name, page) = split_control(link);
            exec("control.exe", &match page {
                Some(p) => format!("/name {name} /page {p}"),
                None => format!("/name {name}"),
            })
        }
        Launch::Command(line) => {
            let line = rf_core::addrcmd::expand_vars(line, &|n| std::env::var(n).ok());
            let (program, args) = split_command(&line);
            exec(&program, &args)
        }
        Launch::Shell(what) => crate::shell::open(Path::new(what)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<PCSettings>
  <SearchableContent ExcludeWithFeature="X">
    <Filename>AAA_SettingsPageApps</Filename>
    <ApplicationInformation><Glyph>x</Glyph></ApplicationInformation>
    <SettingIdentity><SettingPaths><Path>
          <Node Type="Page">SettingsPageInstalledApps</Node>
        </Path></SettingPaths></SettingIdentity>
    <SettingInformation>
      <Description>@{windows?ms-resource://A/SearchResources/SettingsPageInstalledApps/Description}</Description>
      <HighKeywords>@{windows?ms-resource://A/SearchResources/SettingsPageInstalledApps/HighKeywords}</HighKeywords>
    </SettingInformation>
  </SearchableContent>
  <SearchableContent>
    <Filename>AAA_Uninstall</Filename>
    <SettingIdentity><SettingPaths>
        <Path>
          <Node Type="Page">SettingsPageInstalledApps</Node>
          <Node Type="Group">G1</Node>
        </Path>
        <Path><Node Type="Page">Other</Node></Path>
      </SettingPaths></SettingIdentity>
    <SettingInformation><Description>@uninstall</Description></SettingInformation>
  </SearchableContent>
  <SearchableContent>
    <Filename>Classic_DevMgr</Filename>
    <ApplicationInformation><DeepLink>%windir%\system32\mmc.exe %windir%\system32\devmgmt.msc</DeepLink></ApplicationInformation>
    <SettingIdentity><PageID>{74246bfc-4c96-11d0-abef-0020af6b0b7a}</PageID></SettingIdentity>
    <SettingInformation><Description>@devmgr.dll,-4</Description><Keywords>@@a.dll,-1@b.dll,-2</Keywords></SettingInformation>
  </SearchableContent>
  <SearchableContent>
    <Filename>Classic_Assoc</Filename>
    <ApplicationInformation><DeepLink>Microsoft.DefaultPrograms\pageFileAssoc</DeepLink></ApplicationInformation>
    <SettingInformation><Description>Plain &amp; simple</Description></SettingInformation>
  </SearchableContent>
  <SearchableContent>
    <Filename>Defender_X</Filename>
    <ApplicationInformation><DeepLink>RansomwareProtection</DeepLink></ApplicationInformation>
    <SettingIdentity><PageID>{6E6DDBCB-9C89-434B-A994-D5F22239523B}</PageID></SettingIdentity>
    <SettingInformation><Description>@defender</Description></SettingInformation>
  </SearchableContent>
  <SearchableContent><Filename>NoName</Filename></SearchableContent>
  <SearchableContent>
    <Filename>Shell_X</Filename>
    <ApplicationInformation><DeepLink>shell:::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}</DeepLink></ApplicationInformation>
    <SettingInformation><Description>@unknown</Description></SettingInformation>
  </SearchableContent>
</PCSettings>"#;

    fn table(r: &str) -> Option<String> {
        Some(
            match r {
                "@{windows?ms-resource://A/SearchResources/SettingsPageInstalledApps/Description}" => "Yüklü uygulamalar",
                "@{windows?ms-resource://A/SearchResources/SettingsPageInstalledApps/HighKeywords}" => "uygulama kaldır;program kaldır",
                "@uninstall" => "Uygulama kaldır",
                "@devmgr.dll,-4" => "Aygıt Yöneticisi",
                "@a.dll,-1" => "donanım;sürücü",
                "@b.dll,-2" => "device;driver",
                "@defender" => "Fidye yazılımı koruması",
                _ => return None,
            }
            .to_string(),
        )
    }

    #[test]
    fn the_files_entries_are_read_with_their_pages_links_and_references() {
        let raws = parse(SAMPLE);
        assert_eq!(raws.len(), 6, "the entry without a name is left out");
        assert_eq!(raws[0].page.as_deref(), Some("SettingsPageInstalledApps"));
        assert_eq!((raws[0].depth, raws[0].keyword_refs.len()), (1, 1));
        assert_eq!((raws[1].depth, raws[1].page.as_deref()), (2, Some("SettingsPageInstalledApps")), "the first path counts");
        assert_eq!(raws[2].deep_link.as_deref(), Some(r"%windir%\system32\mmc.exe %windir%\system32\devmgmt.msc"));
        assert_eq!(raws[2].page_id.as_deref(), Some("{74246BFC-4C96-11D0-ABEF-0020AF6B0B7A}"));
        assert_eq!(raws[3].name_ref, "Plain & simple", "entities are decoded");
        // a text that is not the catalogue: nothing, and no panic
        assert!(parse("").is_empty() && parse("<html><SearchableContent").is_empty() && parse("<SearchableContent><Description>").is_empty());
    }

    #[test]
    fn an_entry_is_opened_by_its_kind_and_the_unknown_ones_are_left_out() {
        let raws = parse(SAMPLE);
        assert_eq!(classify(&raws[0]), Some(Launch::Page("SettingsPageInstalledApps".into())));
        assert_eq!(classify(&raws[2]), Some(Launch::Command(r"%windir%\system32\mmc.exe %windir%\system32\devmgmt.msc".into())));
        assert_eq!(classify(&raws[3]), Some(Launch::Control(r"Microsoft.DefaultPrograms\pageFileAssoc".into())));
        assert_eq!(classify(&raws[4]), None, "Windows Security's own pages");
        assert_eq!(classify(&raws[5]), Some(Launch::Shell("shell:::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}".into())));
        // a link of no known form, and a page id that is not an id
        assert_eq!(classify(&Raw { deep_link: Some("SomethingElse /x".into()), ..Raw::default() }), None);
        assert_eq!(classify(&Raw { page: Some("a b&c".into()), depth: 1, ..Raw::default() }), None);
        assert_eq!(classify(&Raw::default()), None);
    }

    #[test]
    fn names_are_resolved_settings_carry_their_pages_name_and_what_does_not_resolve_is_left_out() {
        let entries = build(&parse(SAMPLE), &table);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Yüklü uygulamalar", "Uygulama kaldır", "Aygıt Yöneticisi", "Plain & simple"], "the unresolved shell entry and the Defender page are out");
        assert_eq!(entries[0].place, "", "a page itself");
        assert_eq!(entries[0].keywords, "uygulama kaldır;program kaldır");
        assert_eq!(entries[1].place, "Yüklü uygulamalar", "a setting says which page it is on");
        assert_eq!(entries[1].launch, Launch::Page("SettingsPageInstalledApps".into()), "and opens that page");
        assert_eq!(entries[2].keywords, "donanım;sürücü;device;driver", "several references are joined");
        assert!(entries[2].launch.is_classic() && !entries[0].launch.is_classic());
        // the same file read twice (the three files overlap): one entry each
        let twice = [parse(SAMPLE), parse(SAMPLE)].concat();
        assert_eq!(build(&twice, &table).len(), 4);
    }

    #[test]
    fn the_small_file_gives_back_what_was_written_and_only_for_its_key() {
        let entries = build(&parse(SAMPLE), &table);
        let text = to_text("lang1055|a.xml:1:2", &entries);
        assert_eq!(from_text("lang1055|a.xml:1:2", &text), Some(entries.clone()));
        assert_eq!(from_text("lang1033|a.xml:1:2", &text), None, "another language");
        assert_eq!(from_text("lang1055|a.xml:1:3", &text), None, "a changed file");
        assert_eq!(from_text("k", "something else\n"), None);
        assert_eq!(from_text("k", ""), None);
        // a broken line is skipped, the rest is read
        let broken = format!("{}garbage\n", to_text("k", &entries[..1]));
        assert_eq!(from_text("k", &broken).map(|v| v.len()), Some(1));
        // a tab or a line break in a name cannot break the file
        let odd = vec![Entry { name: "a\tb\nc".into(), place: String::new(), keywords: String::new(), launch: Launch::Page("P".into()) }];
        assert_eq!(from_text("k", &to_text("k", &odd)).unwrap()[0].name, "a b c");
    }

    #[test]
    fn a_launch_is_one_string_and_comes_back_from_it() {
        for l in [Launch::Page("SettingsPageX".into()), Launch::Control(r"Microsoft.Sound\1".into()), Launch::Command(r"%windir%\system32\mmc.exe a.msc".into()), Launch::Shell("shell:::{A}".into())] {
            assert_eq!(Launch::decode(&l.encode()), Some(l));
        }
        assert_eq!(Launch::decode("page:"), None);
        assert_eq!(Launch::decode("other:x"), None);
        assert_eq!(Launch::decode("nothing"), None);
    }

    #[test]
    fn a_command_line_and_a_control_panel_link_are_taken_apart() {
        assert_eq!(split_command(r"C:\Windows\system32\mmc.exe C:\Windows\system32\devmgmt.msc"), (r"C:\Windows\system32\mmc.exe".into(), r"C:\Windows\system32\devmgmt.msc".into()));
        assert_eq!(split_command(r"C:\Windows\system32\rundll32.exe shell32.dll,Control_RunDLL x.cpl"), (r"C:\Windows\system32\rundll32.exe".into(), "shell32.dll,Control_RunDLL x.cpl".into()));
        assert_eq!(split_command(r#""C:\Program Files\a.exe" /x"#), (r"C:\Program Files\a.exe".into(), "/x".into()));
        assert_eq!(split_command(r"C:\Windows\system32\dfrgui.exe"), (r"C:\Windows\system32\dfrgui.exe".into(), String::new()));
        assert_eq!(split_control(r"Microsoft.DefaultPrograms\pageFileAssoc"), ("Microsoft.DefaultPrograms", Some("pageFileAssoc")));
        assert_eq!(split_control("Microsoft.System"), ("Microsoft.System", None));
        assert_eq!(keyword_parts("@@a.dll,-1@b.dll,-2"), ["@a.dll,-1", "@b.dll,-2"]);
        assert_eq!(keyword_parts("@{x?ms-resource://y}"), ["@{x?ms-resource://y}"]);
    }

    /// Read-only, on this PC's own catalogue (run with `--ignored --nocapture`): counts and times only.
    #[test]
    #[ignore]
    fn probe_the_real_catalogue() {
        let dir = std::env::temp_dir().join(format!("rf-settingscat-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = std::time::Instant::now();
        let first = load(&dir);
        let built = t.elapsed();
        let t = std::time::Instant::now();
        let again = load(&dir);
        println!("entries {} (classic {}), built in {built:?}, from the small file in {:?}, file {} KB", first.len(), first.iter().filter(|e| e.launch.is_classic()).count(), t.elapsed(), std::fs::metadata(dir.join("settings-catalogue.tsv")).map_or(0, |m| m.len() / 1024));
        assert_eq!(first, again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
