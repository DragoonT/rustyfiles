//! Loads rustyfiles_dialog.dll straight from the build folder (no registry) and shows its Open,
//! Save or folder dialog like an app would; prints what came back.
//!   probe open|multi|save|folder|both|plainsink <start folder>
//!
//! `both`: an Open dialog with an event sink of the JetBrains runtime's kind (IntelliJ IDEA's "Open File or Project", issue 17):
//! it names the OK button "Select Folder" while a folder is selected and "Open" for a file, and reads the answer from the selected
//! items in OnFileOk. `plainsink`: a sink that renames nothing, as most apps' are.
//!
//! WITHOUT A WINDOW. `RF_PROBE_DLL=<path>` loads that copy of the DLL instead. Put the copy in a folder of its own with a
//! `rustyfiles-exe.txt` beside it that names THIS program, and the DLL starts this program as the picker (`probe --pick <pipe>`): it
//! prints the request it was given and answers what `RF_PROBE_ANSWER` says: `folder` (the folder the request starts in),
//! `file:<path>`, or anything else for Cancel.
use std::ffi::c_void;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IClassFactory};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemServices::SFGAO_FOLDER;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::core::{GUID, HRESULT, HSTRING, IUnknown, Interface, Ref, Result, implement, w};

/// The app's side of the dialog's events.
#[implement(IFileDialogEvents)]
struct Sink {
    /// Names the OK button after what is selected, as the JetBrains runtime does.
    renames: bool,
}

impl IFileDialogEvents_Impl for Sink_Impl {
    fn OnFileOk(&self, pfd: Ref<IFileDialog>) -> Result<()> {
        let d = pfd.ok()?;
        unsafe {
            let arr = d.cast::<IFileOpenDialog>()?.GetSelectedItems()?;
            for i in 0..arr.GetCount()? {
                let p = arr.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
                println!("SINK OnFileOk selected {}", p.to_string().unwrap_or_default());
                CoTaskMemFree(Some(p.0 as *const c_void));
            }
        }
        Ok(())
    }
    fn OnFolderChanging(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn OnFolderChange(&self, _: Ref<IFileDialog>) -> Result<()> {
        // (after this one the JetBrains runtime takes a cancelled dialog for "use the open folder": our dialog must not send it)
        println!("SINK OnFolderChange");
        Ok(())
    }
    fn OnSelectionChange(&self, pfd: Ref<IFileDialog>) -> Result<()> {
        if !self.renames {
            return Ok(());
        }
        let d = pfd.ok()?;
        unsafe {
            let folder = d.GetCurrentSelection()?.GetAttributes(SFGAO_FOLDER).is_ok_and(|a| a.0 & SFGAO_FOLDER.0 != 0);
            d.SetOkButtonLabel(if folder { w!("Select Folder") } else { w!("Open") })?;
        }
        Ok(())
    }
    fn OnShareViolation(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<FDE_SHAREVIOLATION_RESPONSE> {
        Ok(FDESVR_DEFAULT)
    }
    fn OnTypeChange(&self, _: Ref<IFileDialog>) -> Result<()> {
        Ok(())
    }
    fn OnOverwrite(&self, _: Ref<IFileDialog>, _: Ref<IShellItem>) -> Result<FDE_OVERWRITE_RESPONSE> {
        Ok(FDEOR_DEFAULT)
    }
}

/// `probe --pick <pipe>`: stands in for rustyfiles' picker window. No window: the request is printed and answered.
fn stand_in_picker(pipe: &str) {
    use rustyfiles_dialog::proto::{Request, Response, read_msg, write_msg};
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pipe).expect("the request's pipe");
    let req: Request = read_msg(&mut f).expect("a request");
    println!("PICKER mode {:?}, ok label {:?}, folder label {:?}", req.mode, req.ok_label, req.folder_label);
    let answer = std::env::var("RF_PROBE_ANSWER").unwrap_or_default();
    let resp = match answer.as_str() {
        "folder" => Response { ok: true, paths: req.folder.clone().into_iter().collect(), filter_index: req.filter_index },
        a if a.starts_with("file:") => Response { ok: true, paths: vec![a["file:".len()..].to_string()], filter_index: req.filter_index },
        _ => Response { ok: false, paths: Vec::new(), filter_index: req.filter_index },
    };
    write_msg(&mut f, &resp).expect("the answer");
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--pick") {
        stand_in_picker(args.get(2).map(String::as_str).unwrap_or_default());
        return Ok(());
    }
    let mode = args.get(1).map(String::as_str).unwrap_or("open");
    let start = args.get(2).cloned().unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dll = std::env::var_os("RF_PROBE_DLL")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("rustyfiles_dialog.dll"));
        let lib = LoadLibraryW(&HSTRING::from(dll.as_os_str()))?;
        let f: unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(GetProcAddress(lib, windows::core::s!("DllGetClassObject")).unwrap());
        let clsid = if mode == "save" { FileSaveDialog } else { FileOpenDialog };
        let mut cf: *mut c_void = std::ptr::null_mut();
        f(&clsid, &IClassFactory::IID, &mut cf).ok()?;
        let cf = IClassFactory::from_raw(cf);
        let d: IFileDialog = cf.CreateInstance::<Option<&IUnknown>, IFileDialog>(None)?;
        d.SetTitle(w!("Probe"))?;
        let specs = [
            COMDLG_FILTERSPEC { pszName: w!("Text"), pszSpec: w!("*.txt;*.md") },
            COMDLG_FILTERSPEC { pszName: w!("All files"), pszSpec: w!("*.*") },
        ];
        if mode != "folder" {
            d.SetFileTypes(&specs)?;
            d.SetFileTypeIndex(1)?;
        }
        let mut opts = d.GetOptions()?;
        if mode == "folder" {
            opts |= FOS_PICKFOLDERS;
        }
        if mode == "multi" {
            opts |= FOS_ALLOWMULTISELECT;
        }
        if mode == "save" {
            d.SetFileName(w!("notlar"))?;
            d.SetDefaultExtension(w!("txt"))?;
        }
        d.SetOptions(opts)?;
        let folder: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(start.as_str()), None)?;
        d.SetFolder(&folder)?;
        if mode == "both" || mode == "plainsink" {
            let sink: IFileDialogEvents = Sink { renames: mode == "both" }.into();
            d.Advise(&sink)?;
        }
        match d.Show(None) {
            Ok(()) if mode == "multi" => {
                // multi-select apps read the array
                let arr = d.cast::<IFileOpenDialog>()?.GetResults()?;
                for i in 0..arr.GetCount()? {
                    let p = arr.GetItemAt(i)?.GetDisplayName(SIGDN_FILESYSPATH)?;
                    println!("RESULT {}", p.to_string().unwrap_or_default());
                    CoTaskMemFree(Some(p.0 as *const c_void));
                }
            }
            Ok(()) => {
                let item = d.GetResult()?;
                let p = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                println!("RESULT {} (type {})", p.to_string().unwrap_or_default(), d.GetFileTypeIndex().unwrap_or(0));
                CoTaskMemFree(Some(p.0 as *const c_void));
            }
            Err(e) => println!("CANCELLED {:?}", e.code()),
        }
    }
    Ok(())
}
