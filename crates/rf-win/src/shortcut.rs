//! `.lnk` shortcuts via IShellLinkW + IPersistFile. Blocking + COM: call off the UI thread.

use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{Interface, PCWSTR};

use crate::shell::ensure_com;
use crate::wide::{from_wide, wide};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutInfo {
    /// Empty when the target is not a file-system path (e.g. a Control Panel item).
    pub target: PathBuf,
    pub args: String,
    pub working_dir: PathBuf,
    /// Icon file ("" = use the target's icon) and its resource index.
    pub icon: String,
    pub icon_index: i32,
}

/// Reads a `.lnk` file.
pub fn resolve(path: &Path) -> anyhow::Result<ShortcutInfo> {
    ensure_com();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        let w = wide(path);
        file.Load(PCWSTR(w.as_ptr()), STGM_READ)?;

        let mut buf = vec![0u16; 32768];
        let mut fd = WIN32_FIND_DATAW::default();
        // SLGP_RAWPATH (0x4) keeps %env% variables unexpanded; we want the expanded one (flags 0).
        let _ = link.GetPath(&mut buf, &mut fd, 0);
        let target = from_wide(&buf);
        buf.fill(0);
        let _ = link.GetArguments(&mut buf);
        let args = from_wide(&buf);
        buf.fill(0);
        let _ = link.GetWorkingDirectory(&mut buf);
        let working_dir = from_wide(&buf);
        buf.fill(0);
        let mut icon_index = 0i32;
        let _ = link.GetIconLocation(&mut buf, &mut icon_index);
        let icon = from_wide(&buf);

        Ok(ShortcutInfo { target: target.into(), args, working_dir: working_dir.into(), icon, icon_index })
    }
}

/// Creates (or overwrites) `link_path` (should end in `.lnk`) pointing at `target`.
/// The working directory defaults to the target's folder (or the target itself for folders).
pub fn create(link_path: &Path, target: &Path) -> anyhow::Result<()> {
    ensure_com();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let t = wide(target);
        link.SetPath(PCWSTR(t.as_ptr()))?;
        let dir = if target.is_dir() { Some(target) } else { target.parent() };
        if let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            let dw = wide(d);
            let _ = link.SetWorkingDirectory(PCWSTR(dw.as_ptr()));
        }
        let file: IPersistFile = link.cast()?;
        let l = wide(link_path);
        file.Save(PCWSTR(l.as_ptr()), true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_resolve() {
        let dir = std::env::temp_dir().join(format!("rf-lnk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("hedef.txt");
        std::fs::write(&target, b"x").unwrap();
        let lnk = dir.join("hedef.lnk");
        create(&lnk, &target).unwrap();
        assert!(lnk.exists());
        let info = resolve(&lnk).unwrap();
        // compare real paths: the temp folder may come as a short 8.3 name (C:\Users\RUNNER~1)
        // while the shortcut resolves to the long one
        let same = |a: &std::path::Path, b: &std::path::Path| std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap();
        assert!(same(&info.target, &target), "{:?} vs {:?}", info.target, target);
        assert!(info.args.is_empty());
        assert!(same(&info.working_dir, &dir), "{:?} vs {:?}", info.working_dir, dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
