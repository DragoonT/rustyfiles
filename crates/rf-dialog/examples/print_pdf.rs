//! Prints a line to "Microsoft Print to PDF" with this build of the dialog serving
//! FileSaveDialog in this process only (CoRegisterClassObject; the registry is not touched), so
//! the driver's "save the PDF as" request goes through it. Prints what the print calls returned;
//! the dialog's own steps go to dialog.log. rustyfiles-exe.txt next to this exe names the
//! rustyfiles.exe that shows the picker.
//! `cargo run --release -p rf-dialog --example print_pdf`

#[cfg(windows)]
fn main() {
    use windows::Win32::Graphics::Gdi::{CreateDCW, DeleteDC, TextOutW};
    use windows::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoInitializeEx, CoRegisterClassObject, IClassFactory, REGCLS_MULTIPLEUSE};
    use windows::Win32::UI::Shell::FileSaveDialog;
    use windows::core::{GUID, IUnknown, Interface, PCWSTR, w};

    // the picker the dialog starts inherits this: a test copy with its own settings, which
    // leaves the user's registrations alone
    let cfg = std::env::temp_dir().join("rf-print-pdf-cfg");
    let _ = std::fs::create_dir_all(&cfg);
    unsafe { std::env::set_var("RUSTYFILES_CONFIG_DIR", &cfg) };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut f: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = rustyfiles_dialog::DllGetClassObject(&FileSaveDialog as *const GUID, &IClassFactory::IID, &mut f);
        assert!(hr.is_ok(), "class object: {hr:?}");
        let factory = IClassFactory::from_raw(f);
        let unk: IUnknown = factory.cast().unwrap();
        let cookie = CoRegisterClassObject(&FileSaveDialog, &unk, CLSCTX_INPROC_SERVER, REGCLS_MULTIPLEUSE).expect("register");
        println!("FileSaveDialog served by this build (cookie {cookie})");

        let dc = CreateDCW(w!("WINSPOOL"), w!("Microsoft Print to PDF"), PCWSTR::null(), None);
        assert!(!dc.is_invalid(), "no printer DC");
        let di = DOCINFOW { cbSize: std::mem::size_of::<DOCINFOW>() as i32, lpszDocName: w!("rustyfiles print test"), ..Default::default() };
        let job = StartDocW(dc, &di);
        println!("StartDoc -> {job}");
        if job > 0 {
            StartPage(dc);
            let text: Vec<u16> = "rustyfiles print test".encode_utf16().collect();
            let _ = TextOutW(dc, 200, 200, &text);
            EndPage(dc);
            println!("EndDoc -> {}", EndDoc(dc));
        }
        let _ = DeleteDC(dc);
    }
}

#[cfg(not(windows))]
fn main() {}
