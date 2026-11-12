//! Settings -> Düzenleyici: the master switch of the automatic runs, the list of rules.

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::organize::Rule;
use rf_core::organize::summary::summary;

use super::{Organizer, Problem};
use crate::settings_view::{button, card, card_body, section, shell, switch};
use crate::t;
use crate::theme::{self, Palette};

fn small_button(id: (&'static str, usize), icon: L, tip: &'static str, p: &Palette, run: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .id(id)
        .size(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .border_1()
        .border_color(p.border)
        .bg(p.hover)
        .cursor_pointer()
        .tab_index(0)
        .hover(|s| s.bg(p.pressed))
        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
        .child(Icon::new(icon).size(px(14.)))
        .on_click(move |_, window, cx| run(window, cx))
        .into_any_element()
}

/// `blocked`: the rules cannot be saved this session, so the warning stays (no "Tamam").
fn problem_card(p: &Palette, problem: &Problem, blocked: bool) -> AnyElement {
    let text = super::problem_text(problem);
    shell(p)
        .border_color(p.danger)
        .child(
            h_flex()
                .px_4()
                .py_3()
                .gap_4()
                .items_center()
                .child(Icon::new(L::TriangleAlert).size(px(18.)).flex_shrink_0().text_color(p.danger))
                .child(div().flex_1().min_w_0().text_sm().child(text))
                .when(!blocked, |d| d.child(button("org-problem-ok", p, t!("Tamam"), |_, cx| super::dismiss_problem(cx)))),
        )
        .into_any_element()
}

fn rule_card(p: &Palette, r: &Rule, first: bool, last: bool, refusal: Option<&'static str>, cx: &mut App) -> AnyElement {
    let id = r.id;
    let home = super::home();
    let words = summary(r, &super::tr, home.as_deref());
    let on = r.enabled;
    let _ = cx;
    shell(p)
        .child(
            h_flex()
                .px_4()
                .pt_3()
                .gap_4()
                .items_start()
                .child(Icon::new(L::FolderCog).size(px(18.)).flex_shrink_0().mt(px(2.)).text_color(if on { p.text } else { p.text_tertiary }))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().text_sm().font_weight(FontWeight::MEDIUM).when(!on, |d| d.text_color(p.text_tertiary)).child(r.name.clone()))
                                .when(r.auto, |d| d.child(div().px_1p5().rounded(px(4.)).border_1().border_color(p.accent).text_xs().text_color(p.accent).child(t!("otomatik")))),
                        )
                        .child(div().text_xs().text_color(p.text_secondary).child(words))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.text_tertiary)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(format!("{}: {}{}", t!("Klasör"), r.folder.display(), if r.subfolders { format!(" · {}", t!("alt klasörler dahil")) } else { String::new() })),
                        )
                        .children(refusal.map(|why| div().text_xs().text_color(p.danger).child(crate::i18n::td(why)))),
                )
                .child(switch(("org-en", id as usize), p, on, move |_, cx| {
                    super::update(cx, |f| {
                        if let Some(r) = f.rules.iter_mut().find(|r| r.id == id) {
                            r.enabled = !on;
                        }
                    })
                })),
        )
        .child(
            h_flex()
                .px_4()
                .pt_2()
                .pb_3()
                .pl(px(50.))
                .gap_2()
                .items_center()
                .child(button(("org-run", id as usize), p, t!("Şimdi çalıştır"), move |window, cx| super::run_rule_now(id, window, cx)))
                .child(button(("org-edit", id as usize), p, t!("Kuralı değiştir"), move |window, cx| super::edit_rule(Some(id), None, window, cx)))
                .child(button(("org-del", id as usize), p, t!("Sil"), move |window, cx| delete_dialog(id, window, cx)))
                .child(div().flex_1())
                .when(!first, |d| d.child(small_button(("org-up", id as usize), L::ChevronUp, t!("Yukarı taşı (önce bu kural denenir)"), p, move |_, cx| super::update(cx, |f| f.shift(id, -1)))))
                .when(!last, |d| d.child(small_button(("org-down", id as usize), L::ChevronDown, t!("Aşağı taşı"), p, move |_, cx| super::update(cx, |f| f.shift(id, 1))))),
        )
        .into_any_element()
}

fn delete_dialog(id: u32, window: &mut Window, cx: &mut App) {
    crate::prompt::show(
        t!("Kural silinsin mi?"),
        Some(t!("Kural kaldırılır. Daha önce düzenlenmiş dosyalara dokunulmaz.").to_string()),
        None,
        vec![
            crate::prompt::Button::new(t!("Sil"), crate::prompt::Kind::Danger, move |_, _, cx| {
                super::update(cx, |f| f.remove(id));
                Ok(())
            }),
            crate::prompt::Button::cancel(),
        ],
        window,
        cx,
    );
}

pub fn page(p: &Palette, cx: &mut App) -> Vec<AnyElement> {
    super::ensure(cx);
    let Organizer { file, problem, blocked } = cx.global::<Organizer>();
    let (rules, auto_on, problem, blocked) = (file.rules.clone(), file.auto_enabled, problem.clone(), *blocked);
    let protected = super::protected();

    let mut out: Vec<AnyElement> = Vec::new();
    if let Some(pr) = &problem {
        out.push(problem_card(p, pr, blocked));
    }
    out.push(card(
        p,
        L::Zap,
        t!("Dosyalar geldikçe kendiliğinden düzenle"),
        Some(t!("Açıkken, \"kendiliğinden çalıştır\" seçili kuralların klasörleri izlenir; yeni gelen dosyalar 3 saniye durulduktan sonra önizleme gösterilmeden taşınır, yeniden adlandırılır ya da Geri Dönüşüm Kutusu'na gönderilir. Kapalıyken hiçbir şey izlenmez ve çalışmaz.")),
        switch("org-auto", p, auto_on, move |_, cx| super::update(cx, |f| f.auto_enabled = !auto_on)),
    ));
    out.push(section(p, t!("Kurallar")));
    out.push(
        shell(p)
            .child(card_body(
                p,
                L::ListFilter,
                t!("Düzenleme kuralları").into(),
                Some(t!("Bir dosyaya uyan ilk kural uygulanır. Elle çalıştırmada önce önizleme gösterilir; kendiliğinden çalışan kurallar önizleme göstermez. Her çalıştırma, Durum Merkezi'ndeki satırından tek hamlede geri alınabilir.").into()),
                Some(button("org-new", p, t!("Yeni kural"), |window, cx| super::edit_rule(None, None, window, cx))),
            ))
            .into_any_element(),
    );
    if rules.is_empty() {
        out.push(
            shell(p)
                .child(card_body(
                    p,
                    L::Sparkles,
                    t!("Henüz kural yok").into(),
                    Some(t!("Bir klasör seçin, hangi dosyalara bakacağını yazın ve nereye gideceklerini belirleyin. Hiçbir kural siz kaydedene kadar oluşmaz.").into()),
                    Some(button("org-starter", p, t!("Örnek kuralı dene"), |window, cx| super::edit_rule(None, Some(super::starter_rule()), window, cx))),
                ))
                .child(
                    div()
                        .px_4()
                        .pb_3()
                        .pl(px(50.))
                        .text_xs()
                        .text_color(p.text_secondary)
                        .child(t!("Örnek: İndirilenler klasöründeki resimler Resimler\\{year}\\{month} klasörüne gitsin.")),
                )
                .into_any_element(),
        );
    }
    let n = rules.len();
    for (i, r) in rules.iter().enumerate() {
        let refusal = super::refusal(&protected, &r.folder, r.subfolders).map(|w| w.text());
        out.push(rule_card(p, r, i == 0, i + 1 == n, refusal, cx));
    }
    out
}
