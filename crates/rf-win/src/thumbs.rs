//! Shell icons and thumbnails via IShellItemImageFactory, decoded to BGRA pixels.
//! Blocking + COM: call from the worker pool (`ThumbPool`), never the UI thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DIBSECTION, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL, THREAD_MODE_BACKGROUND_BEGIN};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY,
};
use windows::core::PCWSTR;

use crate::shell::ensure_com;
use crate::wide::wide;

/// Straight-alpha BGRA, top-down rows.
#[derive(Clone)]
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Icon,
    /// Real thumbnail if the shell has one; otherwise falls back to the icon.
    Thumbnail,
    /// The file's own picture or nothing (an error): for types that may or may not have one (see `has_provider`), where the caller
    /// has something better to show than this one file's icon.
    ThumbnailOnly,
}

/// Whether the shell has a thumbnail provider for files of this extension (lower case, no dot): one a program registered for the
/// type itself, for its class or for its kind. Read from the registry once for each extension and remembered until the program ends
/// (a provider installed meanwhile is seen after a restart).
///
/// WHY (issue 307): thumbnails were asked for a fixed list of types only (pictures, videos, PDFs and a few more), so a type that a
/// thumbnail extension such as SageThumbs 2K draws (`.pcraft`, `.swf`, a sound file's cover) stayed an icon here while Explorer
/// showed its picture.
pub fn has_provider(ext: &str) -> bool {
    static KNOWN: Mutex<Option<std::collections::HashMap<String, bool>>> = Mutex::new(None);
    if ext.is_empty() || ext.len() > 32 || !ext.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '+' | '~')) {
        return false;
    }
    if let Some(&known) = KNOWN.lock().get_or_insert_with(Default::default).get(ext) {
        return known;
    }
    // (asked outside the lock: two threads may both ask about a new extension, and both get the same answer)
    let found = provider_of(ext).is_some_and(|p| Some(&p) != none_provider().as_ref());
    KNOWN.lock().get_or_insert_with(Default::default).insert(ext.to_string(), found);
    found
}

/// What the shell answers for an extension nobody registered: a provider that stands for every file there is (should one exist)
/// is no sign that this type has a picture of its own.
fn none_provider() -> Option<String> {
    static NONE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NONE.get_or_init(|| provider_of("rustyfiles-no-such-type")).clone()
}

/// The class of the thumbnail provider the shell would use for this extension (IThumbnailProvider, or the older IExtractImage).
fn provider_of(ext: &str) -> Option<String> {
    use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW};
    let dotted = wide(&format!(".{ext}"));
    ["{E357FCCD-A995-4576-B01F-234630154E96}", "{BB2E617C-0920-11D1-9A0B-00C04FC2D6C1}"].iter().find_map(|interface| {
        let interface = wide(interface);
        let mut buf = [0u16; 64];
        let mut len = buf.len() as u32;
        // SAFETY: both strings end in a zero and `len` is the buffer's size in characters
        let hr = unsafe {
            AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, PCWSTR(dotted.as_ptr()), PCWSTR(interface.as_ptr()), Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len)
        };
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (hr == windows::Win32::Foundation::S_OK && end > 0).then(|| String::from_utf16_lossy(&buf[..end]).to_uppercase())
    })
}

/// Starts GDI+ once and never shuts it down.
///
/// The shell starts and stops GDI+ around icon extraction. When the last user stops it,
/// `GdiplusShutdown` holds the GDI+ lock while waiting for its helper thread to exit, and
/// a thread exit needs the loader lock. Some third-party thumbnail handlers (Corel's
/// FileInfoProvider.dll) call `GdiplusStartup` from DllMain, i.e. under the loader lock,
/// and then wait for the GDI+ lock: a deadlock that also blocks every new thread of the
/// process (the folder listing never starts). With our own reference held for the whole
/// process lifetime, the shell's shutdown calls only decrement the count and never wait.
pub fn keep_gdiplus_alive() {
    use windows::Win32::Graphics::GdiPlus::{GdiplusStartup, GdiplusStartupInput, Status};
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        let st = GdiplusStartup(&mut token, &input, std::ptr::null_mut());
        if st != Status(0) {
            log::warn!("GdiplusStartup: {st:?}");
        }
    });
}

/// A design file's thumbnail from the file itself (`crate::design`), not from the shell: the
/// program's own thumbnail provider may be missing, or draw only the page (CorelDRAW's is blank
/// when the design sits beside the page). An Illustrator file that is a PDF and has no saved
/// thumbnail gets its first artboard drawn.
fn design_thumbnail(path: &Path, size: u32) -> Option<Bgra> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !crate::design::is_design_ext(&ext) {
        return None;
    }
    if let Some(img) = crate::design::preview(path) {
        return Some(crate::look::decode::bgra_within(img, size));
    }
    if ext != "ai" {
        return None;
    }
    let mut head = [0u8; 5];
    use std::io::Read;
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head != b"%PDF-" {
        return None;
    }
    // Windows' PDF engine on a thread of its own (it wants a multithreaded apartment)
    let p = path.to_path_buf();
    std::thread::spawn(move || -> Option<Bgra> {
        let pdf = crate::look::pdf::Pdf::open(&p, None).ok()?;
        let (w, h) = pdf.page_size(0).ok()?;
        let width = if h > w { (size as f32 * w / h.max(1.0)).round() as u32 } else { size };
        pdf.render(0, width.max(16)).ok()
    })
    .join()
    .ok()
    .flatten()
}

pub fn get_image(path: &Path, size: u32, kind: Kind) -> anyhow::Result<Bgra> {
    if matches!(kind, Kind::Thumbnail) {
        if let Some(b) = design_thumbnail(path, size) {
            return Ok(b);
        }
        // Photoshop files from the file itself: Windows has no thumbnail provider for them
        if crate::psd_thumb::handles(path)
            && let Some(b) = crate::psd_thumb::thumbnail(path, size)
        {
            return Ok(b);
        }
    }
    // A customised folder's icon straight from the file its desktop.ini names. The shell keeps a folder's desktop.ini in this
    // process: asked again after the icon was changed a second time, it gave the first icon back (issue 65). A file that gives no
    // picture leaves it to the shell, as before.
    if matches!(kind, Kind::Icon)
        && let Some((file, index)) = crate::folder_custom::folder_icon(path)
    {
        let file = std::path::PathBuf::from(crate::folder_custom::expand_env(&file));
        let file = if file.is_relative() { path.join(file) } else { file };
        if let Some(b) = crate::dock::icons::extract_resource(&file, index, size) {
            return Ok(b);
        }
    }
    keep_gdiplus_alive();
    ensure_com();
    let w = wide(path);
    let factory: IShellItemImageFactory = unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None)? };
    let sz = SIZE { cx: size as i32, cy: size as i32 };
    let hbmp = match kind {
        Kind::Icon => unsafe { factory.GetImage(sz, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)? },
        Kind::Thumbnail => unsafe {
            factory
                .GetImage(sz, SIIGBF(SIIGBF_THUMBNAILONLY.0 | SIIGBF_RESIZETOFIT.0 | SIIGBF_BIGGERSIZEOK.0))
                .or_else(|_| factory.GetImage(sz, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK))?
        },
        Kind::ThumbnailOnly => unsafe { factory.GetImage(sz, SIIGBF(SIIGBF_THUMBNAILONLY.0 | SIIGBF_RESIZETOFIT.0 | SIIGBF_BIGGERSIZEOK.0))? },
    };
    let r = hbitmap_to_bgra(hbmp);
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
    }
    // A folder's preview is the folder drawn with its first pictures in it, on a transparent ground. For some folders (a hidden one,
    // some with programs' pictures in them) the shell hands it over with no alpha at all, and it came out as a black square with the
    // folder in it: such a folder gets its icon instead.
    if matches!(kind, Kind::Thumbnail)
        && r.as_ref().is_ok_and(|b| b.data.chunks_exact(4).all(|p| p[3] == 255))
        && std::fs::metadata(path).is_ok_and(|m| m.is_dir())
    {
        let hbmp = unsafe { factory.GetImage(sz, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)? };
        let r = hbitmap_to_bgra(hbmp);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
        }
        return r;
    }
    r
}

/// A picture that came out bigger than its box on the screen, made to fit `size` (its longer side); any other is handed back as it
/// is. Done for the requests that name their box's pixels (`Request::fit`), on the worker's thread.
///
/// The shell may answer with more pixels than asked (`SIIGBF_BIGGERSIZEOK`), and for a folder's preview it always does: 256 x 256,
/// whatever the size. Such a picture went to the screen as it was and the graphics card squeezed it into the tile, from four
/// neighbouring pixels a point and nothing else: jagged at 100 % (256 into 96) and soft at 175 % (256 into 168), "blurred and
/// pixelated" in the report (issue 253). Here, on the worker's thread, every pixel of the result is the weighted mean of all the
/// pixels it stands for, and the tile then draws it one to one.
///
/// The mean is taken with the colours weighted by their alpha: a transparent pixel's colour is black, and it would darken the
/// picture's edge otherwise.
fn fit_to(b: Bgra, size: u32) -> Bgra {
    let longest = b.width.max(b.height);
    if size == 0 || longest <= size || b.data.len() != (b.width as usize) * (b.height as usize) * 4 {
        return b;
    }
    let side = |v: u32| ((v as u64 * size as u64 + longest as u64 / 2) / longest as u64).max(1) as u32;
    let (w, h) = (side(b.width), side(b.height));
    let mut data = b.data;
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 255 {
            p[0] = ((p[0] as u32 * a + 127) / 255) as u8;
            p[1] = ((p[1] as u32 * a + 127) / 255) as u8;
            p[2] = ((p[2] as u32 * a + 127) / 255) as u8;
        }
    }
    let Some(src) = image::RgbaImage::from_raw(b.width, b.height, data) else { return Bgra { width: 0, height: 0, data: Vec::new() } };
    let mut data = image::imageops::resize(&src, w, h, image::imageops::FilterType::CatmullRom).into_raw();
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            p[0] = ((p[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            p[1] = ((p[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            p[2] = ((p[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    Bgra { width: w, height: h, data }
}

/// The file's own picture only (a song's cover art, a video's frame), never its type's icon: `None` when it has none. Blocking.
pub fn picture_only(path: &Path, size: u32) -> Option<Bgra> {
    keep_gdiplus_alive();
    ensure_com();
    let w = wide(path);
    let factory: IShellItemImageFactory = unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.ok()?;
    let sz = SIZE { cx: size as i32, cy: size as i32 };
    let hbmp = unsafe { factory.GetImage(sz, SIIGBF(SIIGBF_THUMBNAILONLY.0 | SIIGBF_RESIZETOFIT.0 | SIIGBF_BIGGERSIZEOK.0)) }.ok()?;
    let r = hbitmap_to_bgra(hbmp).ok();
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
    }
    r
}

fn hbitmap_to_bgra(hbmp: HBITMAP) -> anyhow::Result<Bgra> {
    unsafe {
        let mut ds = DIBSECTION::default();
        let n = GetObjectW(HGDIOBJ(hbmp.0), std::mem::size_of::<DIBSECTION>() as i32, Some(&mut ds as *mut _ as *mut _));
        anyhow::ensure!(n as usize >= std::mem::size_of::<BITMAP>(), "GetObject failed");
        let bm = ds.dsBm;
        anyhow::ensure!(!bm.bmBits.is_null() && bm.bmBitsPixel == 32, "unsupported bitmap");
        let w = bm.bmWidth as u32;
        let h = bm.bmHeight.unsigned_abs();
        // Let GDI hand the rows over top-down: the DIB's own row order can't be trusted (for
        // EXIF-rotated photos the shell returns a top-down section that GetObject reports as
        // bottom-up, which turned those thumbnails upside down).
        let mut data = vec![0u8; (w * h * 4) as usize];
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbmp, 0, h, Some(data.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        anyhow::ensure!(lines as u32 == h, "GetDIBits failed");
        let any_alpha = data.chunks_exact(4).any(|p| p[3] != 0);
        if !any_alpha {
            // Opaque thumbnails often come with alpha = 0.
            for p in data.chunks_exact_mut(4) {
                p[3] = 255;
            }
        } else {
            // Shell bitmaps are premultiplied; GPUI wants straight alpha.
            for p in data.chunks_exact_mut(4) {
                let a = p[3] as u32;
                if a != 0 && a != 255 {
                    p[0] = ((p[0] as u32 * 255 + a / 2) / a).min(255) as u8;
                    p[1] = ((p[1] as u32 * 255 + a / 2) / a).min(255) as u8;
                    p[2] = ((p[2] as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Ok(Bgra { width: w, height: h, data })
    }
}

/// Whether a folder with these attributes can have an icon of its own. The shell looks into a folder's `desktop.ini` (an icon the
/// user chose under Properties > Customize, the pictures of Downloads, Documents and the like) only when the folder is read-only or
/// a system folder: that is how Windows marks a customised folder. Such a folder is asked for its own icon; every other folder shares
/// one picture. (A user on Discord, 2026-11-14: "there is an option to make folder icons ... in this new app the folder icons are
/// gone": all folders shared the one picture.)
pub fn folder_has_own_icon(attrs: u32) -> bool {
    const READONLY: u32 = 0x1;
    const SYSTEM: u32 = 0x4;
    attrs & (READONLY | SYSTEM) != 0
}

/// Extensions whose icon differs per file (must not be cached by extension).
pub fn has_unique_icon(ext: &str) -> bool {
    matches!(ext, "exe" | "lnk" | "ico" | "url" | "cur" | "ani" | "scr" | "cpl" | "msc" | "appref-ms" | "website")
}

pub struct Request {
    pub key: u64,
    pub path: PathBuf,
    pub size: u32,
    pub kind: Kind,
    /// The picture the user is looking at right now (the preview pane's): see `ThumbPool` lanes.
    pub urgent: bool,
    /// The pixels the picture's box has on the screen, when the asker knows them: a picture that comes out bigger is made to fit
    /// them (`fit_to`). `None` leaves the picture as Windows gave it, which is what every asker that gives its size in the
    /// interface's own pixels counts on (on a scaled display the bigger picture is what keeps it sharp there).
    pub fit: Option<u32>,
    /// When it was handed to the pool (for `Response::waited`).
    queued: std::time::Instant,
}

impl Request {
    pub fn new(key: u64, path: PathBuf, size: u32, kind: Kind) -> Self {
        Self { key, path, size, kind, urgent: false, fit: None, queued: std::time::Instant::now() }
    }
}

pub struct Response {
    pub key: u64,
    pub image: Option<Arc<Bgra>>,
    /// Request was discarded (backlog) — not a failure; may be retried.
    pub dropped: bool,
    /// How long the request stood in the queue, and how long making the picture took (zero when dropped).
    pub waited: std::time::Duration,
    pub took: std::time::Duration,
    /// Made by the lane of urgent requests.
    pub urgent: bool,
}

/// How many urgent requests may stand waiting. The preview pane asks for one picture and Quick Look (over it) for
/// another, so two are live at the same time; more than that is arrow-keying through files, and only the newest matter.
const URGENT_KEEP: usize = 2;

/// How many of the `waiting` urgent requests must go (the oldest) to make room for one more.
fn urgent_overflow(waiting: usize) -> usize {
    (waiting + 1).saturating_sub(URGENT_KEEP)
}

/// How many threads serve the lane. The pane's picture and Quick Look's are live together, and one file whose thumbnail
/// handler hangs (a share that does not answer) must not keep the other from being made.
const LANE_THREADS: usize = 2;

/// The picture of an urgent request. An ordinary picture file is decoded in this process (`pane_picture`) and not by the
/// shell: with every core of a 16-thread machine busy, the shell's way of making a 1920 x 1080 JPEG's thumbnail took 63 ms
/// to 2.5 s on a warm thread (up to 30 s on a lane thread while the list's workers made their tiles through the shell
/// at the same moment) and WIC's 12 ms; on an idle machine 23 to 75 ms and 9 ms. Everything else, and a file WIC cannot
/// read, is the shell's.
fn urgent_image(path: &Path, size: u32, kind: Kind) -> Option<Bgra> {
    if matches!(kind, Kind::Thumbnail) && crate::pane_picture::handles(path) {
        if let Ok(b) = crate::pane_picture::decode(path, size) {
            return Some(b);
        }
    }
    get_image(path, size, kind).ok()
}

/// The lane for the picture the user is looking at (see `ThumbPool`): its own queue and a thread or two.
struct Lane {
    queue: Arc<Mutex<Vec<Request>>>,
    wake: Sender<()>,
}

impl Lane {
    fn spawn(on_done: Arc<dyn Fn(Response) + Send + Sync>) -> Self {
        let queue: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
        let (wake, rx): (Sender<()>, Receiver<()>) = crossbeam_channel::unbounded();
        for i in 0..LANE_THREADS {
            let (q, rx, on_done) = (queue.clone(), rx.clone(), on_done.clone());
            std::thread::Builder::new()
                .name(format!("rf-thumb-urgent-{i}"))
                .spawn(move || {
                    // Normal priority on purpose, and not background mode: the workers below run at the lowest CPU and I/O
                    // priority so that a folder of thousands of tiles never slows the user down; the picture the user waits
                    // for is the user's work.
                    ensure_com();
                    while rx.recv().is_ok() {
                        loop {
                            let req = q.lock().pop();
                            let Some(req) = req else { break };
                            let started = std::time::Instant::now();
                            let image = urgent_image(&req.path, req.size, req.kind).map(Arc::new);
                            on_done(Response { key: req.key, image, dropped: false, waited: started - req.queued, took: started.elapsed(), urgent: true });
                        }
                    }
                })
                .ok();
        }
        Self { queue, wake }
    }
}

/// Worker pool for icon/thumbnail extraction. Newest requests are served first (they belong to what the user is
/// looking at right now). The workers run in background mode (lowest CPU and I/O priority), all alike, so one slow
/// file (a video, a big PDF) can hold a worker for seconds. A request marked `urgent` goes to a lane of its own
/// instead: a couple of normal-priority threads that are started by the first such request, only a couple of requests deep.
pub struct ThumbPool {
    stack: Arc<Mutex<Vec<Request>>>,
    wake: Sender<()>,
    on_done: Arc<dyn Fn(Response) + Send + Sync>,
    lane: std::sync::OnceLock<Lane>,
}

impl ThumbPool {
    pub fn new(workers: usize, on_done: impl Fn(Response) + Send + Sync + 'static) -> Self {
        let stack: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
        let (wake, rx): (Sender<()>, Receiver<()>) = crossbeam_channel::unbounded();
        let on_done: Arc<dyn Fn(Response) + Send + Sync> = Arc::new(on_done);
        keep_gdiplus_alive();
        for i in 0..workers {
            let stack = stack.clone();
            let rx = rx.clone();
            let on_done = on_done.clone();
            std::thread::Builder::new()
                .name(format!("rf-thumb-{i}"))
                .spawn(move || {
                    unsafe {
                        // Background mode lowers CPU *and* I/O priority: never compete with the user's work.
                        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
                        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                    }
                    ensure_com();
                    while rx.recv().is_ok() {
                        loop {
                            let req = stack.lock().pop();
                            let Some(req) = req else { break };
                            let started = std::time::Instant::now();
                            let image = get_image(&req.path, req.size, req.kind).ok().map(|b| match req.fit {
                                Some(px) => fit_to(b, px),
                                None => b,
                            }).map(Arc::new);
                            on_done(Response { key: req.key, image, dropped: false, waited: started - req.queued, took: started.elapsed(), urgent: false });
                        }
                    }
                })
                .ok();
        }
        Self { stack, wake, on_done, lane: std::sync::OnceLock::new() }
    }

    pub fn request(&self, req: Request) {
        if req.urgent {
            return self.request_urgent(req);
        }
        let mut s = self.stack.lock();
        // Bound the backlog: drop the oldest (scrolled-away) requests.
        let dropped: Vec<Request> = if s.len() > 512 { s.drain(0..256).collect() } else { Vec::new() };
        s.push(req);
        drop(s);
        // Tell the cache so those slots don't stay "pending" forever (they will be re-requested).
        for d in dropped {
            (self.on_done)(Response { key: d.key, image: None, dropped: true, waited: d.queued.elapsed(), took: std::time::Duration::ZERO, urgent: d.urgent });
        }
        let _ = self.wake.send(());
    }

    fn request_urgent(&self, req: Request) {
        let lane = self.lane.get_or_init(|| Lane::spawn(self.on_done.clone()));
        let mut q = lane.queue.lock();
        // Arrow keys through many files ask for one picture per file: only the newest are kept, a decode per file is never queued.
        let over = urgent_overflow(q.len());
        let dropped: Vec<Request> = q.drain(0..over).collect();
        q.push(req);
        drop(q);
        for d in dropped {
            (self.on_done)(Response { key: d.key, image: None, dropped: true, waited: d.queued.elapsed(), took: std::time::Duration::ZERO, urgent: true });
        }
        let _ = lane.wake.send(());
    }

    /// Drops queued work (e.g. on navigation).
    pub fn clear(&self) {
        self.stack.lock().clear();
        if let Some(l) = self.lane.get() {
            l.queue.lock().clear();
        }
    }
}

#[cfg(test)]
mod tests {
    /// A picture bigger than asked for is made to fit, with its colours kept right up to a transparent edge (issue 253).
    #[test]
    fn a_picture_bigger_than_asked_is_made_to_fit() {
        use super::{Bgra, fit_to};
        // a yellow square with a transparent frame around it, as a folder's 256 px preview is
        let (w, h) = (256u32, 256u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        // (its edges between two of the smaller picture's pixels, so that those come out part see-through)
        for y in 37..219u32 {
            for x in 37..219u32 {
                let i = ((y * w + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[40, 200, 250, 255]);
            }
        }
        let b = fit_to(Bgra { width: w, height: h, data }, 168);
        assert_eq!((b.width, b.height, b.data.len()), (168, 168, 168 * 168 * 4));
        let at = |x: u32, y: u32| -> [u8; 4] { b.data[((y * 168 + x) * 4) as usize..][..4].try_into().unwrap() };
        assert_eq!(at(84, 84), [40, 200, 250, 255], "the middle keeps its colour");
        assert_eq!(at(2, 2)[3], 0, "the frame stays transparent");
        // on the edge of the square the pixels are part see-through, and their colour is still the square's, not darkened by
        // the transparent ground's black
        let edge = (0..168).map(|x| at(x, 84)).find(|p| p[3] > 20 && p[3] < 235).expect("a part see-through pixel on the edge");
        assert!(edge[0].abs_diff(40) <= 12 && edge[1].abs_diff(200) <= 12 && edge[2].abs_diff(250) <= 12, "{edge:?}");
        // the longer side decides, and a picture that fits already is left as it is
        let wide = fit_to(Bgra { width: 200, height: 100, data: vec![255; 200 * 100 * 4] }, 100);
        assert_eq!((wide.width, wide.height), (100, 50));
        let small = fit_to(Bgra { width: 48, height: 48, data: vec![7; 48 * 48 * 4] }, 168);
        assert_eq!((small.width, small.height, small.data[0]), (48, 48, 7));
    }

    #[test]
    fn a_type_has_a_thumbnail_provider_only_when_one_is_registered_for_it() {
        // (Windows registers one for its own picture types; nothing does for a type nobody knows, a name that is no extension, or none)
        assert!(super::has_provider("jpg"), "{:?}", super::provider_of("jpg"));
        assert!(!super::has_provider("rustyfiles-another-unknown-type"));
        assert!(!super::has_provider(""));
        assert!(!super::has_provider(r"a\b"));
        // what this machine says about the types the list never asked for (printed for a look: `-- --nocapture`)
        for ext in ["mp3", "m4a", "opus", "flac", "wav", "ttf", "otf", "txt", "exe", "lnk", "url", "zip", "msi", "html", "svg", "docx", "swf", "pcraft"] {
            println!("{ext}: {:?}", super::provider_of(ext).filter(|p| Some(p) != super::none_provider().as_ref()));
        }
    }

    use super::*;

    /// A JPEG with a red top-left corner, turned clockwise via its EXIF tag: the shell
    /// thumbnail must show the red corner at the top right (it came out upside down once).
    #[test]
    fn exif_rotated_thumbnail_is_upright() {
        use windows::Win32::Foundation::GENERIC_WRITE;
        use windows::Win32::Graphics::Imaging::*;
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
        ensure_com();
        let dir = std::env::temp_dir().join(format!("rf-thumb-exif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("r.jpg");
        let (w, h) = (64u32, 40u32);
        let mut px = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                let red = x < 16 && y < 16;
                px[i..i + 3].copy_from_slice(if red { &[0, 0, 255] } else { &[200, 120, 40] });
            }
        }
        unsafe {
            let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
            let bmp = f.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat24bppBGR, w * 3, &px).unwrap();
            let stream = f.CreateStream().unwrap();
            let pw = wide(&p);
            stream.InitializeFromFilename(PCWSTR(pw.as_ptr()), GENERIC_WRITE.0).unwrap();
            let enc = f.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null()).unwrap();
            enc.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
            let (mut fe, mut props) = (None, None);
            enc.CreateNewFrame(&mut fe, &mut props).unwrap();
            let fe = fe.unwrap();
            fe.Initialize(props.as_ref()).unwrap();
            fe.SetSize(w, h).unwrap();
            let mut fmt = GUID_WICPixelFormat24bppBGR;
            fe.SetPixelFormat(&mut fmt).unwrap();
            fe.WriteSource(&bmp, std::ptr::null()).unwrap();
            fe.Commit().unwrap();
            enc.Commit().unwrap();
        }
        crate::rotate::rotate(&p, true).unwrap();
        let img = get_image(&p, 64, Kind::Thumbnail).unwrap();
        assert!(img.height > img.width, "portrait after the turn: {}x{}", img.width, img.height);
        let at = |x: u32, y: u32| {
            let i = ((y * img.width + x) * 4) as usize;
            (img.data[i + 2], img.data[i + 1], img.data[i])
        };
        let (r, g, b) = at(img.width - 3, 3);
        assert!(r > 180 && g < 90 && b < 90, "top-right should be red, got {:?}", (r, g, b));
        let (r, _, _) = at(img.width - 3, img.height - 3);
        assert!(r < 120, "bottom-right should not be red");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a test pool's `on_done` saw: the answer and the priority of the thread that gave it.
    struct Seen {
        key: u64,
        dropped: bool,
        urgent: bool,
        priority: i32,
    }

    /// A pool whose answers for keys below 100 (the ones that were made, not dropped) stop the thread that gives them
    /// until `open` is called: a lane or a worker that is busy for a long time. `entered` hears when one has stopped.
    struct GatedPool {
        pool: ThumbPool,
        seen: Receiver<Seen>,
        entered: Receiver<u64>,
        gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    impl GatedPool {
        fn new(workers: usize) -> Self {
            let (tx, seen) = crossbeam_channel::unbounded();
            let (etx, entered) = crossbeam_channel::unbounded();
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let g = gate.clone();
            let pool = ThumbPool::new(workers, move |r| {
                let priority = unsafe { windows::Win32::System::Threading::GetThreadPriority(GetCurrentThread()) };
                if r.key < 100 && !r.dropped {
                    let _ = etx.send(r.key);
                    let (m, cv) = &*g;
                    let _guard = cv.wait_while(m.lock().unwrap(), |open| !*open).unwrap();
                }
                let _ = tx.send(Seen { key: r.key, dropped: r.dropped, urgent: r.urgent, priority });
            });
            Self { pool, seen, entered, gate }
        }

        fn open(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }

        fn ask(&self, key: u64, urgent: bool) {
            let mut r = Request::new(key, std::env::temp_dir().join("rf-thumb-no-such-file.jpg"), 64, Kind::Thumbnail);
            r.urgent = urgent;
            self.pool.request(r);
        }
    }

    const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

    /// Arrow keys through many files must not queue a decode per file: while both lane threads are busy, a burst of
    /// urgent requests leaves exactly `URGENT_KEEP` waiting (the newest) and answers every older one as dropped.
    #[test]
    fn a_burst_of_urgent_requests_keeps_only_the_newest() {
        let t = GatedPool::new(1);
        for key in 1..=LANE_THREADS as u64 {
            t.ask(key, true);
            assert_eq!(t.entered.recv_timeout(WAIT).unwrap(), key, "the lane thread took it");
        }
        // (the lane is now full of work: nothing below can start)
        for key in 100..110u64 {
            t.ask(key, true);
        }
        let mut dropped = Vec::new();
        for _ in 0..(10 - URGENT_KEEP) {
            let s = t.seen.recv_timeout(WAIT).unwrap();
            assert!(s.dropped && s.urgent, "an older request was answered as dropped");
            dropped.push(s.key);
        }
        dropped.sort_unstable();
        assert_eq!(dropped, (100..110 - URGENT_KEEP as u64).collect::<Vec<_>>(), "the oldest ones are the dropped");
        assert!(t.seen.try_recv().is_err(), "nothing else was answered while the lane was busy");
        t.open();
        let mut made = Vec::new();
        while made.len() < LANE_THREADS + URGENT_KEEP {
            let s = t.seen.recv_timeout(WAIT).unwrap();
            assert!(!s.dropped && s.urgent);
            made.push(s.key);
        }
        made.sort_unstable();
        let mut want: Vec<u64> = (1..=LANE_THREADS as u64).chain(110 - URGENT_KEEP as u64..110).collect();
        want.sort_unstable();
        assert_eq!(made, want, "the two that were being made and the newest waiting ones");
    }

    /// The picture the user looks at is not behind the list's tiles: with every worker stopped on one, an urgent
    /// request is still answered, by a thread at normal priority (a worker's is lower: it runs in background mode).
    #[test]
    fn an_urgent_request_does_not_wait_for_busy_workers() {
        let t = GatedPool::new(2);
        t.ask(1, false);
        t.ask(2, false);
        let mut stopped = [t.entered.recv_timeout(WAIT).unwrap(), t.entered.recv_timeout(WAIT).unwrap()];
        stopped.sort_unstable();
        assert_eq!(stopped, [1, 2], "both workers are stopped on a tile");
        t.ask(150, true);
        let s = t.seen.recv_timeout(WAIT).expect("the urgent request is answered while the workers are busy");
        assert_eq!((s.key, s.dropped, s.urgent), (150, false, true));
        assert_eq!(s.priority, 0, "the lane runs at normal priority");
        t.open();
        let mut workers = Vec::new();
        for _ in 0..2 {
            workers.push(t.seen.recv_timeout(WAIT).unwrap());
        }
        assert!(workers.iter().all(|w| !w.urgent && w.priority < 0), "the workers run below normal priority");
    }

    #[test]
    fn only_a_read_only_or_system_folder_can_have_an_icon_of_its_own() {
        const DIR: u32 = 0x10;
        assert!(!folder_has_own_icon(DIR));
        assert!(!folder_has_own_icon(DIR | 0x2), "hidden");
        assert!(!folder_has_own_icon(DIR | 0x20 | 0x2000), "archive, not indexed");
        assert!(folder_has_own_icon(DIR | 0x1));
        assert!(folder_has_own_icon(DIR | 0x4));
        assert!(folder_has_own_icon(DIR | 0x1 | 0x4 | 0x2));
    }

    /// On the system the tests run on: a folder given an icon of its own (as Properties > Customize does) is marked the way
    /// `folder_has_own_icon` looks for, and the shell gives it a picture that is not the plain folder's.
    #[test]
    fn a_folder_with_its_own_icon_gets_its_own_picture() {
        let base = std::env::temp_dir().join(format!("rf-foldericon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (plain, own) = (base.join("plain"), base.join("own"));
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(&own).unwrap();
        // (shell32's icon 15 is the computer: nothing like a folder)
        crate::folder_custom::set_folder_icon(&own, Some((r"C:\Windows\System32\shell32.dll", 15))).unwrap();
        let attrs_of = |p: &Path| unsafe { windows::Win32::Storage::FileSystem::GetFileAttributesW(windows::core::PCWSTR(crate::wide::wide(p).as_ptr())) };
        let (a_plain, a_own) = (attrs_of(&plain), attrs_of(&own));
        let (i_plain, i_own) = (get_image(&plain, 32, Kind::Icon).unwrap(), get_image(&own, 32, Kind::Icon).unwrap());
        // (the folder is read-only now: take the mark off so that the scratch folder can be removed)
        let _ = crate::folder_custom::set_folder_icon(&own, None);
        let _ = std::fs::remove_dir_all(&base);
        assert!(!folder_has_own_icon(a_plain) && folder_has_own_icon(a_own), "plain {a_plain:#x}, customised {a_own:#x}");
        assert!(i_plain.data != i_own.data, "the customised folder's picture is the plain folder's");
    }

    #[test]
    fn icon_for_exe_and_folder() {
        let exe = PathBuf::from(r"C:\Windows\explorer.exe");
        let img = get_image(&exe, 32, Kind::Icon).unwrap();
        assert!(img.width >= 32 && img.height >= 32);
        assert!(img.data.chunks_exact(4).any(|p| p[3] > 0));
        let dir = get_image(Path::new(r"C:\Windows"), 32, Kind::Icon).unwrap();
        assert!(dir.width > 0);
    }
}
