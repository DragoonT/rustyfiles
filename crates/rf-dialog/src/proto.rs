//! What the DLL asks rustyfiles and what it gets back, over a named pipe: a little-endian u32
//! length, then that many bytes of JSON.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Open,
    Save,
    /// Pick a folder (FOS_PICKFOLDERS).
    Folder,
}

/// One entry of the "file type" list: "Text files" and "*.txt;*.md".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub name: String,
    pub spec: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Request {
    pub mode: Mode,
    /// Several files may be picked (open only).
    pub multi: bool,
    pub title: String,
    pub ok_label: String,
    /// Folder to start in (the app's choice, else its default).
    pub folder: Option<String>,
    pub file_name: String,
    pub filters: Vec<Filter>,
    /// 1-based, like the COM API.
    pub filter_index: u32,
    /// Without the dot ("txt"); added on save when the name has no extension.
    pub default_ext: String,
    pub overwrite_prompt: bool,
    /// The calling app's window (the picker stays above it).
    pub owner: isize,
    /// The calling app's exe name, for the window title.
    pub app: String,
    /// Open only: the app takes a FOLDER as the answer too, and this is what its OK button says while a folder is the choice
    /// ("Select Folder"). Empty: files only, as an Open dialog is. See [`folder_label_from`]. (A DLL older than this field does not
    /// send it.)
    #[serde(default)]
    pub folder_label: String,
}

/// Whether an app that showed an Open dialog also takes a folder from it, from what its OK button says for each kind of selection.
///
/// Windows' Open dialog never answers with a folder: OK on a folder goes into it. Some programs make it answer with one anyway, by
/// hooking the dialog's window: the JetBrains runtime does (IntelliJ IDEA's "Open File or Project": a project is a folder), and it
/// renames the OK button to "Select Folder" while a folder is selected and back to "Open" for a file. The hook cannot work on our
/// window, but the renaming goes through the dialog object, which is ours: an app that gives the button ANOTHER label for a folder
/// than for a file means that OK chooses that folder. The result is the folder's label, or nothing when the app makes no difference
/// (it names the button once, or not at all). Pure.
pub fn folder_label_from(for_folder: &str, for_file: &str) -> String {
    let (folder, file) = (for_folder.trim(), for_file.trim());
    if folder.is_empty() || folder == file { String::new() } else { folder.to_string() }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    pub ok: bool,
    pub paths: Vec<String>,
    pub filter_index: u32,
}

pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_msg<T: for<'a> Deserialize<'a>>(r: &mut impl Read) -> std::io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 16 << 20 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Does `name` match one of the patterns of `spec` ("*.txt;*.md", "*.*", "*")?
pub fn matches(spec: &str, name: &str) -> bool {
    let name = name.to_lowercase();
    spec.split(';').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).any(|p| glob(&p, &name))
}

fn glob(p: &str, s: &str) -> bool {
    if p == "*" || p == "*.*" {
        return true;
    }
    // '*' any run, '?' one char
    let (p, s): (Vec<char>, Vec<char>) = (p.chars().collect(), s.chars().collect());
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The first concrete extension of a spec ("*.txt;*.md" -> "txt"), for adding to saved names.
pub fn first_ext(spec: &str) -> Option<String> {
    spec.split(';').map(str::trim).find_map(|p| p.strip_prefix("*.").filter(|e| !e.is_empty() && !e.contains(['*', '?'])).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_takes_folders_when_its_button_says_something_else_for_a_folder() {
        // the JetBrains runtime: "Select Folder" while a folder is selected, "Open" for a file
        assert_eq!(folder_label_from("Select Folder", "Open"), "Select Folder");
        assert_eq!(folder_label_from(" Klasör Seç ", "Aç"), "Klasör Seç");
        // only the folder gets a label
        assert_eq!(folder_label_from("Choose", ""), "Choose");
        // no difference, or no label at all: an ordinary Open dialog
        assert_eq!(folder_label_from("Open", "Open"), "");
        assert_eq!(folder_label_from("", ""), "");
        assert_eq!(folder_label_from("", "Open"), "");
        assert_eq!(folder_label_from("Import", " Import "), "");
    }

    #[test]
    fn a_request_from_an_older_dll_has_no_folder_label() {
        let mut v = serde_json::to_value(Request::default()).unwrap();
        v.as_object_mut().unwrap().remove("folder_label");
        let r: Request = serde_json::from_value(v).unwrap();
        assert_eq!(r.folder_label, "");
    }

    #[test]
    fn round_trip() {
        let r = Request { mode: Mode::Save, title: "Kaydet".into(), filters: vec![Filter { name: "Metin".into(), spec: "*.txt".into() }], filter_index: 1, ..Default::default() };
        let mut buf = Vec::new();
        write_msg(&mut buf, &r).unwrap();
        assert_eq!(read_msg::<Request>(&mut buf.as_slice()).unwrap(), r);
    }

    #[test]
    fn patterns() {
        assert!(matches("*.txt;*.md", "Notlar.MD"));
        assert!(!matches("*.txt;*.md", "a.rs"));
        assert!(matches("*.*", "README"));
        assert!(matches("rapor-??.pdf", "rapor-01.pdf"));
        assert!(!matches("rapor-??.pdf", "rapor-1.pdf"));
        assert_eq!(first_ext("*.jpg;*.jpeg"), Some("jpg".into()));
        assert_eq!(first_ext("*.*"), None);
    }
}
