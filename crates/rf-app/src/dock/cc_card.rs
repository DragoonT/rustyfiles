//! Settings' card for what our control centre shows: one chip for each part of the panel (a tile, a card, the row of quick buttons)
//! and one for each quick button, lit while that thing is shown; a click hides it or shows it again. The names and the rule for the
//! list are `cc_model` (`PARTS`, `Quick`, `with_hidden`, tested there); this file only draws them and writes `dock_cc_hidden`.
//!
//! WHY (the user, 2026-11-15, when the panel got everything Windows' quick settings and macOS's Control Centre have: "hepsini
//! ekleyelim. ama isteyen orayı minimalize edebilsin de."): a panel that has everything is too much for whoever wants the volume
//! and nothing else. Everything is shown until it is hidden here, and what a machine does not have (a battery, a Wi-Fi radio) is
//! never shown whatever this list says.
//!
//! A card of its own and not an entry of the Dock page for the reason `status_card.rs` gives: an entry is one control on one field,
//! and this is a list. The page puts it right under the switch that chooses our control centre.

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::cc_model::{self, Quick, Words};
use crate::settings::{self, Settings};
use crate::settings_view::{card_body, shell};
use crate::t;
use crate::theme::Palette;

fn desc() -> &'static str {
    t!("İşaretli olanlar panelde görünür; gizlemek ya da geri getirmek için üzerine tıklayın. Bilgisayarda olmayan bir şey (pil, Wi-Fi) zaten gösterilmez.")
}

pub(crate) fn card(p: &Palette, s: &Settings, live: bool) -> Div {
    let words = Words::current();
    let (accent, border, hover, pressed, secondary) = (p.accent, p.border, p.hover, p.pressed, p.text_secondary);
    let chip = |key: &'static str, label: &'static str| -> AnyElement {
        let shown = !s.dock_cc_hidden.iter().any(|h| h == key);
        h_flex()
            .id(SharedString::from(format!("dock-cc-part-{key}")))
            .h(px(28.))
            .px_2p5()
            .gap_1p5()
            .items_center()
            .rounded(crate::theme::RADIUS_SM)
            .border_1()
            .border_color(if shown { accent } else { border })
            .bg(hover)
            .text_sm()
            .when(!shown, |d| d.text_color(secondary))
            .child(Icon::new(if shown { L::Check } else { L::EyeOff }).size(px(14.)))
            .child(label)
            .when(live, |d| {
                d.cursor_pointer().hover(move |d| d.bg(pressed)).on_click(move |_, _, cx| settings::update(cx, move |s| s.dock_cc_hidden = cc_model::with_hidden(&s.dock_cc_hidden, key, shown)))
            })
            .into_any_element()
    };
    let group = |title: &'static str, chips: Vec<AnyElement>| {
        v_flex().pl(px(50.)).pr_4().pb_3().gap_2().child(div().text_xs().text_color(secondary).child(title)).child(h_flex().flex_wrap().gap_2().children(chips))
    };
    let parts: Vec<AnyElement> = cc_model::PARTS.into_iter().map(|key| chip(key, cc_model::part_name(key, words))).collect();
    let quick: Vec<AnyElement> = Quick::ALL.into_iter().map(|q| chip(q.key(), q.name(words))).collect();
    let inner = v_flex()
        .child(card_body(p, L::SlidersHorizontal, t!("Denetim merkezinde gösterilenler").into(), Some(desc().into()), None))
        .child(group(t!("Kartlar ve döşemeler"), parts))
        .child(group(t!("Hızlı düğmeler"), quick));
    shell(p).child(if live { inner } else { inner.opacity(0.55) })
}
