//! What the Start panel does with one app of "All apps": start it, start it as administrator, find its file.
//!
//! Every call goes through the one shell thread (`crate::sta::call`, with a limit): worker threads that call the shell hang it (see
//! that function's comment). So all of these may be called from any thread; they block until the shell thread answers or the limit
//! passes.

use std::path::PathBuf;
use std::time::Duration;

use crate::apps::App;

/// How long the shell thread may take to start something before the caller stops waiting (the start itself goes on).
const START_LIMIT: Duration = Duration::from_secs(15);
const LOOK_LIMIT: Duration = Duration::from_secs(4);

/// Starts the app, as a double click on it in Explorer's "All apps" would.
pub fn launch(app: &App) -> anyhow::Result<()> {
    let app = app.clone();
    crate::sta::call(START_LIMIT, move || crate::apps::launch(&app)).unwrap_or_else(|| anyhow::bail!("the shell did not answer in {START_LIMIT:?}"))
}

/// The program or shortcut the app starts (`System.Link.TargetParsingPath` of its AppsFolder item). `None` for packaged apps, which
/// have none, and for anything the shell does not say.
pub fn location(app: &App) -> Option<PathBuf> {
    let parsing = app.parsing();
    crate::sta::call(LOOK_LIMIT, move || target_of(&parsing)).flatten()
}

pub(crate) fn target_of(parsing: &str) -> Option<PathBuf> {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::{GPS_DEFAULT, IPropertyStore, SHGetPropertyStoreFromParsingName};
    crate::shell::ensure_com();
    let w = crate::wide::wide(parsing);
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreFromParsingName(windows::core::PCWSTR(w.as_ptr()), None, GPS_DEFAULT).ok()?;
        let key = PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xB9B4B3FC_2B51_4A42_B5D8_324146AFCF25), pid: 2 };
        let v = store.GetValue(&key).ok()?;
        let s = PropVariantToStringAlloc(&v).ok()?;
        let out = s.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
        out.filter(|t| !t.trim().is_empty()).map(PathBuf::from)
    }
}

/// Starts the app with administrator rights. Windows shows its own consent prompt (UAC) because the USER asked for the elevation
/// by choosing this item: that is the point, and nothing else in the dock ever elevates. A classic app is started from its program
/// when the shell names one, anything else by its place in "All apps".
pub fn launch_as_admin(app: &App) -> anyhow::Result<()> {
    let app = app.clone();
    crate::sta::call(START_LIMIT, move || run_as_admin(&app)).unwrap_or_else(|| anyhow::bail!("the shell did not answer in {START_LIMIT:?}"))
}

/// Whether a failure is the user answering "No" to Windows' consent prompt (`ERROR_CANCELLED`): not an error worth a message.
pub fn is_cancelled(e: &anyhow::Error) -> bool {
    use windows::core::HRESULT;
    e.chain().any(|c| c.downcast_ref::<windows::core::Error>().is_some_and(|w| w.code() == HRESULT::from_win32(1223)))
}

fn run_as_admin(app: &App) -> anyhow::Result<()> {
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};
    crate::shell::ensure_com();
    let target = target_of(&app.parsing()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && p.is_file());
    let file = match &target {
        Some(p) => crate::wide::wide(p.as_os_str()),
        None => crate::wide::wide(app.parsing()),
    };
    let dir = target.as_ref().and_then(|p| p.parent()).map(crate::wide::wide);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::HRESULT;

    #[test]
    fn declining_the_consent_prompt_is_recognised_through_added_context_and_nothing_else_is() {
        let declined = anyhow::Error::from(windows::core::Error::from_hresult(HRESULT::from_win32(1223)));
        assert!(is_cancelled(&declined));
        assert!(is_cancelled(&declined.context("starting Paint as administrator")));
        let other = anyhow::Error::from(windows::core::Error::from_hresult(HRESULT::from_win32(2)));
        assert!(!is_cancelled(&other));
        assert!(!is_cancelled(&anyhow::anyhow!("the shell did not answer")));
    }
}
