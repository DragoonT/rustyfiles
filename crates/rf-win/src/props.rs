//! Media/document properties from the Windows property system (duration, resolution, frame
//! rate, bitrate, camera…), formatted by Windows itself (localized). Blocking: off the UI thread.

use std::path::Path;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::PropertiesSystem::{
    GPS_BESTEFFORT, GPS_OPENSLOWITEM, IPropertyStore, PDFF_DEFAULT, PSFormatForDisplayAlloc,
    SHGetPropertyStoreFromParsingName,
};
use windows::core::PCWSTR;

use crate::shell::ensure_com;
use crate::wide::wide;

/// (label, key) pairs shown in the preview pane, in display order.
fn wanted() -> Vec<(&'static str, PROPERTYKEY)> {
    vec![
        ("Süre", PKEY_Media_Duration),
        ("Çözünürlük", PKEY_Video_FrameWidth), // merged with height below
        ("Kare hızı", PKEY_Video_FrameRate),
        ("Video bit hızı", PKEY_Video_EncodingBitrate),
        ("Toplam bit hızı", PKEY_Video_TotalBitrate),
        ("Ses bit hızı", PKEY_Audio_EncodingBitrate),
        ("Örnekleme", PKEY_Audio_SampleRate),
        ("Kanal", PKEY_Audio_ChannelCount),
        ("Boyutlar", PKEY_Image_Dimensions),
        ("Bit derinliği", PKEY_Image_BitDepth),
        ("Kamera", PKEY_Photo_CameraModel),
        ("Çekim tarihi", PKEY_Photo_DateTaken),
        ("Sanatçı", PKEY_Music_Artist),
        ("Albüm", PKEY_Music_AlbumTitle),
        ("Başlık", PKEY_Title),
        ("Sayfa", PKEY_Document_PageCount),
        ("Yazar", PKEY_Author),
    ]
}

fn display(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let pv = store.GetValue(key).ok()?;
        let s = PSFormatForDisplayAlloc(key, &pv, PDFF_DEFAULT).ok()?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as *const _));
        out.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }
}

/// Returns non-empty properties as (label, value).
pub fn media_props(path: &Path) -> Vec<(&'static str, String)> {
    ensure_com();
    let w = wide(path);
    // BESTEFFORT: don't force slow handlers to fully parse; never hydrates cloud files by itself
    let store: IPropertyStore =
        match unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(w.as_ptr()), None, GPS_BESTEFFORT | GPS_OPENSLOWITEM) } {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
    let mut out = Vec::new();
    for (label, key) in wanted() {
        if key == PKEY_Video_FrameWidth {
            if let (Some(wd), Some(h)) = (display(&store, &PKEY_Video_FrameWidth), display(&store, &PKEY_Video_FrameHeight)) {
                out.push((label, format!("{wd} × {h}")));
            }
            continue;
        }
        if let Some(v) = display(&store, &key) {
            out.push((label, v));
        }
    }
    out
}

/// The descriptive fields a user may write (Explorer's Details tab: Description).
/// `tags` and `authors` are lists, written "a; b".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editable {
    pub title: String,
    pub subject: String,
    pub tags: String,
    pub comment: String,
    pub authors: String,
}

fn editable_keys() -> [PROPERTYKEY; 5] {
    [PKEY_Title, PKEY_Subject, PKEY_Keywords, PKEY_Comment, PKEY_Author]
}

/// The fields as they are now; None when the file's type has no properties that can be
/// written (most types: only those with a property handler of their own, such as JPEG, MP3,
/// MP4, Office documents) or the file can't be opened for writing.
pub fn editable(path: &Path) -> Option<Editable> {
    use windows::Win32::UI::Shell::PropertiesSystem::{GPS_READWRITE, IPropertyStoreCapabilities};
    use windows::core::Interface;
    ensure_com();
    let w = wide(path);
    let store: IPropertyStore = unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(w.as_ptr()), None, GPS_READWRITE) }.ok()?;
    let caps: IPropertyStoreCapabilities = store.cast().ok()?;
    // (S_OK = writable; S_FALSE and errors are not)
    let writable = |k: &PROPERTYKEY| unsafe { caps.IsPropertyWritable(k) }.is_ok();
    let keys = editable_keys();
    if !keys.iter().any(writable) {
        return None;
    }
    let get = |k: &PROPERTYKEY| display(&store, k).unwrap_or_default();
    Some(Editable { title: get(&keys[0]), subject: get(&keys[1]), tags: get(&keys[2]), comment: get(&keys[3]), authors: get(&keys[4]) })
}

/// Writes the fields of `new` that differ from `old` into the file. An empty text removes the
/// property.
pub fn set_editable(path: &Path, old: &Editable, new: &Editable) -> anyhow::Result<()> {
    use windows::Win32::System::Com::StructuredStorage::{InitPropVariantFromStringAsVector, PROPVARIANT};
    use windows::Win32::UI::Shell::PropertiesSystem::{GPS_READWRITE, PSCoerceToCanonicalValue};
    ensure_com();
    let w = wide(path);
    let store: IPropertyStore = unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(w.as_ptr()), None, GPS_READWRITE) }?;
    let pairs = [(&old.title, &new.title), (&old.subject, &new.subject), (&old.tags, &new.tags), (&old.comment, &new.comment), (&old.authors, &new.authors)];
    let mut changed = false;
    for (key, (was, now)) in editable_keys().iter().zip(pairs) {
        if was == now {
            continue;
        }
        let text = now.trim();
        let mut pv = if text.is_empty() {
            PROPVARIANT::default()
        } else {
            // "a; b" as a list of strings; made the property's own type below (a single string
            // for a title, a list for tags)
            let wt = wide(text);
            unsafe { InitPropVariantFromStringAsVector(PCWSTR(wt.as_ptr())) }?
        };
        if !text.is_empty() {
            unsafe { PSCoerceToCanonicalValue(key, &mut pv) }?;
        }
        unsafe { store.SetValue(key, &pv) }?;
        changed = true;
    }
    if changed {
        unsafe { store.Commit() }?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// A JPEG's title and tags are written and read back; a text file has nothing to write.
    #[test]
    fn descriptive_fields_are_written_into_the_file() {
        use super::{Editable, editable, set_editable};
        let dir = std::env::temp_dir().join(format!("rf-props-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("a.jpg");
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 80, 20])).save(&jpg).unwrap();
        let was = editable(&jpg).expect("a JPEG has writable properties");
        assert_eq!(was, Editable::default());
        let new = Editable { title: "Gün batımı".into(), tags: "tatil; deniz".into(), comment: "ilk deneme".into(), ..Default::default() };
        set_editable(&jpg, &was, &new).unwrap();
        let now = editable(&jpg).unwrap();
        assert_eq!((now.title.as_str(), now.tags.as_str(), now.comment.as_str()), ("Gün batımı", "tatil; deniz", "ilk deneme"));
        // an emptied field is removed; the others stay
        set_editable(&jpg, &now, &Editable { title: String::new(), ..now.clone() }).unwrap();
        let last = editable(&jpg).unwrap();
        assert_eq!((last.title.as_str(), last.tags.as_str()), ("", "tatil; deniz"));
        // still a picture
        assert_eq!(image::image_dimensions(&jpg).unwrap(), (8, 8));
        let txt = dir.join("a.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert_eq!(editable(&txt), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_dimensions() {
        let jpg = std::fs::read_dir(r"C:\Windows\Web\Wallpaper\Windows")
            .ok()
            .and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jpg"))));
        if let Some(p) = jpg {
            let props = super::media_props(&p);
            assert!(props.iter().any(|(l, _)| *l == "Boyutlar"), "{props:?}");
        }
    }
}
