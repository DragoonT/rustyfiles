//! Settings → Dock: the dock's and the shell's settings on a page of their own.
//!
//! They used to be a block at the end of another page and were not found. Now the page is built from ONE list, [`entries`]:
//! every `dock_*` and `shell_*` setting that has a control, with the section it sits in, what it needs, and the texts. The page
//! draws that list and nothing else, and the Settings search reads the same cards (every card builder notes its title and
//! description for the index), so the page and the index cannot drift apart. The list itself is plain data and is tested without a
//! window: no setting left out, none twice, every switch wired to its own field, the dependency on the dock.

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::dock::size_pref::DockSizePref;
use crate::settings::{self, DockMagnify, Settings};
use crate::settings_view::{button, card_body, dropdown, section, shell, sw, switch};
use crate::t;
use crate::theme::Palette;

/// The groups of the page, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Left,
    Middle,
    Right,
    /// What hides Explorer's taskbar while the dock runs (the desktop's own settings are on the Masaüstü page: `desktop_page.rs`).
    Taskbar,
    Experimental,
}

impl Section {
    pub(crate) const ALL: [Section; 6] = [Section::General, Section::Left, Section::Middle, Section::Right, Section::Taskbar, Section::Experimental];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Section::General => t!("GENEL"),
            Section::Left => t!("SOL ADA"),
            Section::Middle => t!("ORTA ADA"),
            Section::Right => t!("SAĞ ADA"),
            Section::Taskbar => t!("GÖREV ÇUBUĞU"),
            Section::Experimental => t!("DENEYSEL"),
        }
    }
}

/// What a setting needs before it does anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Needs {
    /// Nothing: it is the switch that starts the dock.
    Nothing,
    /// The dock runs: the islands, the panels and the shell host are all one process, started only by the dock switch.
    DockOn,
    /// Switching it ON needs the dock running, but switching it OFF always works. For the shell's switches: they start hooks and
    /// hide Windows' taskbar, so one that is still on in the file while the dock is off (turned off in a hurry, or the dock stopped
    /// by something else) must be possible to turn off without starting the dock first.
    DockOnToTurnOn,
}

impl Needs {
    /// Whether the control can be used with these settings.
    pub(crate) fn met(self, s: &Settings) -> bool {
        match self {
            Needs::Nothing => true,
            Needs::DockOn | Needs::DockOnToTurnOn => s.dock_enabled,
        }
    }

    /// The one line shown under a control that cannot be used yet.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Needs::Nothing => "",
            Needs::DockOn | Needs::DockOnToTurnOn => t!("Dock kapalı: bu ayar yalnızca Dock çalışırken işe yarar. Açmak için sayfanın başındaki Dock anahtarını kullanın."),
        }
    }
}

/// The control of an entry, with how it reads and writes its own field (the tests check that it touches that field and no other).
#[derive(Clone, Copy)]
pub(crate) enum Control {
    /// A switch on a boolean setting.
    Switch(fn(&Settings) -> bool, fn(&mut Settings, bool)),
    /// The size slider (`dock::size_slider`, which writes the field itself): the setter is what ties the entry to its field in the tests.
    Size(fn(&Settings) -> DockSizePref, #[cfg_attr(not(test), allow(dead_code))] fn(&mut Settings, DockSizePref)),
    Magnify(fn(&Settings) -> DockMagnify, fn(&mut Settings, DockMagnify)),
    City(fn(&Settings) -> &str, fn(&mut Settings, String)),
}

/// How an entry is drawn with the current settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    /// Fully usable.
    Live,
    /// The dock is off but the switch is still on: it can be turned off and nothing else.
    OffOnly,
    /// The dock is off: dimmed, nothing to click, one line says why.
    Inert,
}

/// One setting of the page.
#[derive(Clone, Copy)]
pub(crate) struct Entry {
    /// The control's element id (kept as it was on the old page).
    pub id: &'static str,
    /// The field of `Settings` it changes (only the tests read it: they compare these with the fields the struct really has).
    #[cfg_attr(not(test), allow(dead_code))]
    pub field: &'static str,
    pub section: Section,
    pub needs: Needs,
    pub icon: L,
    pub title: &'static str,
    /// The description; none for the city, whose line shows the city (see [`city_line`]).
    pub desc: Option<&'static str>,
    /// Its real path has never been run on a real machine (only fakes): the card says so.
    pub never_run: bool,
    pub control: Control,
}

impl Entry {
    /// Whether the control works, only turns off, or is inert with these settings.
    pub(crate) fn state(&self, s: &Settings) -> RowState {
        if self.needs.met(s) {
            return RowState::Live;
        }
        match (self.needs, self.control) {
            (Needs::DockOnToTurnOn, Control::Switch(get, _)) if get(s) => RowState::OffOnly,
            _ => RowState::Inert,
        }
    }
}

/// What the size slider does, under its title.
pub(crate) fn size_desc() -> &'static str {
    t!("Dock'un tamamı kaydırıcıyla seçtiğiniz oranda büyür ya da küçülür: adalar, simgeler, arama alanı, saat, tepsi simgeleri ve aralıklar. %100, Dock'un her zamanki boyutudur. Kaydırırken çalışan Dock sizinle birlikte değişir; ekranda ayrılan şerit ve simgelerin keskinliği bıraktığınızda güncellenir.")
}

/// The line under a switch that is still on while the dock is off.
pub(crate) fn off_only_line() -> &'static str {
    t!("Dock kapalı: bu ayar şimdi işe yaramaz ve yalnızca kapatılabilir. Yeniden açmak için önce sayfanın başındaki Dock anahtarını açın.")
}

/// The line under a card whose real path nobody has run yet.
pub(crate) fn never_run_line() -> &'static str {
    t!("Henüz hiç çalıştırılmadı: bu özelliğin gerçek yolu bir bilgisayarda denenmedi, yalnızca sahte girdilerle sınandı.")
}

/// The intro at the top of the page.
pub(crate) fn intro() -> &'static str {
    t!("Dock, ekranın altındaki üç yüzen adadır: Başlat ve arama, uygulamalar, sistem simgeleri ve saat. Deneyseldir ve varsayılan olarak kapalıdır.")
}

/// Every dock and shell setting that has a control, in the order of the page. The texts are the ones the old block had, word for word.
pub(crate) fn entries() -> Vec<Entry> {
    use Section::*;
    #[allow(clippy::too_many_arguments)]
    fn e(
        id: &'static str,
        field: &'static str,
        section: Section,
        needs: Needs,
        icon: L,
        title: &'static str,
        desc: Option<&'static str>,
        never_run: bool,
        control: Control,
    ) -> Entry {
        Entry { id, field, section, needs, icon, title, desc, never_run, control }
    }
    let dock = Needs::DockOn;
    // the shell's switches: on needs the dock, off always works
    let shell_needs = Needs::DockOnToTurnOn;
    vec![
        // ---- GENEL
        e(
            "dock-show",
            "dock_enabled",
            General,
            Needs::Nothing,
            L::PanelBottom,
            t!("Dock'u göster (deneysel)"),
            Some(t!("Ekranın altında üç yüzen ada: Başlat ve arama, uygulamalar, sistem simgeleri ve saat. Ayrı bir süreç olarak çalışır ve rustyfiles kapansa da açık kalır; Windows görev çubuğuna dokunmaz.")),
            false,
            Control::Switch(|s| s.dock_enabled, |s, v| s.dock_enabled = v),
        ),
        e(
            "dock-reserve",
            "dock_reserve",
            General,
            dock,
            L::AppWindow,
            t!("Pencereler dock'un altına girmesin"),
            Some(t!("Büyütülmüş pencereler dock'un üstünde durur. Kapalıyken dock yalnızca pencerelerin üstünde yüzer.")),
            false,
            Control::Switch(|s| s.dock_reserve, |s, v| s.dock_reserve = v),
        ),
        e(
            "dock-size",
            "dock_size",
            General,
            dock,
            L::Maximize2,
            t!("Dock boyutu"),
            Some(size_desc()),
            false,
            Control::Size(|s| s.dock_size, |s, v| s.dock_size = v),
        ),
        e(
            "dock-magnify",
            "dock_magnify",
            General,
            dock,
            L::ZoomIn,
            t!("Simge büyütme"),
            Some(t!("Fare dock'un üzerindeyken simgeler Mac'teki gibi büyür: Hafif 1,25 katına, Tam 1,55 katına kadar.")),
            false,
            Control::Magnify(|s| s.dock_magnify, |s, v| s.dock_magnify = v),
        ),
        // ---- SOL ADA
        e(
            "dock-own-start",
            "dock_own_start",
            Left,
            dock,
            L::Search,
            t!("rustyfiles Başlat menüsünü kullan"),
            Some(t!("Dock'taki logo ve arama alanı Windows'un Başlat menüsü yerine rustyfiles'ın kendi panelini açar. Kapalıyken Windows'un Başlat menüsü ve hızlı arama açılır.")),
            false,
            Control::Switch(|s| s.dock_own_start, |s, v| s.dock_own_start = v),
        ),
        e(
            "dock-start-recommended",
            "dock_start_recommended",
            Left,
            dock,
            L::Clock,
            t!("Başlat'ta önerilenler"),
            Some(t!("Başlat panelinde sabitlenmiş uygulamaların altında en son açtığınız belgeler (ne zaman açıldığıyla birlikte) görünür. Kapalıyken bölüm çizilmez ve sabitlenenler o yeri kullanır.")),
            false,
            Control::Switch(|s| s.dock_start_recommended, |s, v| s.dock_start_recommended = v),
        ),
        e(
            "dock-start-grow",
            "dock_start_grow",
            Left,
            dock,
            L::Sparkles,
            t!("Başlat adadan büyüyerek açılsın"),
            Some(t!("Başlat düğmesine ya da arama alanına basınca soldaki cam ada yukarı ve sağa büyüyüp Başlat paneline dönüşür, kapanırken adaya geri küçülür. Kapalıyken panel eskisi gibi adanın üstünde ayrı bir pencere olarak açılır. Animasyonlar kapalıysa tek karede açılır.")),
            false,
            Control::Switch(|s| s.dock_start_grow, |s, v| s.dock_start_grow = v),
        ),
        e("dock-city-edit", "dock_weather_city", Left, dock, L::MapPin, t!("Hava durumu şehri"), None, false, Control::City(|s| s.dock_weather_city.as_str(), |s, v| s.dock_weather_city = v)),
        // ---- ORTA ADA
        e(
            "dock-hover-previews",
            "dock_hover_previews",
            Middle,
            dock,
            L::AppWindow,
            t!("Simge üzerinde pencere önizlemesi"),
            Some(t!("Dock'taki çalışan bir uygulamanın simgesinde fareyi bir an bekletince, o uygulamanın pencereleri canlı küçük resimlerle üstte görünür: pencereye tıklayınca öne gelir, x pencereyi kapatmasını ister.")),
            false,
            Control::Switch(|s| s.dock_hover_previews, |s, v| s.dock_hover_previews = v),
        ),
        e(
            "dock-app-menu",
            "dock_app_menu",
            Middle,
            dock,
            L::Pin,
            t!("Simge sağ tık menüsü"),
            Some(t!("Dock'taki bir uygulamanın simgesine sağ tıklayınca rustyfiles'ın kendi menüsü açılır: uygulamanın adı, Dock'a sabitle / Dock'tan kaldır ve pencereleri kapat. Dock'un sabitleme listesi kendisinindir; Windows'un görev çubuğu sabitlerine yazılmaz.")),
            false,
            Control::Switch(|s| s.dock_app_menu, |s, v| s.dock_app_menu = v),
        ),
        // ---- SAĞ ADA
        e(
            "dock-own-cc",
            "dock_own_control_centre",
            Right,
            dock,
            L::Volume2,
            t!("rustyfiles denetim merkezini kullan"),
            Some(t!("Dock'taki ağ ve ses simgeleri Windows'un hızlı ayarları yerine rustyfiles'ın panelini açar: ses, ağ, Bluetooth, şimdi çalan, parlaklık. Kapalıyken Windows'un hızlı ayarları açılır.")),
            false,
            Control::Switch(|s| s.dock_own_control_centre, |s, v| s.dock_own_control_centre = v),
        ),
        e(
            "dock-own-cal",
            "dock_own_calendar",
            Right,
            dock,
            L::Calendar,
            t!("rustyfiles takvimini kullan"),
            Some(t!("Dock'taki saate tıklayınca Windows'un takvimi yerine rustyfiles'ın paneli açılır: ay, hafta numarasız gün tablosu, saniyeli saat. Kapalıyken Windows'un bildirim merkezi ve takvimi açılır.")),
            false,
            Control::Switch(|s| s.dock_own_calendar, |s, v| s.dock_own_calendar = v),
        ),
        e(
            "dock-own-notif",
            "dock_own_notifications",
            Right,
            dock,
            L::Bell,
            t!("rustyfiles bildirim panelini kullan"),
            Some(t!("Dock'taki zile tıklayınca Windows'un bildirim merkezi yerine rustyfiles'ın paneli açılır: diğer uygulamaların bildirimleri uygulamaya göre gruplanır, zilde bildirim varken küçük bir nokta görünür. Kapalıyken Windows'un bildirim merkezi açılır.")),
            false,
            Control::Switch(|s| s.dock_own_notifications, |s, v| s.dock_own_notifications = v),
        ),
        // ---- GÖREV ÇUBUĞU (the desktop's switch moved to the Masaüstü page)
        e(
            "shell-hide-taskbar",
            "shell_hide_taskbar",
            Taskbar,
            shell_needs,
            L::Monitor,
            t!("Windows görev çubuğunu gizle (deneysel)"),
            Some(t!("Dock çalıştığı sürece Windows'un görev çubuğu ve bildirim alanı simgeleri gizlenir; dock durunca geri gelir. Windows'un hiçbir görev çubuğu ayarı değiştirilmez. rustyfiles beklenmedik biçimde durursa küçük bir yardımcı program (rustyfiles-shellguard.exe) görev çubuğunu bir iki saniye içinde geri getirir; yardımcı çalışmıyorsa görev çubuğu gizlenmez. Windows tuşu ve Windows'un kendi açılır pencereleri (Başlat, hızlı ayarlar, saat) hâlâ gizli görev çubuğunun yerine göre açılır; Windows tuşunun rustyfiles Başlat'ını açması için aşağıdaki Windows tuşu anahtarını da açmanız önerilir.")),
            true,
            Control::Switch(|s| s.shell_hide_taskbar, |s, v| s.shell_hide_taskbar = v),
        ),
        // ---- DENEYSEL
        e(
            "shell-winkey",
            "shell_win_key",
            Experimental,
            shell_needs,
            L::Keyboard,
            t!("Windows tuşu rustyfiles Başlat'ını açsın (deneysel)"),
            Some(t!("Açıkken rustyfiles klavyeyi sistem genelinde bir kancayla dinler ve yalnızca Windows tuşuna tek başına basıp bırakmaya bakar; başka hiçbir tuşu saklamaz, kaydetmez ya da başka yere göndermez. Windows'un kendi Başlat menüsü açılmasın diye tek bir görünmez, hiçbir şey yapmayan tuş gönderir. Windows tuşlu kısayollara dokunmaz. rustyfiles durursa ya da takılırsa Windows eski davranışına döner.")),
            true,
            Control::Switch(|s| s.shell_win_key, |s, v| s.shell_win_key = v),
        ),
        e(
            "shell-altab",
            "shell_own_switcher",
            Experimental,
            shell_needs,
            L::Layers,
            t!("Alt+Tab rustyfiles pencere değiştiricisini açsın (deneysel)"),
            Some(t!("Açıkken rustyfiles klavyeyi sistem genelinde bir kancayla dinler ve yalnızca Alt+Tab'a (ve açıkken Sekme, oklar, Enter, Esc, Alt'ı bırakmaya) bakar; başka hiçbir tuşu saklamaz, kaydetmez ya da başka yere göndermez. Alt bırakılırken öndeki programın menü çubuğu etkinleşmesin diye tek bir görünmez, hiçbir şey yapmayan tuş gönderir. rustyfiles durursa ya da takılırsa Windows'un kendi Alt+Tab'ı geri gelir.")),
            true,
            Control::Switch(|s| s.shell_own_switcher, |s, v| s.shell_own_switcher = v),
        ),
        e(
            "shell-tray",
            "shell_tray_host",
            Experimental,
            shell_needs,
            L::Layers,
            t!("Tepsi simgelerini rustyfiles de toplasın (deneysel)"),
            Some(t!("Açıkken rustyfiles, programların bildirim alanına gönderdiği simge iletilerini Explorer'ın görev çubuğu penceresinin önünde bir pencereyle karşılar, kendine kaydeder ve her iletiyi değiştirmeden Explorer'a da iletir; Explorer'ın kendi tepsisi aynen çalışır. Dock simgeleri henüz göstermez. Kapatınca ya da rustyfiles durunca bu pencere kaldırılır ve programlardan simgelerini yeniden eklemeleri istenir. Her ileti Explorer'a da gittiği için Explorer'ın kendi listesi eksiksiz kalır.")),
            true,
            Control::Switch(|s| s.shell_tray_host, |s, v| s.shell_tray_host = v),
        ),
    ]
}

/// The entries of each section, in the order of the page; a section with no entry is not listed.
pub(crate) fn grouped(all: &[Entry]) -> Vec<(Section, Vec<Entry>)> {
    Section::ALL
        .iter()
        .map(|&sec| (sec, all.iter().copied().filter(|e| e.section == sec).collect::<Vec<_>>()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// The text under the weather city: the city, or what happens without one.
pub(crate) fn city_line(city: &str) -> String {
    let city = city.trim();
    if city.is_empty() { t!("Ayarlanmadı: hava durumu gösterilmez").to_string() } else { city.to_string() }
}

// ---------------------------------------------------------------- the page

/// The page: the intro, then the sections. A control that needs the dock is drawn dimmed and inert, with a line that says why;
/// it is never left out, so it is found by the search and the user sees where it is. `size_slider` is the size row's slider, built by
/// the caller (it needs the app's focus handles); without one the row shows the value.
pub(crate) fn page(p: &Palette, s: &Settings, size_slider: Option<AnyElement>) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = vec![div().pb_1().text_sm().text_color(p.text_secondary).child(intro()).into_any_element()];
    let mut size_slider = size_slider;
    for (sec, rows) in grouped(&entries()) {
        out.push(section(p, sec.title()));
        for e in rows {
            let slider = if matches!(e.control, Control::Size(..)) { size_slider.take() } else { None };
            out.push(row(p, s, &e, slider));
        }
    }
    out
}

fn row(p: &Palette, s: &Settings, e: &Entry, slider: Option<AnyElement>) -> AnyElement {
    let state = e.state(s);
    let desc: Option<String> = match e.control {
        Control::City(get, _) => Some(city_line(get(s))),
        _ => e.desc.map(str::to_string),
    };
    let is_size = matches!(e.control, Control::Size(..));
    let control = match slider {
        Some(slider) if is_size && state != RowState::Inert => slider,
        // (the size row without its slider, as in a test, shows the value)
        _ if state == RowState::Inert || is_size => inert_control(p, s, e),
        _ => live_control(p, s, e),
    };
    // (the card notes its title and description for the Settings search, usable or not)
    let body = card_body(p, e.icon, e.title.into(), desc.map(Into::into), Some(control));
    // (the opacity is the card's own: it reaches the icon, the texts and the control)
    let body = if state == RowState::Inert { body.opacity(0.55) } else { body };
    shell(p)
        .child(body)
        .when(state == RowState::Inert, |d| d.child(note_line(p, e.needs.reason(), false)))
        .when(state == RowState::OffOnly, |d| d.child(note_line(p, off_only_line(), false)))
        .when(e.never_run, |d| d.child(note_line(p, never_run_line(), true)))
        .into_any_element()
}

/// A small line under a card's text (aligned with it: past the icon).
fn note_line(p: &Palette, text: &'static str, warning: bool) -> Div {
    div().pl(px(50.)).pr_4().pb_3().text_xs().text_color(if warning { p.danger } else { p.text_secondary }).child(text)
}

fn magnify_choices() -> Vec<(DockMagnify, String)> {
    vec![(DockMagnify::Off, t!("Kapalı").into()), (DockMagnify::Subtle, t!("Hafif").into()), (DockMagnify::Full, t!("Tam").into())]
}

/// The working control of an entry.
fn live_control(p: &Palette, s: &Settings, e: &Entry) -> AnyElement {
    match e.control {
        Control::Switch(get, set) => sw(e.id, p, get(s), set),
        // (the slider is built by `dock::size_slider` and handed to `page`; with none the row shows the value)
        Control::Size(..) => inert_control(p, s, e),
        Control::Magnify(get, set) => dropdown(e.id, p, get(s), magnify_choices(), set),
        Control::City(get, set) => {
            let city = get(s).trim().to_string();
            h_flex()
                .gap_2()
                .child(button(e.id, p, t!("Değiştir…"), {
                    let city = city.clone();
                    move |window, cx| {
                        let ok = crate::prompt::Button::new(t!("Kaydet"), crate::prompt::Kind::Primary, move |text, _, cx| {
                            let text = text.unwrap_or_default().trim().to_string();
                            settings::update(cx, |s| set(s, text));
                            Ok(())
                        });
                        crate::prompt::show(
                            t!("Hava durumu şehri"),
                            Some(t!("Dock'un sol adasında sıcaklık gösterilir (Open-Meteo). Boş bırakırsanız hava durumu gösterilmez ve ağa hiç istek gitmez.").to_string()),
                            Some((city.clone(), t!("Şehir adı, örneğin: Erzincan").to_string())),
                            vec![ok, crate::prompt::Button::cancel()],
                            window,
                            cx,
                        );
                    }
                }))
                .children((!city.is_empty()).then(|| button("dock-city-clear", p, t!("Temizle"), move |_, cx| settings::update(cx, |s| set(s, String::new())))))
                .into_any_element()
        }
    }
}

/// What stands where the control would be while the dock is off: the current value, with nothing to click.
fn inert_control(p: &Palette, s: &Settings, e: &Entry) -> AnyElement {
    let label = |text: String| {
        div().h(px(32.)).px_3().flex().items_center().rounded(crate::theme::RADIUS_SM).border_1().border_color(p.border).text_sm().child(text).into_any_element()
    };
    match e.control {
        // over glass the same liquid switch as the live ones (a click does nothing); the card around it is dimmed
        Control::Switch(get, _) if p.liquid => switch(e.id, p, get(s), |_, _| {}),
        Control::Switch(get, _) => {
            let on = get(s);
            h_flex()
                .gap_3()
                .items_center()
                .child(div().text_sm().text_color(p.text_secondary).child(if on { t!("Açık") } else { t!("Kapalı") }))
                .child(
                    h_flex()
                        .w(px(40.))
                        .h(px(20.))
                        .px(px(3.))
                        .items_center()
                        .rounded_full()
                        .border_1()
                        .border_color(p.text_secondary)
                        .when(on, |d| d.justify_end())
                        .child(div().size(px(12.)).rounded_full().bg(p.text_secondary)),
                )
                .into_any_element()
        }
        Control::Size(get, _) => label(get(s).percent_text(crate::i18n::is_tr())),
        Control::Magnify(get, _) => label(magnify_choices().into_iter().find(|(v, _)| *v == get(s)).map(|(_, l)| l).unwrap_or_default()),
        Control::City(..) => v_flex().into_any_element(),
    }
}

/// For tests that compare texts: the interface language is one process-wide value and one test of `i18n` switches it for a few
/// milliseconds, so a test that reads the Turkish keys runs its body again when the language was not Turkish all the way through.
#[cfg(test)]
pub(crate) fn while_turkish(body: impl Fn()) {
    for _ in 0..400 {
        if crate::i18n::is_tr() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&body));
            if crate::i18n::is_tr() {
                if let Err(e) = result {
                    std::panic::resume_unwind(e);
                }
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the interface language did not settle on Turkish");
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    // (not `super::*`: the glob of gpui_kit up there brings a `test` of its own that would shadow `#[test]`)
    use super::{Control, Entry, Needs, RowState, while_turkish, Section, city_line, entries, grouped, intro, magnify_choices, never_run_line, off_only_line};
    use crate::dock::size_pref::DockSizePref;
    use crate::settings::Settings;

    #[test]
    fn ids_and_fields_are_unique() {
        let all = entries();
        let ids: HashSet<_> = all.iter().map(|e| e.id).collect();
        let fields: HashSet<_> = all.iter().map(|e| e.field).collect();
        assert_eq!(ids.len(), all.len(), "an element id is used twice");
        assert_eq!(fields.len(), all.len(), "a setting is on the page twice");
    }

    /// The `dock_*` / `shell_*` fields the settings struct has, read from its source, so a setting added later without a place on
    /// the page fails here instead of being lost again.
    fn struct_fields() -> HashSet<String> {
        include_str!("settings.rs")
            .lines()
            .filter_map(|l| {
                let rest = l.trim_start().strip_prefix("pub ")?;
                let name = rest.split(':').next()?.trim();
                // (`shell_submenu` is the context menu's "Show more options" switch, nothing to do with the shell host)
                // (`shell_desktop_cover` is the desktop's switch: it is on the Masaüstü page, `desktop_page.rs`)
                // (`dock_weather_location` is on this page too, drawn by a card of its own after the entries: `dock/weather_card.rs`,
                // added in `settings_view.rs`; it shows Windows' own permission, which an entry cannot)
                let ours = (name.starts_with("dock_") || name.starts_with("shell_")) && name != "shell_submenu" && name != "shell_desktop_cover" && name != "dock_weather_location";
                (ours && rest.contains(':') && !rest.contains('(')).then(|| name.to_string())
            })
            .collect()
    }

    #[test]
    fn every_dock_and_shell_setting_is_on_the_page_and_nothing_else() {
        let on_page: HashSet<String> = entries().iter().map(|e| e.field.to_string()).collect();
        let in_struct = struct_fields();
        assert!(in_struct.len() >= 16, "the source scan found too little: {in_struct:?}");
        let missing: Vec<_> = in_struct.difference(&on_page).collect();
        let unknown: Vec<_> = on_page.difference(&in_struct).collect();
        assert!(missing.is_empty(), "settings with no place on the Dock page: {missing:?}");
        assert!(unknown.is_empty(), "entries for a setting that does not exist: {unknown:?}");
    }

    #[test]
    fn no_other_page_draws_a_dock_or_shell_setting() {
        // the old block lived in settings_view.rs; nothing of it may come back there (or be drawn a second time)
        let other = include_str!("settings_view.rs");
        for e in entries() {
            assert!(!uses_field(other, e.field), "settings_view.rs still uses {}", e.field);
        }
    }

    /// Whether `src` reads or writes `.field` on any binding (`s.`, `st.`, `settings.`, `cx.global::<Settings>().`), not a longer name
    /// that only starts with it.
    fn uses_field(src: &str, field: &str) -> bool {
        let needle = format!(".{field}");
        src.match_indices(&needle).any(|(at, _)| !src[at + needle.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    }

    #[test]
    fn the_field_scan_sees_any_binding_name_and_not_longer_names() {
        assert!(uses_field("let on = s.dock_size;", "dock_size"));
        assert!(uses_field("if settings.dock_size == x {}", "dock_size"));
        assert!(uses_field("st.shell_win_key=true", "shell_win_key"));
        assert!(uses_field("cx.global::<Settings>().dock_reserve.clone()", "dock_reserve"));
        assert!(!uses_field("let on = s.dock_size_extra;", "dock_size"));
        assert!(!uses_field("dock_size without a dot", "dock_size"));
        assert!(!uses_field("s.dock_enabled_x + s.hdock_enabled", "dock_enabled"));
    }

    #[test]
    fn every_section_has_entries_and_follows_the_order() {
        let g = grouped(&entries());
        let secs: Vec<Section> = g.iter().map(|(s, _)| *s).collect();
        assert_eq!(secs, Section::ALL.to_vec(), "an empty section would be a heading with nothing under it");
        // the sections hold what the page promises
        let ids = |sec: Section| g.iter().find(|(s, _)| *s == sec).map(|(_, v)| v.iter().map(|e| e.field).collect::<Vec<_>>()).unwrap();
        assert_eq!(ids(Section::General), ["dock_enabled", "dock_reserve", "dock_size", "dock_magnify"]);
        assert_eq!(ids(Section::Left), ["dock_own_start", "dock_start_recommended", "dock_start_grow", "dock_weather_city"]);
        assert_eq!(ids(Section::Middle), ["dock_hover_previews", "dock_app_menu"]);
        assert_eq!(ids(Section::Right), ["dock_own_control_centre", "dock_own_calendar", "dock_own_notifications"]);
        assert_eq!(ids(Section::Taskbar), ["shell_hide_taskbar"]);
        assert_eq!(ids(Section::Experimental), ["shell_win_key", "shell_own_switcher", "shell_tray_host"]);
    }

    #[test]
    fn the_taskbar_text_that_points_below_has_its_target_below() {
        // "aşağıdaki Windows tuşu anahtarı": the Windows key switch must come after the taskbar one
        let all: Vec<_> = grouped(&entries()).into_iter().flat_map(|(_, v)| v).map(|e| e.field).collect();
        let pos = |f: &str| all.iter().position(|x| *x == f).unwrap();
        assert!(pos("shell_win_key") > pos("shell_hide_taskbar"));
    }

    #[test]
    fn each_switch_reads_and_writes_its_own_field_only() {
        let switches: Vec<Entry> = entries().into_iter().filter(|e| matches!(e.control, Control::Switch(..))).collect();
        assert!(switches.len() >= 13);
        for (i, e) in switches.iter().enumerate() {
            let Control::Switch(get, set) = e.control else { unreachable!() };
            for start in [false, true] {
                // begin from a state where every switch has the same value, flip one, and only that one may differ
                let mut s = Settings::default();
                for o in &switches {
                    if let Control::Switch(_, set_o) = o.control {
                        set_o(&mut s, start);
                    }
                }
                set(&mut s, !start);
                for (j, o) in switches.iter().enumerate() {
                    let Control::Switch(get_o, _) = o.control else { unreachable!() };
                    assert_eq!(get_o(&s), if i == j { !start } else { start }, "{} was changed through {}", o.field, e.field);
                }
                assert_eq!(get(&s), !start);
            }
        }
    }

    #[test]
    fn a_switch_shows_the_value_the_default_settings_have() {
        let d = Settings::default();
        let get = |f: &str| match entries().into_iter().find(|e| e.field == f).unwrap().control {
            Control::Switch(g, _) => g(&d),
            _ => panic!("{f} is not a switch"),
        };
        assert!(!get("dock_enabled"), "experimental: off until asked for");
        assert!(get("dock_reserve"));
        assert!(!get("shell_win_key") && !get("shell_own_switcher") && !get("shell_tray_host") && !get("shell_hide_taskbar"));
    }

    #[test]
    fn everything_but_the_dock_switch_needs_the_dock() {
        let off = Settings { dock_enabled: false, ..Default::default() };
        let on = Settings { dock_enabled: true, ..Default::default() };
        for e in entries() {
            if e.field == "dock_enabled" {
                assert_eq!(e.needs, Needs::Nothing);
                assert!(e.needs.met(&off), "the switch that starts the dock must work while the dock is off");
            } else {
                assert!(matches!(e.needs, Needs::DockOn | Needs::DockOnToTurnOn), "{}", e.field);
                assert!(!e.needs.met(&off), "{} must not be usable while the dock is off", e.field);
                assert!(e.needs.met(&on));
                assert!(!e.needs.reason().is_empty());
            }
        }
        assert!(Needs::Nothing.reason().is_empty());
    }

    /// Writes a control once with a value other than the current one.
    fn change_once(e: &Entry, s: &mut Settings) {
        match e.control {
            Control::Switch(get, set) => set(s, !get(s)),
            Control::Size(get, set) => set(s, DockSizePref::at_position((get(s).position() + 1) % 16)),
            Control::Magnify(get, set) => set(s, magnify_choices().into_iter().map(|(v, _)| v).find(|v| *v != get(s)).unwrap()),
            Control::City(get, set) => {
                assert_ne!(get(s), "Erzincan");
                set(s, "Erzincan".to_string());
            }
        }
    }

    /// Every control written once, and the JSON of the settings before and after: exactly the entry's own field may differ. This ties
    /// a dropdown's or the city's closure to its entry, which the switches' test above cannot.
    #[test]
    fn every_control_changes_its_own_field_and_no_other() {
        let before = serde_json::to_value(Settings::default()).unwrap();
        let before = before.as_object().unwrap();
        for e in entries() {
            let mut s = Settings::default();
            change_once(&e, &mut s);
            let after = serde_json::to_value(&s).unwrap();
            let after = after.as_object().unwrap();
            let all: HashSet<&String> = before.keys().chain(after.keys()).collect();
            let changed: Vec<&String> = all.into_iter().filter(|k| before.get(*k) != after.get(*k)).collect();
            assert_eq!(changed.len(), 1, "{} changed {changed:?}", e.field);
            assert_eq!(changed[0], e.field, "the control of {} changed {}", e.field, changed[0]);
        }
    }

    #[test]
    fn a_shell_switch_that_is_on_can_be_turned_off_while_the_dock_is_off() {
        let shell = ["shell_hide_taskbar", "shell_win_key", "shell_own_switcher", "shell_tray_host"];
        let on = Settings { dock_enabled: true, ..Default::default() };
        for e in entries() {
            let mut s = Settings { dock_enabled: false, ..Default::default() };
            // with the dock on everything works
            assert_eq!(e.state(&on), RowState::Live, "{}", e.field);
            let Control::Switch(_, set) = e.control else {
                // dropdowns and the city are never on or off: inert while the dock is off
                assert_eq!(e.state(&s), RowState::Inert, "{}", e.field);
                continue;
            };
            if e.field == "dock_enabled" {
                assert_eq!(e.state(&s), RowState::Live);
                continue;
            }
            set(&mut s, true);
            if shell.contains(&e.field) {
                assert_eq!(e.state(&s), RowState::OffOnly, "{} is on, the dock is off: it must be possible to turn it off", e.field);
                set(&mut s, false);
                assert_eq!(e.state(&s), RowState::Inert, "{} is off now: turning it on needs the dock", e.field);
            } else {
                // the dock's own switches stay dimmed, as the brief says
                assert_eq!(e.state(&s), RowState::Inert, "{}", e.field);
            }
        }
        assert!(!off_only_line().is_empty() && off_only_line() != Needs::DockOn.reason());
    }

    #[test]
    fn the_experimental_ones_say_so_and_every_text_has_its_english_pair() {
        while_turkish(|| {
            let en: HashSet<&str> = crate::i18n_en::EN.iter().map(|(k, _)| *k).collect();
            let has = |text: &str| en.contains(text);
            for e in entries() {
                if e.field == "dock_enabled" || e.field.starts_with("shell_") {
                    assert!(e.title.contains("(deneysel)"), "{} is experimental and must say so", e.field);
                }
                assert!(has(e.title), "no English for the title of {}", e.field);
                assert!(e.desc.is_none_or(has), "no English for the description of {}", e.field);
            }
            for sec in Section::ALL {
                assert!(has(sec.title()), "{sec:?}");
            }
            for text in [intro(), never_run_line(), off_only_line(), Needs::DockOn.reason()] {
                assert!(has(text), "{text}");
            }
            for (_, l) in magnify_choices() {
                assert!(has(&l), "{l}");
            }
        });
    }

    #[test]
    fn the_warnings_of_the_risky_switches_keep_their_key_sentences() {
        while_turkish(|| {
            // the old block's lines, word for word (a script compared all of them against the old file; these are the ones about safety)
            let desc = |f: &str| entries().into_iter().find(|e| e.field == f).unwrap().desc.unwrap();
            assert!(desc("shell_hide_taskbar").contains("rustyfiles-shellguard.exe"));
            assert!(desc("shell_hide_taskbar").contains("yardımcı çalışmıyorsa görev çubuğu gizlenmez"));
            assert!(desc("shell_win_key").contains("başka hiçbir tuşu saklamaz, kaydetmez ya da başka yere göndermez"));
            assert!(desc("shell_win_key").contains("rustyfiles durursa ya da takılırsa Windows eski davranışına döner"));
            assert!(desc("shell_own_switcher").contains("başka hiçbir tuşu saklamaz, kaydetmez ya da başka yere göndermez"));
            assert!(desc("shell_own_switcher").contains("Windows'un kendi Alt+Tab'ı geri gelir"));
            assert!(desc("shell_tray_host").contains("Her ileti Explorer'a da gittiği için Explorer'ın kendi listesi eksiksiz kalır"));
        });
    }

    #[test]
    fn the_four_paths_nobody_has_run_say_so() {
        let never: Vec<_> = entries().into_iter().filter(|e| e.never_run).map(|e| e.field).collect();
        assert_eq!(never, ["shell_hide_taskbar", "shell_win_key", "shell_own_switcher", "shell_tray_host"]);
        assert!(!never_run_line().is_empty());
    }

    #[test]
    fn every_entry_has_texts_and_the_city_line_is_made_from_the_city() {
        while_turkish(|| {
            for e in entries() {
                assert!(!e.title.is_empty(), "{}", e.field);
                assert!(e.desc.is_some_and(|d| !d.is_empty()) || matches!(e.control, Control::City(..)), "{} has no description", e.field);
                assert!(!e.id.is_empty());
            }
            assert!(!intro().is_empty());
            assert_eq!(city_line("  Erzincan "), "Erzincan");
            assert!(!city_line("   ").is_empty() && city_line("   ") != "   ");
        });
    }
}
