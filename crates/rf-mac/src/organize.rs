//! The folder organizer for the Mac app: rf-core's rule engine (`rf_core::organize`) with the
//! disk work it leaves to its caller. Rules travel as JSON in the shape of `organize::Rule`
//! (serde), so the Swift side can edit them as plain values; the plan comes back as JSON too.
//! Running the plan (moves, renames, the Trash) and undoing it are the app's: it has the undo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use rf_core::organize::{self, Dest, FileFact, FileKind, PlanContext, ProtectedPaths, Rule, Unusable};
use serde::Serialize;

static ENGLISH: AtomicBool = AtomicBool::new(true);

/// The organizer's words in English (on) or Turkish (off), with the app's language.
pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

/// The engine's texts are Turkish; the Windows app's string table gives their English.
fn tr(s: &'static str) -> String {
    if !ENGLISH.load(Ordering::Relaxed) {
        return s.to_string();
    }
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Pair {
            tr: String,
            en: String,
        }
        let pairs: Vec<Pair> = serde_json::from_str(include_str!("../../rf-app/locales/_source.json")).unwrap_or_default();
        pairs.into_iter().map(|p| (p.tr, p.en)).collect()
    });
    table.get(s).cloned().unwrap_or_else(|| s.to_string())
}

/// The kind of a file from its extension, as the Mac app's icons see it.
fn classify(ext: &str) -> FileKind {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "webp" | "tif" | "tiff" | "bmp" | "svg" | "raw" | "cr2" | "cr3" | "nef" | "arw"
        | "dng" | "psd" | "ico" | "avif" => FileKind::Image,
        "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" => FileKind::Video,
        "mp3" | "m4a" | "aac" | "wav" | "flac" | "aiff" | "aif" | "ogg" | "opus" | "wma" | "alac" => FileKind::Audio,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "pages" | "numbers" | "key" | "txt" | "rtf" | "md" | "odt" | "ods"
        | "odp" | "csv" | "epub" => FileKind::Document,
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "dmg" | "iso" | "zst" => FileKind::Archive,
        "app" | "pkg" | "mpkg" | "command" | "sh" | "exe" | "msi" => FileKind::Program,
        _ => FileKind::Other,
    }
}

/// The places a rule may not watch or fill on a Mac: the system, the apps, the user's Library,
/// the home folder itself, rustyfiles' own settings.
fn protected() -> ProtectedPaths {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut program_files: Vec<PathBuf> =
        ["/Applications", "/Library", "/usr", "/bin", "/sbin", "/opt", "/private", "/Volumes/Macintosh HD"].iter().map(PathBuf::from).collect();
    if let Some(h) = &home {
        program_files.push(h.join("Applications"));
    }
    ProtectedPaths {
        windows: Some(PathBuf::from("/System")),
        program_files,
        user_profile: home.clone(),
        app_data: home.iter().map(|h| h.join("Library")).collect(),
        app_dirs: Vec::new(),
        app_files: Vec::new(),
    }
}

#[derive(Serialize)]
struct OutStep {
    source: String,
    /// The new full path; absent: to the Trash.
    dest: Option<String>,
    rule: String,
}

#[derive(Serialize)]
struct OutPlan {
    steps: Vec<OutStep>,
    skipped: Vec<(String, String, String)>,
    loops: Vec<String>,
    unusable: Vec<(String, String)>,
    in_place: usize,
    ignored: usize,
}

/// The files rules look at: each enabled rule's folder (and the folders below it when the rule
/// says so), one stat per file. Hidden files are marked: the engine leaves them alone.
fn facts(rules: &[Rule]) -> Vec<FileFact> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    fn ms(t: std::io::Result<std::time::SystemTime>) -> i64 {
        t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
    }
    fn walk(dir: &Path, deep: bool, out: &mut Vec<FileFact>, seen: &mut std::collections::HashSet<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            // a package (an app, a bundle) is one file to the user
            let package = meta.is_dir() && name.contains('.') && !name.starts_with('.');
            if meta.is_dir() && !package {
                if deep && !name.starts_with('.') {
                    walk(&p, deep, out, seen);
                }
                continue;
            }
            if !seen.insert(p.clone()) {
                continue;
            }
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
            let mut f = FileFact::new(&p, if package { 0 } else { meta.len() }, ms(meta.modified()), ms(meta.created()), classify(&ext));
            f.hidden = name.starts_with('.');
            out.push(f);
        }
    }
    for r in rules.iter().filter(|r| r.enabled) {
        walk(&r.folder, r.subfolders, &mut out, &mut seen);
    }
    out
}

/// The plan for these rules (JSON array of `organize::Rule`); `only`: just the rule with this id.
#[uniffi::export]
pub fn organize_plan(rules_json: String, only: Option<u32>) -> String {
    let mut rules: Vec<Rule> = serde_json::from_str(&rules_json).unwrap_or_default();
    if let Some(id) = only {
        rules.retain(|r| r.id == id);
    }
    // folders a Mac must not have sorted for it
    let prot = protected();
    let exists = |p: &Path| p.exists();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut ctx = PlanContext::new(now, classify, &exists, &prot);
    ctx.kind_labels = FileKind::ALL.map(|k| tr(kind_word(k)));
    let mut unusable_extra = Vec::new();
    rules.retain(|r| {
        // the root of the disk, or a volume's root: never
        let root = r.folder == Path::new("/") || (r.folder.parent() == Some(Path::new("/Volumes")));
        if root && r.enabled {
            unusable_extra.push((r.name.clone(), tr("Bir sürücünün kökü düzenlenemez: daha belirli bir klasör seçin")));
        }
        !root
    });
    let facts = facts(&rules);
    let plan = organize::plan(&rules, &facts, &ctx);
    let out = OutPlan {
        steps: plan
            .steps
            .iter()
            .map(|s| OutStep {
                source: s.source.display().to_string(),
                dest: match &s.dest {
                    Dest::Move(p) => Some(p.display().to_string()),
                    Dest::Recycle => None,
                },
                rule: s.rule_name.clone(),
            })
            .collect(),
        skipped: plan.skipped.iter().map(|s| (s.source.display().to_string(), s.rule_name.clone(), tr(s.reason.text()))).collect(),
        loops: plan.loops.iter().map(|(_, n)| n.clone()).collect(),
        unusable: plan
            .unusable
            .iter()
            .map(|(_, n, why)| {
                let text = match why {
                    Unusable::Problem(p) => tr(p.text()),
                    Unusable::Refused(r) => tr(r.text()),
                };
                (n.clone(), text)
            })
            .chain(unusable_extra)
            .collect(),
        in_place: plan.in_place,
        ignored: plan.ignored,
    };
    serde_json::to_string(&out).unwrap_or_else(|_| "{}".into())
}

fn kind_word(k: FileKind) -> &'static str {
    match k {
        FileKind::Image => "Resimler",
        FileKind::Video => "Videolar",
        FileKind::Audio => "Sesler",
        FileKind::Document => "Belgeler",
        FileKind::Archive => "Arşivler",
        FileKind::Program => "Programlar",
        FileKind::Other => "Diğer",
    }
}

/// A rule in one line ("pictures older than 30 days → Pictures/{year}").
#[uniffi::export]
pub fn organize_summary(rule_json: String) -> String {
    let Ok(rule) = serde_json::from_str::<Rule>(&rule_json) else { return String::new() };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    organize::summary::summary(&rule, &tr, home.as_deref())
}

/// The template variables a destination or a new name may use ({year}, {name}…).
#[uniffi::export]
pub fn organize_template_vars() -> Vec<String> {
    organize::TEMPLATE_VARS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_moves_by_extension_and_leaves_hidden_files() {
        let d = std::env::temp_dir().join(format!("rf-mac-organize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for n in ["a.jpg", "b.png", "c.txt", ".hidden.jpg"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let rules = format!(
            r#"[{{"id":1,"name":"Pictures","enabled":true,"folder":"{}","conditions":[{{"type":"extension","list":["jpg","png"]}}],"action":{{"type":"move_to","folder":"Pictures"}}}}]"#,
            d.display()
        );
        let plan: serde_json::Value = serde_json::from_str(&organize_plan(rules, None)).unwrap();
        let steps = plan["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2, "{plan}");
        for s in steps {
            let dest = s["dest"].as_str().unwrap();
            assert!(dest.starts_with(&d.join("Pictures").display().to_string()), "{dest}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_summary_is_english_when_asked() {
        set_english(true);
        let s = organize_summary(r#"{"id":1,"name":"x","folder":"/tmp/x","action":{"type":"recycle"}}"#.into());
        assert!(!s.is_empty());
        assert!(!s.contains("Geri Dönüşüm"), "{s}");
    }
}
