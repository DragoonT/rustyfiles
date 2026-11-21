//! Settings > Eklentiler: the add-ons rustyfiles can install (`addons.rs`), each with its version, licence and source, and Install,
//! Update and Remove. The list is read when the page is first shown (and with "Yeniden dene" after a failure); an install runs on a
//! thread of its own and says how far it is.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::addons::{self, AddonInfo, Manifest};
use crate::settings_view::{button, card, section};
use crate::t;
use crate::theme::Palette;

/// An install or update under way: the bytes so far and in all, and its cancel switch.
struct Busy {
    done: Arc<AtomicU64>,
    total: u64,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    manifest: Option<Result<Manifest, String>>,
    loading: bool,
    busy: HashMap<String, Busy>,
    /// The last thing that went wrong with an add-on (shown under it until the next try).
    errors: HashMap<String, String>,
}

impl Global for State {}

fn state(cx: &mut App) -> &mut State {
    if !cx.has_global::<State>() {
        cx.set_global(State::default());
    }
    cx.global_mut::<State>()
}

fn load(cx: &mut App) {
    let st = state(cx);
    if st.loading {
        return;
    }
    st.loading = true;
    cx.spawn(async move |cx| {
        let got = cx.background_spawn(async { addons::fetch_manifest() }).await;
        cx.update(|cx| {
            let st = cx.global_mut::<State>();
            st.loading = false;
            st.manifest = Some(got);
            cx.refresh_windows();
        });
    })
    .detach();
}

/// What the add-on is for, in the user's language (the list's own words are English).
fn description(id: &str) -> Option<&'static str> {
    match id {
        "hyperdownload" => Some(t!("İndirme yöneticisi: bir dosyayı birçok bağlantıyla birden indirir, duraklatılıp kaldığı yerden sürdürülür; torrent ve magnet bağlantılarını da indirir")),
        _ => None,
    }
}

fn install(info: AddonInfo, cx: &mut App) {
    let id = info.id.clone();
    let done = Arc::new(AtomicU64::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let total = info.file().map_or(0, |f| f.size);
    let st = state(cx);
    st.errors.remove(&id);
    st.busy.insert(id.clone(), Busy { done: done.clone(), total, cancel: cancel.clone() });
    // the page shows the progress: it is drawn again now and then while the install runs
    cx.spawn({
        let id = id.clone();
        async move |cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                let going = cx.update(|cx| {
                    cx.refresh_windows();
                    cx.try_global::<State>().is_some_and(|s| s.busy.contains_key(&id))
                });
                if !going {
                    break;
                }
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let (d, c) = (done.clone(), cancel.clone());
        let hd = info.id == crate::hyperdownload::ID;
        let got = cx
            .background_spawn(async move {
                let r = addons::install(&info, &|n, _| d.store(n, Ordering::Relaxed), &|| c.load(Ordering::Relaxed)).map(|_| ());
                // HyperDownload: the browsers learn where its host is
                if r.is_ok() && hd {
                    crate::hyperdownload::register_host();
                }
                r
            })
            .await;
        cx.update(|cx| {
            let st = cx.global_mut::<State>();
            st.busy.remove(&id);
            if let Err(e) = got {
                st.errors.insert(id, e);
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

fn remove(id: String, cx: &mut App) {
    if id == crate::hyperdownload::ID {
        crate::hyperdownload::unregister_host();
    }
    let got = addons::remove(&id);
    let st = state(cx);
    match got {
        Ok(()) => {
            st.errors.remove(&id);
        }
        Err(e) => {
            st.errors.insert(id, format!("{}: {e}", t!("Kaldırılamadı (program açık olabilir)")));
        }
    }
    cx.refresh_windows();
}

fn open_url(url: String) {
    std::thread::spawn(move || {
        let _ = rf_win::shell::open(std::path::Path::new(&url));
    });
}

pub fn page(p: &Palette, cx: &mut App) -> Vec<AnyElement> {
    if state(cx).manifest.is_none() {
        load(cx);
    }
    let mut out: Vec<AnyElement> = vec![
        div()
            .text_sm()
            .text_color(p.text_secondary)
            .child(t!("Eklentiler rustyfiles'a ayrı programlar olarak eklenir: yalnızca siz kurunca indirilir, rustyfiles'ın listesindeki parmak iziyle (SHA-256) denetlenir ve istediğiniz zaman kaldırılır."))
            .into_any_element(),
    ];
    let st = state(cx);
    let manifest = match &st.manifest {
        None => {
            out.push(div().pt_3().text_sm().text_color(p.text_secondary).child(t!("Eklenti listesi okunuyor…")).into_any_element());
            return out;
        }
        Some(Err(e)) => {
            let e = e.clone();
            out.push(card(p, L::CircleAlert, t!("Eklenti listesi okunamadı"), Some(&e), button("addons-retry", p, t!("Yeniden dene"), |_, cx| {
                state(cx).manifest = None;
                load(cx);
            })));
            return out;
        }
        Some(Ok(m)) => m.clone(),
    };
    out.push(section(p, t!("Kullanılabilir")));
    if manifest.addons.is_empty() {
        out.push(div().text_sm().text_color(p.text_secondary).child(t!("Şu an kurulabilecek bir eklenti yok.")).into_any_element());
    }
    for info in manifest.addons {
        let id = info.id.clone();
        let have = addons::installed(&id);
        let busy = state(cx).busy.get(&id).map(|b| (b.done.load(Ordering::Relaxed), b.total, b.cancel.clone()));
        let error = state(cx).errors.get(&id).cloned();
        let available = info.file().is_some();
        let mut facts = Vec::new();
        match &have {
            Some(h) => facts.push(format!("{} {}", t!("Kurulu:"), h.version)),
            None => facts.push(format!("{} {}", t!("Sürüm:"), info.version)),
        }
        if let Some(h) = &have
            && addons::newer(&info.version, &h.version)
        {
            facts.push(format!("{} {}", t!("Yeni sürüm:"), info.version));
        }
        facts.push(info.license.clone());
        if let Some(f) = info.file().filter(|f| f.size > 0) {
            facts.push(format!("{:.1} MB", f.size as f64 / 1e6));
        }
        let mut desc = description(&id).map(str::to_string).unwrap_or_default();
        if !desc.is_empty() {
            desc.push('\n');
        }
        desc.push_str(&facts.join(" · "));
        if let Some(e) = &error {
            desc.push('\n');
            desc.push_str(e);
        }
        if !available {
            desc.push('\n');
            desc.push_str(t!("Bu işlemci için henüz bir sürümü yok."));
        }
        let control: AnyElement = match (busy, &have) {
            (Some((done, total, cancel)), _) => {
                let text = if total > 0 { format!("{} {}%", t!("İndiriliyor"), (done * 100 / total.max(1)).min(100)) } else { format!("{} {:.1} MB", t!("İndiriliyor"), done as f64 / 1e6) };
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().text_color(p.text_secondary).child(text))
                    .child(button(SharedString::from(format!("addon-cancel-{id}")), p, t!("İptal"), move |_, _| cancel.store(true, Ordering::Relaxed)))
                    .into_any_element()
            }
            (None, Some(h)) => {
                let mut row = h_flex().gap_2();
                if available && addons::newer(&info.version, &h.version) {
                    let i = info.clone();
                    row = row.child(button(SharedString::from(format!("addon-update-{id}")), p, t!("Güncelle"), move |_, cx| install(i.clone(), cx)));
                }
                let rid = id.clone();
                row.child(button(SharedString::from(format!("addon-remove-{id}")), p, t!("Kaldır"), move |_, cx| remove(rid.clone(), cx))).into_any_element()
            }
            (None, None) if available => {
                let i = info.clone();
                button(SharedString::from(format!("addon-install-{id}")), p, t!("Kur"), move |_, cx| install(i.clone(), cx))
            }
            (None, None) => div().into_any_element(),
        };
        out.push(card(p, L::Download, &info.name, Some(&desc), control));
        let source = info.source.clone();
        if !source.is_empty() {
            out.push(
                div()
                    .id(SharedString::from(format!("addon-source-{id}")))
                    .pl_4()
                    .pt_1()
                    .text_xs()
                    .text_color(p.accent)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(format!("{} {}", t!("Kaynak kodu:"), source))
                    .on_click(move |_, _, _| open_url(source.clone()))
                    .into_any_element(),
            );
        }
    }
    let _ = v_flex;
    out
}
