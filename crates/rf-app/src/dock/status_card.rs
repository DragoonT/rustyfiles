//! Settings' card for the right island's own buttons (safe removal, Windows Security, network, volume): one row for each, in the order
//! the island draws them, with two arrows that move the row one place and a switch that shows or hides the button. A row can also be
//! dragged onto another row and takes its place (the same kind of drag as a pinned folder of the sidebar). The rules are
//! `rf_win::dock::status_order` (tested there); this file only draws them and writes the two lists (`dock_status_order`,
//! `dock_status_hidden`). The running dock reads them when it is told that the file changed (`control::Keys::status`).
//!
//! WHY A CARD OF ITS OWN and not entries of the Dock page (`dock_page::entries`): an entry is one control on one field, and this is a
//! list. The page puts it at the end of the right island's section.

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::status_order::{self, Status};

use crate::settings::{self, Settings};
use crate::settings_view::{card_body, shell, switch};
use crate::t;
use crate::theme::Palette;

/// The button's name in the list.
pub fn name(which: Status) -> &'static str {
    match which {
        Status::Eject => t!("Güvenli kaldırma"),
        Status::Security => t!("Windows Güvenliği"),
        Status::Network => t!("Ağ"),
        Status::Volume => t!("Ses"),
    }
}

/// The picture beside the name: the button's own symbol (the network's and the volume's are their usual ones; the island's follow the
/// system's state).
pub fn icon(which: Status) -> L {
    match which {
        Status::Eject => L::Eject,
        Status::Security => L::ShieldCheck,
        Status::Network => L::Wifi,
        Status::Volume => L::Volume2,
    }
}

fn desc() -> &'static str {
    t!("Sağ adanın kendi simgelerinin sırası ve hangilerinin görüneceği. Listede üstte olan solda durur. Sırayı oklarla, satırı sürükleyerek ya da simgeyi Dock'un üzerinde sürükleyerek değiştirebilirsiniz. Güvenli kaldırma simgesi yalnızca çıkarılabilir bir sürücü takılıyken görünür.")
}

/// A row of the list while it is dragged: which button, the row it came from, and its name (the picture that follows the pointer).
#[derive(Clone)]
struct RowDrag {
    which: Status,
    ix: usize,
    title: SharedString,
}

impl Render for RowDrag {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        div().px_3().py_1p5().rounded(crate::theme::RADIUS).bg(p.card_bg).border_1().border_color(p.accent).text_sm().text_color(p.text).shadow_md().child(self.title.clone())
    }
}

/// A small square button with an arrow; `run` is `None` at the end of the list (drawn faint, nothing to click) and while the Dock is off.
fn arrow(id: SharedString, p: &Palette, icon: L, tip: &'static str, run: Option<Box<dyn Fn(&mut App)>>) -> AnyElement {
    let pressed = p.pressed;
    let base = div().id(id).size(px(28.)).flex().items_center().justify_center().rounded(crate::theme::RADIUS_SM).child(Icon::new(icon).size(px(14.)).text_color(p.text));
    match run {
        Some(run) => base
            .cursor_pointer()
            // Tab reaches the arrow, Enter / Space presses it
            .tab_index(0)
            .hover(move |s| s.bg(pressed))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
            .on_click(move |_, _, cx| run(cx))
            .into_any_element(),
        None => base.opacity(0.3).into_any_element(),
    }
}

/// The card. `live`: the Dock runs (its switch is on); otherwise the card is dimmed and nothing in it can be clicked, like the page's
/// other settings that need the Dock (the page adds the line that says why).
pub(crate) fn card(p: &Palette, s: &Settings, live: bool) -> Div {
    let order = status_order::order(&s.dock_status_order);
    let last = order.len() - 1;
    let rows = order.into_iter().enumerate().map(|(i, which)| {
        let shown = !status_order::is_hidden(&s.dock_status_hidden, which);
        let key = which.key();
        let step = |by: i32| -> Option<Box<dyn Fn(&mut App)>> {
            let at_end = if by < 0 { i == 0 } else { i == last };
            (live && !at_end).then(|| Box::new(move |cx: &mut App| settings::update(cx, |s| s.dock_status_order = status_order::moved(&s.dock_status_order, which, by))) as Box<dyn Fn(&mut App)>)
        };
        let accent = p.accent;
        h_flex()
            .id(SharedString::from(format!("dock-status-row-{key}")))
            .when(live, |d| {
                // drag a row onto another: it takes that row's place (the mark is under the row when it comes from above, over it otherwise)
                d.on_drag(RowDrag { which, ix: i, title: name(which).into() }, |d: &RowDrag, _, _, cx| cx.new(|_| d.clone()))
                    .drag_over::<RowDrag>(move |s, d: &RowDrag, _, _| if d.ix < i { s.border_b_2().border_color(accent) } else if d.ix > i { s.border_t_2().border_color(accent) } else { s })
                    .on_drop(move |d: &RowDrag, _, cx| {
                        let dragged = d.which;
                        settings::update(cx, move |s| s.dock_status_order = status_order::placed(&s.dock_status_order, dragged, i));
                    })
            })
            .h(px(44.))
            .pl(px(50.))
            .pr_4()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(p.border)
            .child(Icon::new(icon(which)).size(px(16.)).flex_shrink_0().text_color(if shown { p.text } else { p.text_secondary }))
            .child(div().flex_1().min_w_0().text_sm().text_color(if shown { p.text } else { p.text_secondary }).child(name(which)))
            .child(arrow(SharedString::from(format!("dock-status-up-{key}")), p, L::ArrowUp, t!("Yukarı taşı"), step(-1)))
            .child(arrow(SharedString::from(format!("dock-status-down-{key}")), p, L::ArrowDown, t!("Aşağı taşı"), step(1)))
            .child(switch(SharedString::from(format!("dock-status-shown-{key}")), p, shown, move |_, cx| {
                if live {
                    settings::update(cx, |s| s.dock_status_hidden = status_order::with_hidden(&s.dock_status_hidden, which, !shown));
                }
            }))
    });
    let inner = v_flex().child(card_body(p, L::ArrowUpDown, t!("Durum simgeleri").into(), Some(desc().into()), None)).children(rows);
    shell(p).child(if live { inner } else { inner.opacity(0.55) })
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the glob of the UI kit above carries its own `test` attribute)
    use gpui_kit::assets::IconName as L;
    use rf_win::dock::status_order::Status;

    use super::{icon, name};

    /// Every button has a name and a picture of its own (two rows that read the same could not be told apart).
    #[test]
    fn every_button_has_its_own_name_and_picture() {
        crate::dock_page::while_turkish(|| {
            let names: std::collections::HashSet<&str> = Status::ALL.iter().map(|s| name(*s)).collect();
            assert_eq!(names.len(), Status::ALL.len());
            assert!(names.iter().all(|n| !n.trim().is_empty()));
        });
        let icons: Vec<L> = Status::ALL.iter().map(|s| icon(*s)).collect();
        for (i, a) in icons.iter().enumerate() {
            assert!(icons[i + 1..].iter().all(|b| std::mem::discriminant(a) != std::mem::discriminant(b)));
        }
    }
}
