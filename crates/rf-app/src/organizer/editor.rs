//! The rule editor: name, folder, conditions (add / remove rows), ALL / ANY, the action with
//! its destination or template, "run automatically", and a live example of where a file of the
//! folder would go. Every control can be reached with Tab; Enter saves, Esc closes.

use std::path::PathBuf;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::organize::rule::parse_ext_list;
use rf_core::organize::summary::{SizeUnit, show_path};
use rf_core::organize::{Action, Cmp, Condition, Dest, FileFact, FileKind, Join, NameOp, ProtectedPaths, Rule, RuleProblem, TEMPLATE_VARS, TimeField};

use crate::select_box::{Choice, select_box_h};
use crate::t;
use crate::theme::{self, Palette};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Name,
    Extension,
    Kind,
    Size,
    Modified,
    Created,
}

impl Field {
    const ALL: [Field; 6] = [Field::Name, Field::Extension, Field::Kind, Field::Size, Field::Modified, Field::Created];

    fn label(self) -> &'static str {
        match self {
            Field::Name => t!("Ad"),
            Field::Extension => t!("Uzantı"),
            Field::Kind => t!("Tür"),
            Field::Size => t!("Boyut"),
            Field::Modified => t!("Değiştirme tarihi"),
            Field::Created => t!("Oluşturma tarihi"),
        }
    }
}

fn name_op_label(op: NameOp) -> &'static str {
    match op {
        NameOp::Contains => t!("şunu içerir"),
        NameOp::StartsWith => t!("şununla başlar"),
        NameOp::EndsWith => t!("şununla biter"),
        NameOp::Wildcard => t!("şu desene uyar (* ve ?)"),
    }
}

fn cmp_label(field: Field, cmp: Cmp) -> &'static str {
    match (field, cmp) {
        (Field::Size, Cmp::Greater) => t!("şundan büyük"),
        (Field::Size, Cmp::Less) => t!("şundan küçük"),
        (_, Cmp::Greater) => t!("daha eski"),
        (_, Cmp::Less) => t!("daha yeni"),
    }
}

fn kind_label(k: FileKind) -> String {
    rf_core::organize::summary::kind_word(k, &super::tr)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActKind {
    MoveTo,
    MoveToTemplate,
    Rename,
    Recycle,
}

impl ActKind {
    const ALL: [ActKind; 4] = [ActKind::MoveTo, ActKind::MoveToTemplate, ActKind::Rename, ActKind::Recycle];

    fn label(self) -> &'static str {
        match self {
            ActKind::MoveTo => t!("Klasöre taşı"),
            ActKind::MoveToTemplate => t!("Şablonla oluşturulan klasöre taşı"),
            ActKind::Rename => t!("Şablonla yeniden adlandır"),
            ActKind::Recycle => t!("Geri Dönüşüm Kutusu'na gönder"),
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            ActKind::MoveTo => t!("Hedef klasör, örneğin D:\\Resimler"),
            ActKind::MoveToTemplate => t!("Klasör şablonu, örneğin D:\\Resimler\\{year}\\{month}"),
            ActKind::Rename => t!("Yeni ad şablonu, örneğin {year}-{month}-{day} {name}.{ext}"),
            ActKind::Recycle => "",
        }
    }
}

struct CondRow {
    field: Field,
    name_op: NameOp,
    cmp: Cmp,
    /// The name text, the extensions ("jpg, png"), the size number or the number of days.
    text: Entity<InputState>,
    kinds: Vec<FileKind>,
    unit: SizeUnit,
}

pub struct EditorView {
    id: u32,
    enabled: bool,
    name: Entity<InputState>,
    folder: Option<PathBuf>,
    subfolders: bool,
    join: Join,
    conds: Vec<CondRow>,
    action: ActKind,
    dest: Entity<InputState>,
    auto: bool,
    error: Option<String>,
    /// The first ordinary files of the folder, for the live example.
    samples: Vec<FileFact>,
    /// A destination chosen in the folder dialog, put into the box at the next draw (the box
    /// needs the window).
    pending_dest: Option<String>,
    protected: ProtectedPaths,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

fn new_input(window: &mut Window, cx: &mut Context<EditorView>, placeholder: &str, value: &str) -> Entity<InputState> {
    let (ph, v) = (placeholder.to_string(), value.to_string());
    cx.new(|cx| InputState::new(window, cx).placeholder(ph).default_value(v))
}

fn parse_number(s: &str) -> Option<f64> {
    let n: f64 = s.trim().replace(',', ".").parse().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n)
}

/// A problem found in a rule, in words for the person editing it.
pub fn problem_text(p: &RuleProblem) -> String {
    let base = crate::i18n::td(p.text());
    match p {
        RuleProblem::UnknownVariable(v) => format!("{base}: {{{v}}}"),
        RuleProblem::EmptyCondition(i) => format!("{}. {}", i + 1, t!("koşul boş: değerini yazın ya da koşulu kaldırın")),
        _ => base,
    }
}

impl EditorView {
    pub fn new(rule: Option<Rule>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rule = rule.unwrap_or_else(|| super::blank_rule(PathBuf::new()));
        let name = new_input(window, cx, t!("Kuralın adı"), &rule.name);
        let (action, dest_text) = match &rule.action {
            Action::MoveTo { folder } => (ActKind::MoveTo, folder.to_string_lossy().into_owned()),
            Action::MoveToTemplate { template } => (ActKind::MoveToTemplate, template.clone()),
            Action::Rename { template } => (ActKind::Rename, template.clone()),
            Action::Recycle => (ActKind::Recycle, String::new()),
        };
        let dest = new_input(window, cx, action.placeholder(), &dest_text);
        let mut conds: Vec<CondRow> = rule.conditions.iter().map(|c| Self::row_of(c, window, cx)).collect();
        if conds.is_empty() && rule.id == 0 {
            // a new rule starts with one condition to fill in
            conds.push(Self::blank_row(Field::Extension, window, cx));
        }
        let mut subs = Vec::new();
        for input in std::iter::once(&name).chain(std::iter::once(&dest)).chain(conds.iter().map(|c| &c.text)) {
            subs.push(Self::watch(input, window, cx));
        }
        name.update(cx, |s, cx| s.focus(window, cx));
        let focus = cx.focus_handle();
        let mut me = Self {
            id: rule.id,
            enabled: rule.enabled,
            name,
            folder: (!rule.folder.as_os_str().is_empty()).then(|| rule.folder.clone()),
            subfolders: rule.subfolders,
            join: rule.join,
            conds,
            action,
            dest,
            auto: rule.auto,
            error: None,
            samples: Vec::new(),
            pending_dest: None,
            protected: super::protected(),
            focus,
            _subs: subs,
            _tasks: Vec::new(),
        };
        me.reload_sample(cx);
        me
    }

    /// Is a condition "older than N days"? Such a rule cannot run by itself (see `Rule::needs_manual_run`).
    fn needs_manual_run(&self) -> bool {
        self.conds.iter().any(|c| matches!(c.field, Field::Modified | Field::Created) && c.cmp == Cmp::Greater)
    }

    /// Typing redraws (the example follows); Enter saves.
    fn watch(input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                this.error = None;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.save(window, cx),
            _ => {}
        })
    }

    fn blank_row(field: Field, window: &mut Window, cx: &mut Context<Self>) -> CondRow {
        let placeholder = match field {
            Field::Name => t!("Aranacak metin"),
            Field::Extension => t!("jpg, png"),
            Field::Size => "5",
            Field::Modified | Field::Created => "30",
            Field::Kind => "",
        };
        CondRow { field, name_op: NameOp::Contains, cmp: Cmp::Greater, text: new_input(window, cx, placeholder, ""), kinds: Vec::new(), unit: SizeUnit::Mb }
    }

    fn row_of(c: &Condition, window: &mut Window, cx: &mut Context<Self>) -> CondRow {
        match c {
            Condition::Name { op, text } => {
                let mut r = Self::blank_row(Field::Name, window, cx);
                r.name_op = *op;
                r.text.update(cx, |s, cx| s.set_value(text.clone(), window, cx));
                r
            }
            Condition::Extension { list } => {
                let r = Self::blank_row(Field::Extension, window, cx);
                r.text.update(cx, |s, cx| s.set_value(list.join(", "), window, cx));
                r
            }
            Condition::Kind { kinds } => {
                let mut r = Self::blank_row(Field::Kind, window, cx);
                r.kinds = kinds.clone();
                r
            }
            Condition::Size { cmp, bytes } => {
                let mut r = Self::blank_row(Field::Size, window, cx);
                let (n, unit) = SizeUnit::split(*bytes);
                r.cmp = *cmp;
                r.unit = unit;
                r.text.update(cx, |s, cx| s.set_value(n.to_string(), window, cx));
                r
            }
            Condition::Age { field, cmp, days } => {
                let mut r = Self::blank_row(if *field == TimeField::Modified { Field::Modified } else { Field::Created }, window, cx);
                r.cmp = *cmp;
                r.text.update(cx, |s, cx| s.set_value(days.to_string(), window, cx));
                r
            }
        }
    }

    fn condition_of(&self, row: &CondRow, cx: &App) -> Result<Condition, String> {
        let text = row.text.read(cx).value().to_string();
        Ok(match row.field {
            Field::Name => Condition::Name { op: row.name_op, text },
            Field::Extension => Condition::Extension { list: parse_ext_list(&text) },
            Field::Kind => Condition::Kind { kinds: row.kinds.clone() },
            Field::Size => {
                let n = parse_number(&text).ok_or_else(|| t!("Boyut için bir sayı yazın").to_string())?;
                Condition::Size { cmp: row.cmp, bytes: (n * row.unit.bytes() as f64).round() as u64 }
            }
            Field::Modified | Field::Created => {
                let days: u32 = text.trim().parse().map_err(|_| t!("Gün sayısı için bir tam sayı yazın").to_string())?;
                Condition::Age { field: if row.field == Field::Modified { TimeField::Modified } else { TimeField::Created }, cmp: row.cmp, days }
            }
        })
    }

    /// The rule as filled in, or what is wrong with it.
    fn build(&self, cx: &App) -> Result<Rule, String> {
        let folder = self.folder.clone().ok_or_else(|| crate::i18n::td(RuleProblem::FolderNotAbsolute.text()))?;
        let mut conditions = Vec::new();
        for row in &self.conds {
            conditions.push(self.condition_of(row, cx)?);
        }
        let dest = self.dest.read(cx).value().trim().to_string();
        let action = match self.action {
            ActKind::MoveTo => Action::MoveTo { folder: PathBuf::from(dest) },
            ActKind::MoveToTemplate => Action::MoveToTemplate { template: dest },
            ActKind::Rename => Action::Rename { template: dest },
            ActKind::Recycle => Action::Recycle,
        };
        let rule = Rule {
            id: self.id,
            name: self.name.read(cx).value().trim().to_string(),
            enabled: self.enabled,
            folder,
            subfolders: self.subfolders,
            join: self.join,
            conditions,
            action,
            auto: self.auto,
        };
        rule.validate().map_err(|p| problem_text(&p))?;
        if let Some(why) = super::refusal(&self.protected, &rule.folder, rule.subfolders) {
            return Err(crate::i18n::td(why.text()));
        }
        // a fixed destination in a protected place (Windows, Program Files, AppData ...)
        if let Action::MoveTo { folder } = &rule.action
            && folder.is_absolute()
            && self.protected.refuse_dest(folder).is_some()
        {
            return Err(crate::i18n::td(rf_core::organize::SkipReason::ProtectedDest.text()));
        }
        Ok(rule)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.build(cx) {
            Ok(rule) => {
                super::update(cx, |f| {
                    f.put(rule);
                });
                super::close(window, cx);
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    /// The first ordinary files of the folder for the live example, looked for off the UI thread.
    fn reload_sample(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.folder.clone() else {
            self.samples.clear();
            return;
        };
        let work = cx.background_spawn(async move { first_files(&folder) });
        let t = cx.spawn(async move |this, cx| {
            let samples = work.await;
            let _ = this.update(cx, |v, cx| {
                v.samples = samples;
                cx.notify();
            });
        });
        self._tasks.push(t);
    }

    fn pick_folder(&mut self, for_dest: bool, cx: &mut Context<Self>) {
        let (tx, rx) = futures::channel::oneshot::channel::<Option<PathBuf>>();
        let title = if for_dest { t!("Hedef klasörü seçin") } else { t!("İzlenecek klasörü seçin") };
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::dialogs::pick(owner, title, rf_win::dialogs::Pick::Folder));
        });
        let t = cx.spawn(async move |this, cx| {
            let Ok(Some(path)) = rx.await else { return };
            let _ = this.update(cx, |v, cx| {
                if for_dest {
                    v.pending_dest = Some(path.to_string_lossy().into_owned());
                } else {
                    v.folder = Some(path);
                    v.error = None;
                    v.reload_sample(cx);
                }
                cx.notify();
            });
        });
        self._tasks.push(t);
    }

    fn add_condition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = Self::blank_row(Field::Extension, window, cx);
        let sub = Self::watch(&row.text, window, cx);
        self._subs.push(sub);
        row.text.update(cx, |s, cx| s.focus(window, cx));
        self.conds.push(row);
        cx.notify();
    }

    /// The live example: a file of the folder that the rule would take (else the first file, else a
    /// made-up one), where it would go, and whether the rule would take its own result again.
    fn example(&self, cx: &App) -> Option<Example> {
        let built = self.build(cx).ok();
        let rule = built.clone().or_else(|| {
            // an unfinished rule still shows its destination
            let mut r = self.build_loose(cx)?;
            r.name = "x".into();
            Some(r)
        })?;
        let now = super::now_ms();
        // only a finished rule has conditions that can be told apart: the loose one has none
        let matching = if built.is_some() { self.samples.iter().find(|f| rule.matches(f, now)) } else { self.samples.first() };
        let (sample, label) = match matching {
            Some(f) => (f.clone(), t!("Örnek:")),
            None => match self.samples.first() {
                Some(f) => (f.clone(), t!("Örnek (koşullardan bağımsız):")),
                None => (FileFact::new(rule.folder.join("örnek.jpg"), 2 << 20, now, now, FileKind::Image), t!("Örnek (varsayımsal dosya):")),
            },
        };
        let text = if matches!(rule.action, Action::Recycle) {
            format!("{} → {}", sample.name, t!("Geri Dönüşüm Kutusu"))
        } else {
            let exists = |_: &std::path::Path| false;
            let mut ctx = rf_core::organize::PlanContext::new(now, super::classify, &exists, &self.protected);
            ctx.kind_labels = super::kind_labels();
            let home = super::home();
            match rf_core::organize::preview_dest(&rule, &sample, &ctx) {
                Ok(Dest::Move(d)) => format!("{} → {}", sample.name, show_path(&d, home.as_deref())),
                Ok(Dest::Recycle) => format!("{} → {}", sample.name, t!("Geri Dönüşüm Kutusu")),
                Err(why) => format!("{}: {}", sample.name, crate::i18n::td(why.text())),
            }
        };
        // would the rule take what it made again? (the engine leaves such a rule out of every run)
        let loops = built.is_some_and(|r| {
            let exists = |_: &std::path::Path| false;
            let ctx = rf_core::organize::PlanContext::new(now, super::classify, &exists, &self.protected);
            !rf_core::organize::plan(std::slice::from_ref(&r), std::slice::from_ref(&sample), &ctx).loops.is_empty()
        });
        Some(Example { label, text, loops })
    }

    /// A rule for the example only: conditions that do not parse are left out, a missing name is fine.
    fn build_loose(&self, cx: &App) -> Option<Rule> {
        let folder = self.folder.clone()?;
        let dest = self.dest.read(cx).value().trim().to_string();
        if dest.is_empty() && self.action != ActKind::Recycle {
            return None;
        }
        let action = match self.action {
            ActKind::MoveTo => Action::MoveTo { folder: PathBuf::from(dest) },
            ActKind::MoveToTemplate => Action::MoveToTemplate { template: dest },
            ActKind::Rename => Action::Rename { template: dest },
            ActKind::Recycle => Action::Recycle,
        };
        Some(Rule { folder, action, ..Rule::default() })
    }
}

/// What the rule editor shows under the destination.
struct Example {
    label: &'static str,
    text: String,
    /// The rule would match its own result again.
    loops: bool,
}

/// The first ordinary files of a folder (a look at the first few hundred entries only).
fn first_files(dir: &std::path::Path) -> Vec<FileFact> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten().take(400) {
        let name = e.file_name().to_string_lossy().into_owned();
        if rf_core::organize::is_partial_download(&name) || !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        out.extend(rf_win::organize::facts_for_names(dir, std::slice::from_ref(&name), super::classify).into_iter().filter(|f| !f.hidden));
        if out.len() >= 60 {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------- drawing

fn chip(id: impl Into<ElementId>, label: SharedString, on: bool, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .min_h(px(28.))
        .flex()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .text_sm()
        .cursor_pointer()
        .tab_index(0)
        .when(on, |d| d.bg(p.card_bg).text_color(p.text).shadow_sm().font_weight(FontWeight::MEDIUM))
        .when(!on, |d| d.text_color(p.text_secondary).hover(|s| s.text_color(p.text).bg(p.pressed)))
        .child(label)
}

fn checkbox(id: &'static str, label: SharedString, on: bool, p: &Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap_2()
        .items_center()
        .min_h(px(28.))
        .cursor_pointer()
        .text_sm()
        .tab_index(0)
        .child(
            div()
                .size(px(16.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(if on { p.accent } else { p.text_tertiary })
                .when(on, |d| d.bg(p.accent))
                .when(on, |d| d.child(Icon::new(L::Check).size(px(12.)).text_color(p.accent_text))),
        )
        .child(label)
}

fn button(id: &'static str, label: &'static str, primary: bool, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .text_sm()
        .cursor_pointer()
        .tab_index(0)
        .when(primary, |d| d.bg(p.accent).text_color(p.accent_text))
        .when(!primary, |d| d.bg(p.hover).border_1().border_color(p.border))
        .hover(|s| s.opacity(0.85))
        .child(label)
}

fn input_box(input: &Entity<InputState>, p: &Palette) -> Div {
    div().h(px(32.)).px_2().flex().items_center().rounded(theme::RADIUS_SM).bg(p.content_bg).border_1().border_color(p.border).child(Input::new(input).appearance(false))
}

fn label(p: &Palette, text: &'static str) -> Div {
    div().text_xs().text_color(p.text_secondary).child(text)
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let me = cx.entity();
        if let Some(v) = self.pending_dest.take() {
            self.dest.update(cx, |s, cx| s.set_value(v, window, cx));
        }

        // --- folder
        let refusal = self.folder.as_ref().and_then(|f| super::refusal(&self.protected, f, self.subfolders));
        let folder_row = h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .bg(p.content_bg)
                    .border_1()
                    .border_color(if refusal.is_some() { p.danger } else { p.border })
                    .text_sm()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .when(self.folder.is_none(), |d| d.text_color(p.text_tertiary))
                    .child(self.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_else(|| t!("Bir klasör seçin").to_string())),
            )
            .child(button("ed-folder", t!("Seç…"), false, &p).on_click(cx.listener(|this, _, _, cx| this.pick_folder(false, cx))));

        // --- conditions
        let mut cond_rows = v_flex().gap_2();
        for (i, row) in self.conds.iter().enumerate() {
            let field = row.field;
            let me1 = me.clone();
            let field_box = select_box_h(SharedString::from(format!("ed-field-{i}")), &p, field.label(), 32., move |_| {
                Field::ALL
                    .iter()
                    .map(|&f| {
                        let me2 = me1.clone();
                        Choice::new(f.label(), f == field, move |window, cx| {
                            me2.update(cx, |v, cx| {
                                if let Some(row) = v.conds.get_mut(i)
                                    && row.field != f
                                {
                                    row.field = f;
                                    row.cmp = Cmp::Greater;
                                    row.kinds.clear();
                                    row.text.update(cx, |s, cx| s.set_value("", window, cx));
                                }
                                cx.notify();
                            })
                        })
                    })
                    .collect()
            });
            let me1 = me.clone();
            let op_box: AnyElement = match field {
                Field::Name => {
                    let cur = row.name_op;
                    select_box_h(SharedString::from(format!("ed-op-{i}")), &p, name_op_label(cur), 32., move |_| {
                        [NameOp::Contains, NameOp::StartsWith, NameOp::EndsWith, NameOp::Wildcard]
                            .iter()
                            .map(|&op| {
                                let me2 = me1.clone();
                                Choice::new(name_op_label(op), op == cur, move |_, cx| {
                                    me2.update(cx, |v, cx| {
                                        if let Some(row) = v.conds.get_mut(i) {
                                            row.name_op = op;
                                        }
                                        cx.notify();
                                    })
                                })
                            })
                            .collect()
                    })
                }
                Field::Size | Field::Modified | Field::Created => {
                    let cur = row.cmp;
                    select_box_h(SharedString::from(format!("ed-op-{i}")), &p, cmp_label(field, cur), 32., move |_| {
                        [Cmp::Greater, Cmp::Less]
                            .iter()
                            .map(|&c| {
                                let me2 = me1.clone();
                                Choice::new(cmp_label(field, c), c == cur, move |_, cx| {
                                    me2.update(cx, |v, cx| {
                                        if let Some(row) = v.conds.get_mut(i) {
                                            row.cmp = c;
                                        }
                                        cx.notify();
                                    })
                                })
                            })
                            .collect()
                    })
                }
                Field::Extension | Field::Kind => div().min_w(px(180.)).text_sm().text_color(p.text_secondary).child(t!("şunlardan biri")).into_any_element(),
            };
            let value: AnyElement = match field {
                Field::Kind => {
                    let mut chips = h_flex().flex_wrap().gap_1().flex_1().min_w_0();
                    for (ki, k) in FileKind::ALL.iter().copied().enumerate() {
                        let on = row.kinds.contains(&k);
                        chips = chips.child(chip(SharedString::from(format!("ed-kind-{i}-{ki}")), kind_label(k).into(), on, &p).when(on, |d| d.border_1().border_color(p.accent)).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Some(row) = this.conds.get_mut(i) {
                                    match row.kinds.iter().position(|x| *x == k) {
                                        Some(at) => {
                                            row.kinds.remove(at);
                                        }
                                        None => row.kinds.push(k),
                                    }
                                }
                                cx.notify();
                            },
                        )));
                    }
                    chips.into_any_element()
                }
                Field::Size => {
                    let me1 = me.clone();
                    let unit = row.unit;
                    h_flex()
                        .gap_2()
                        .flex_1()
                        .min_w_0()
                        .child(div().flex_1().min_w_0().child(input_box(&row.text, &p)))
                        .child(div().w(px(90.)).child(select_box_h(SharedString::from(format!("ed-unit-{i}")), &p, unit.label(), 32., move |_| {
                            SizeUnit::ALL
                                .iter()
                                .map(|&u| {
                                    let me2 = me1.clone();
                                    Choice::new(u.label(), u == unit, move |_, cx| {
                                        me2.update(cx, |v, cx| {
                                            if let Some(row) = v.conds.get_mut(i) {
                                                row.unit = u;
                                            }
                                            cx.notify();
                                        })
                                    })
                                })
                                .collect()
                        })))
                        .into_any_element()
                }
                Field::Modified | Field::Created => h_flex()
                    .gap_2()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .child(div().flex_1().min_w_0().child(input_box(&row.text, &p)))
                    .child(div().text_sm().text_color(p.text_secondary).child(t!("günden")))
                    .into_any_element(),
                Field::Name | Field::Extension => div().flex_1().min_w_0().child(input_box(&row.text, &p)).into_any_element(),
            };
            let is_age = matches!(field, Field::Modified | Field::Created);
            // (Turkish puts the number first: "30 günden daha eski")
            let mut line = h_flex().gap_2().items_start().child(div().w(px(150.)).flex_shrink_0().child(field_box));
            line = if is_age { line.child(value).child(div().flex_shrink_0().child(op_box)) } else { line.child(div().flex_shrink_0().child(op_box)).child(value) };
            cond_rows = cond_rows.child(
                line
                    .child(
                        div()
                            .id(("ed-del", i))
                            .size(px(32.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::RADIUS_SM)
                            .cursor_pointer()
                            .tab_index(0)
                            .hover(|s| s.bg(p.hover))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Koşulu kaldır")).build(w, cx))
                            .child(Icon::new(L::X).size(px(14.)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if i < this.conds.len() {
                                    this.conds.remove(i);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        let join_seg = h_flex().p_0p5().gap_0p5().rounded(theme::RADIUS).bg(p.hover).child(
            chip("ed-join-all", t!("Koşulların hepsi uysun").into(), self.join == Join::All, &p).on_click(cx.listener(|this, _, _, cx| {
                this.join = Join::All;
                cx.notify();
            })),
        ).child(
            chip("ed-join-any", t!("Koşullardan biri uysun").into(), self.join == Join::Any, &p).on_click(cx.listener(|this, _, _, cx| {
                this.join = Join::Any;
                cx.notify();
            })),
        );

        // --- action
        let me1 = me.clone();
        let cur_action = self.action;
        let action_box = select_box_h("ed-action", &p, cur_action.label(), 32., move |_| {
            ActKind::ALL
                .iter()
                .map(|&a| {
                    let me2 = me1.clone();
                    Choice::new(a.label(), a == cur_action, move |window, cx| {
                        me2.update(cx, |v, cx| {
                            if v.action != a {
                                v.action = a;
                                v.dest.update(cx, |s, cx| {
                                    s.set_value("", window, cx);
                                    s.set_placeholder(a.placeholder(), window, cx);
                                });
                            }
                            cx.notify();
                        })
                    })
                })
                .collect()
        });
        let dest_row: Option<AnyElement> = (self.action != ActKind::Recycle).then(|| {
            let mut r = h_flex().gap_2().items_center().child(div().flex_1().min_w_0().child(input_box(&self.dest, &p)));
            if self.action == ActKind::MoveTo || self.action == ActKind::MoveToTemplate {
                r = r.child(button("ed-dest-pick", t!("Seç…"), false, &p).on_click(cx.listener(|this, _, _, cx| this.pick_folder(true, cx))));
            }
            r.into_any_element()
        });
        let vars: Option<AnyElement> = matches!(self.action, ActKind::MoveToTemplate | ActKind::Rename).then(|| {
            let mut r = h_flex().flex_wrap().gap_1().items_center().child(div().text_xs().text_color(p.text_secondary).child(t!("Değişkenler (tıklayınca eklenir):")));
            for (vi, v) in TEMPLATE_VARS.iter().enumerate() {
                let token = format!("{{{v}}}");
                let t2 = token.clone();
                r = r.child(
                    div()
                        .id(("ed-var", vi))
                        .px_2()
                        .min_h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(p.border)
                        .text_xs()
                        .cursor_pointer()
                        .tab_index(0)
                        .hover(|s| s.bg(p.hover))
                        .child(token)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let cur = this.dest.read(cx).value().to_string();
                            this.dest.update(cx, |s, cx| s.set_value(format!("{cur}{t2}"), window, cx));
                            cx.notify();
                        })),
                );
            }
            r.into_any_element()
        });
        let var_help = matches!(self.action, ActKind::MoveToTemplate | ActKind::Rename).then(|| {
            div().text_xs().text_color(p.text_tertiary).child(t!("{name} dosya adı (uzantısız) · {ext} uzantı · {kind} tür klasörü · {year} {month} {day} değiştirilme tarihinden"))
        });
        let example = self.example(cx);

        // --- the form
        let mut form = v_flex().gap_4().px_6().py_4();
        form = form.child(v_flex().gap_1().child(label(&p, t!("Kuralın adı"))).child(input_box(&self.name, &p)));
        form = form.child(
            v_flex()
                .gap_1()
                .child(label(&p, t!("İzlenecek klasör")))
                .child(folder_row)
                .children(refusal.map(|r| div().text_xs().text_color(p.danger).child(crate::i18n::td(r.text()))))
                .child(checkbox("ed-sub", t!("Alt klasörlerdeki dosyalar da").into(), self.subfolders, &p).on_click(cx.listener(|this, _, _, cx| {
                    this.subfolders = !this.subfolders;
                    cx.notify();
                }))),
        );
        form = form.child(
            v_flex()
                .gap_2()
                .child(h_flex().gap_3().items_center().child(label(&p, t!("Koşullar")).flex_1()).child(join_seg))
                .child(cond_rows)
                .when(self.conds.is_empty(), |d| d.child(div().text_xs().text_color(p.text_tertiary).child(t!("Koşul yok: klasördeki tüm dosyalar kurala uyar."))))
                .child(
                    h_flex().child(
                        button("ed-add", t!("Koşul ekle"), false, &p).on_click(cx.listener(|this, _, window, cx| this.add_condition(window, cx))),
                    ),
                ),
        );
        form = form.child(
            v_flex()
                .gap_2()
                .child(label(&p, t!("Ne yapılsın")))
                .child(h_flex().child(div().min_w(px(280.)).child(action_box)))
                .children(dest_row)
                .children(vars)
                .children(var_help)
                .children(example.map(|e| {
                    v_flex()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded(theme::RADIUS_SM)
                        .bg(p.hover)
                        .text_sm()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(L::Sparkles).size(px(14.)).flex_shrink_0().text_color(p.accent))
                                .child(div().flex_shrink_0().text_color(p.text_secondary).child(e.label))
                                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(e.text)),
                        )
                        .when(e.loops, |d| {
                            d.child(div().text_xs().text_color(p.danger).child(t!("Bu kural kendi sonucunu yeniden yakalıyor, bu yüzden hiçbir çalıştırmada kullanılmaz. Koşulu, yeni ad artık uymayacak şekilde daraltın (örneğin ad \"IMG\" ile başlasın) ya da dosyaları başka bir klasöre gönderin.")))
                        })
                })),
        );
        let master_on = super::state(cx).file.auto_enabled;
        form = form.child(
            v_flex()
                .gap_1()
                .child(checkbox("ed-auto", t!("Dosya geldiğinde kendiliğinden çalıştır").into(), self.auto, &p).on_click(cx.listener(|this, _, _, cx| {
                    this.auto = !this.auto;
                    cx.notify();
                })))
                .when(self.auto, |d| d.child(div().text_xs().text_color(p.text_secondary).pl(px(24.)).child(t!("Kendiliğinden çalışırken önizleme gösterilmez; her çalıştırma Durum Merkezi'ndeki satırından geri alınabilir."))))
                .when(self.auto && matches!(self.action, ActKind::Recycle), |d| {
                    d.child(div().text_xs().text_color(p.danger).pl(px(24.)).child(t!("Dikkat: bu kural uyan dosyaları sormadan Geri Dönüşüm Kutusu'na gönderecek.")))
                })
                .when(self.auto && self.needs_manual_run(), |d| {
                    d.child(div().text_xs().text_color(p.danger).pl(px(24.)).child(t!("\"Daha eski\" yaş koşulu olan bir kural kendiliğinden çalışmaz (az önce kopyalanan ya da çıkarılan dosyaların tarihi eski olabilir); yalnızca elle çalışır.")))
                })
                .when(self.auto && !master_on, |d| {
                    d.child(div().text_xs().text_color(p.text_secondary).pl(px(24.)).child(t!("Not: Ayarlar'daki otomatik düzenleme şu an kapalı; açılana kadar bu kural yalnızca elle çalışır.")))
                }),
        );

        let header = h_flex()
            .px_6()
            .py_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(p.divider)
            .child(Icon::new(L::WandSparkles).size(px(18.)).text_color(p.text_secondary))
            .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(if self.id == 0 { t!("Yeni kural") } else { t!("Kuralı düzenle") }));
        let footer = h_flex()
            .px_6()
            .py_3()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(p.divider)
            .child(div().flex_1().min_w_0().text_sm().text_color(p.danger).child(self.error.clone().unwrap_or_default()))
            .child(button("ed-cancel", t!("İptal"), false, &p).on_click(cx.listener(|_, _, window, cx| super::close(window, cx))))
            .child(button("ed-save", t!("Kaydet"), true, &p).on_click(cx.listener(|this, _, window, cx| this.save(window, cx))));

        div()
            .id("organizer-editor")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                "escape" => {
                    super::close(window, cx);
                    cx.stop_propagation();
                }
                "enter" if this.focus.is_focused(window) => {
                    this.save(window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }))
            .child(
                v_flex()
                    .w(px(780.))
                    .max_w(relative(0.95))
                    .max_h(relative(0.92))
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_color(p.text)
                    .child(header)
                    .child(div().id("ed-scroll").flex_1().min_h_0().overflow_y_scroll().child(form))
                    .child(footer)
                    .with_animation("organizer-editor-in", Animation::new(std::time::Duration::from_millis(160)).with_easing(ease_out_quint()), |el, t| el.opacity(t).mt(px(10. * (1. - t)))),
            )
    }
}
