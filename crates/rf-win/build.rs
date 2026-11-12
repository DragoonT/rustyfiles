//! Compiles the glass shaders (`src/glass/glass.hlsl`) at build time, so the program does not have to at every start.
//!
//! WHY. `D3DCompile` of the four entry points took about 160 ms of CPU on the engine thread (the pixel shader of the glass alone about
//! 150 ms) and added about 2 to 5 MB of working set, in every process that attaches glass: the app, and the dock as a process of its own,
//! and again whenever the engine went idle and came back. The bytecode is only about 36 KB and does not depend on the graphics driver
//! (shader model 5.0), so it can be made once, here, and embedded (`glass/gpu.rs` includes the files written below).
//!
//! HOW. `d3dcompiler_47.dll` from System32 is loaded and `D3DCompile` called through a hand-written declaration (no build dependency:
//! the `windows` crate would be built a second time for the host just for this). The flags are those `gpu.rs::compile` uses for the
//! runtime fallback: optimisation level 3, and for the strict pixel shader IEEE strictness. A test in `gpu.rs` compiles at run time and
//! checks the embedded bytes are the same, so the two cannot drift apart.
//!
//! IF IT CANNOT BE DONE (the DLL is missing, the source has an error) the file is written empty and a build warning says why: the program
//! then compiles that shader when it starts, as it did before, and reports the compiler's message like it always has.

use std::path::{Path, PathBuf};

/// What gets compiled: output file, entry point, target profile, extra flags. Keep in step with `Renderer::new` in `glass/gpu.rs`.
const SHADERS: [(&str, &str, &str, u32); 5] = [
    ("vs_main.dxbc", "VSMain", "vs_5_0", 0),
    ("ps_crop.dxbc", "PSCrop", "ps_5_0", 0),
    ("ps_down.dxbc", "PSDown", "ps_5_0", 0),
    ("ps_glass.dxbc", "PSGlass", "ps_5_0", 0),
    ("ps_glass_strict.dxbc", "PSGlass", "ps_5_0", D3DCOMPILE_IEEE_STRICTNESS),
];
const D3DCOMPILE_OPTIMIZATION_LEVEL3: u32 = 1 << 15;
const D3DCOMPILE_IEEE_STRICTNESS: u32 = 1 << 13;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/glass/glass.hlsl");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let src = std::fs::read("src/glass/glass.hlsl").expect("src/glass/glass.hlsl");
    for (file, entry, target, flags) in SHADERS {
        let bytes = match compile(&src, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3 | flags) {
            Ok(b) => b,
            Err(why) => {
                for line in format!("the shader {entry} ({target}) was not compiled at build time, the program will compile it at start: {why}").lines() {
                    println!("cargo:warning={line}");
                }
                Vec::new()
            }
        };
        write(&out.join(file), &bytes);
    }
}

fn write(path: &Path, bytes: &[u8]) {
    // an unchanged file is left alone, so a rebuild that changed nothing does not rebuild the crate
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return;
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

#[cfg(not(windows))]
fn compile(_: &[u8], _: &str, _: &str, _: u32) -> Result<Vec<u8>, String> {
    Err("not a Windows host".into())
}

#[cfg(windows)]
fn compile(src: &[u8], entry: &str, target: &str, flags: u32) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;

    // ID3DBlob: IUnknown's three methods, then GetBufferPointer and GetBufferSize.
    #[repr(C)]
    struct Blob {
        vtbl: *const BlobVtbl,
    }
    #[repr(C)]
    struct BlobVtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut Blob) -> u32,
        buffer_pointer: unsafe extern "system" fn(*mut Blob) -> *mut c_void,
        buffer_size: unsafe extern "system" fn(*mut Blob) -> usize,
    }
    type D3DCompile = unsafe extern "system" fn(
        *const c_void,
        usize,
        *const u8,
        *const c_void,
        *const c_void,
        *const u8,
        *const u8,
        u32,
        u32,
        *mut *mut Blob,
        *mut *mut Blob,
    ) -> i32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

    unsafe fn text(b: *mut Blob) -> Vec<u8> {
        if b.is_null() {
            return Vec::new();
        }
        unsafe {
            let v = &*(*b).vtbl;
            let bytes = std::slice::from_raw_parts((v.buffer_pointer)(b) as *const u8, (v.buffer_size)(b)).to_vec();
            (v.release)(b);
            bytes
        }
    }

    unsafe {
        let dll: Vec<u16> = "d3dcompiler_47.dll".encode_utf16().chain([0]).collect();
        let module = LoadLibraryExW(dll.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if module.is_null() {
            return Err("d3dcompiler_47.dll cannot be loaded".into());
        }
        let f = GetProcAddress(module, c"D3DCompile".as_ptr() as *const u8);
        if f.is_null() {
            return Err("D3DCompile is not in d3dcompiler_47.dll".into());
        }
        let d3d_compile: D3DCompile = std::mem::transmute(f);
        let (name, entry_z, target_z) = (b"glass.hlsl\0".as_slice(), format!("{entry}\0"), format!("{target}\0"));
        let (mut code, mut errors): (*mut Blob, *mut Blob) = (std::ptr::null_mut(), std::ptr::null_mut());
        let hr = d3d_compile(
            src.as_ptr() as *const c_void,
            src.len(),
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            entry_z.as_ptr(),
            target_z.as_ptr(),
            flags,
            0,
            &mut code,
            &mut errors,
        );
        let message = String::from_utf8_lossy(&text(errors)).into_owned();
        if hr < 0 {
            return Err(format!("D3DCompile failed ({hr:#x}): {message}"));
        }
        let bytes = text(code);
        if bytes.is_empty() {
            return Err("D3DCompile returned no bytecode".into());
        }
        Ok(bytes)
    }
}
