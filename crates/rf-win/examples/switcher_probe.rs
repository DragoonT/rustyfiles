//! What the window switcher would list on this machine: COUNTS only (no titles, no program names), and how long the listing takes.
//! Read-only and windowless: it creates no window of any kind (the virtual desktop question is asked on this thread, not on the
//! shell thread, which has a hidden window of its own).
//!
//!   cargo run --release -p rf-win --example switcher_probe

use std::time::Duration;

use rf_win::switcher::{self, ComRoute, Options};

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    // the first call is shown apart: page faults and COM start-up are not the cost of every opening
    let first = switcher::list(ComRoute::CallingThread, Options::default());
    let runs = 20;
    let mut last = switcher::list(ComRoute::CallingThread, Options::default());
    let (mut totals, mut facts, mut desktops, mut items) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..runs {
        last = switcher::list(ComRoute::CallingThread, Options::default());
        totals.push(last.took.total);
        facts.push(last.took.facts);
        desktops.push(last.took.desktops);
        items.push(last.took.items);
    }
    let with_other = switcher::list(ComRoute::CallingThread, Options { other_desktops: true });
    let c = last.counts;
    println!("top-level windows seen:         {}", c.windows_seen);
    println!("listed by default:              {}", c.listed);
    println!("  minimized:                    {}", c.minimized);
    println!("  suspended (kept):             {}", c.suspended);
    println!("cloaked, on another desktop:    {} (left out by default)", c.on_other_desktop);
    println!("cloaked, not minimized:         {} (left out: a frame with nothing behind it)", c.cloaked_not_minimized);
    println!("cloaked by their own app:       {} (left out)", c.cloaked_by_app);
    println!("cloaked, desktop unknown:       {} (left out)", c.desktop_unknown);
    println!("window in front is a listed one: {}", last.front_listed);
    println!("with other desktops included:   {}", with_other.counts.listed);
    println!("first call (cold):              {:?}", first.took.total);
    println!(
        "median of {runs} calls:            total {:?} = facts {:?} + desktops {:?} + items {:?}",
        median(totals),
        median(facts),
        median(desktops),
        median(items)
    );
    let distinct: std::collections::HashSet<&str> = last.items.iter().map(|i| i.group.as_str()).collect();
    println!("distinct apps among the listed: {}", distinct.len());
    let unknown_program = last.items.iter().filter(|i| i.exe.is_none() && i.app_id.is_none()).count();
    println!("listed windows with no readable program: {unknown_program}");
    // for comparison: what the dock's task rules (no owner walk, no suspended apps) list
    let dock_tasks = rf_win::tasklist::tasks();
    println!("the dock's task rules list:     {} ({} minimized)", dock_tasks.len(), dock_tasks.iter().filter(|t| t.minimized).count());
    let lay = switcher::layout::plan(last.items.len(), rf_win::dock::Rect::new(0, 0, 2560, 1392), 1.0);
    println!("layout for them on 2560 x 1440 at 100 %: {} columns x {} rows, cards at {:.0} % size, {} shown", lay.cols, lay.rows, lay.factor * 100.0, lay.visible);

    // the pure parts of an opening, over the real list: the session (scope, selection) and the layout, 1000 times
    let work = rf_win::dock::Rect::new(0, 0, 2560, 1392);
    let mut pure = Vec::new();
    for _ in 0..1000 {
        let t = std::time::Instant::now();
        let mut s = switcher::session::Session::open(last.items.clone(), switcher::chord::Scope::AllWindows, false);
        let l = switcher::layout::plan(s.items().len(), work, 1.0);
        s.fit(&l);
        let cards: Vec<_> = (0..s.items().len()).map(|i| l.picture(i)).collect();
        std::hint::black_box(&cards);
        pure.push(t.elapsed());
    }
    println!("session + layout + card rectangles for {} windows: median {:?} (1000 runs)", last.items.len(), median(pure));
}
