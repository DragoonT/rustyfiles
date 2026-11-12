//! The pure parts of showing a running file operation where the user looks: the speed and the
//! time left, which operation the status bar's strip shows and what gives way when the window
//! is narrow, the life of the floating cards, and the Status Center button's progress ring.
//! Nothing here touches a window or the clock: times come in as seconds / milliseconds so
//! every rule can be tested with a made-up moment.

use std::collections::VecDeque;

// ---------------------------------------------------------------------------------------------
// Speed and time left
// ---------------------------------------------------------------------------------------------

/// How far back the average looks. Long enough that one fast burst (a small file landing in
/// the disk cache) or one slow stretch (a folder of tiny files) does not make the number jump,
/// short enough that it follows a real change within a few seconds.
pub const SPEED_WINDOW_SECS: f64 = 4.0;

/// A rate is not given before the samples span this long: two reports 20 ms apart say nothing.
const MIN_SPAN_SECS: f64 = 0.75;

/// With no new report for this long the operation has stalled and the rate is 0.
const STALL_SECS: f64 = 1.5;

/// A time left past this is not an estimate worth showing.
const MAX_ETA_SECS: f64 = 100.0 * 3600.0;

/// A moving average of how fast a counter (bytes, or the engine's own units) grows.
#[derive(Debug, Clone, Default)]
pub struct SpeedMeter {
    /// (time in seconds since the operation began, counter), oldest first.
    samples: VecDeque<(f64, u64)>,
    last_done: u64,
    paused: bool,
}

impl SpeedMeter {
    /// A progress report. A counter that goes backwards (a new phase starts counting again)
    /// starts the average over. Times that go backwards are ignored.
    pub fn push(&mut self, t: f64, done: u64) {
        if self.paused {
            return;
        }
        if let Some(&(lt, ld)) = self.samples.back() {
            if t < lt {
                return;
            }
            if done < ld {
                self.samples.clear();
            }
        }
        self.last_done = done;
        self.samples.push_back((t, done));
        // keep exactly one sample at or before the window's start: it is the average's base
        let cutoff = t - SPEED_WINDOW_SECS;
        while self.samples.len() >= 2 && self.samples[1].0 <= cutoff {
            self.samples.pop_front();
        }
    }

    /// The operation waits: no rate, and the waiting time must not drag the average down later.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Carries on at `t`: the average starts over from the counter as it was.
    pub fn resume(&mut self, t: f64) {
        if self.paused {
            self.paused = false;
            self.samples.clear();
            self.samples.push_back((t, self.last_done));
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Units per second over the last [`SPEED_WINDOW_SECS`] ending at `now`. `None` while paused or
    /// while there is too little to go on; `Some(0.0)` once reports have stopped coming.
    pub fn rate(&self, now: f64) -> Option<f64> {
        if self.paused {
            return None;
        }
        let &(end_t, end_d) = self.samples.back()?;
        let cutoff = now - SPEED_WINDOW_SECS;
        let at = self.samples.partition_point(|s| s.0 <= cutoff);
        let (base_t, base_d) = if at == 0 {
            // the whole run so far is inside the window
            *self.samples.front()?
        } else {
            // the counter as it was at the window's start: between two close reports it is
            // interpolated; across a silence it stayed where it was (the next report is what
            // moved it, and that movement belongs to the window it arrived in)
            let (t0, d0) = self.samples[at - 1];
            let d = match self.samples.get(at) {
                Some(&(t1, d1)) if t1 - t0 <= STALL_SECS => d0 + (d1.saturating_sub(d0) as f64 * (cutoff - t0) / (t1 - t0)) as u64,
                _ => d0,
            };
            (cutoff, d)
        };
        if now - end_t >= STALL_SECS {
            // nothing came for a while: the average runs out to 0 as the old samples leave the window
            let span = (now - base_t).max(MIN_SPAN_SECS);
            return Some(if base_t >= end_t { 0.0 } else { end_d.saturating_sub(base_d) as f64 / span });
        }
        let span = now.max(end_t) - base_t;
        if end_t <= base_t || span < MIN_SPAN_SECS {
            return None;
        }
        Some(end_d.saturating_sub(base_d) as f64 / span)
    }
}

/// Seconds left at `rate` units a second with `remaining` units to go. `None` when it cannot be
/// said: nothing left, no (or a zero) rate, or an absurdly long time.
pub fn eta_secs(remaining: u64, rate: Option<f64>) -> Option<f64> {
    let rate = rate?;
    if remaining == 0 || rate.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || !rate.is_finite() {
        return None;
    }
    let s = remaining as f64 / rate;
    (s.is_finite() && s <= MAX_ETA_SECS).then_some(s)
}

/// One bar of the history is this long.
const BUCKET_SECS: f64 = 2.0;
/// Bars kept: the last minute.
pub const HISTORY_BARS: usize = 30;

/// The speed of the last minute in bars of two seconds, newest last, for the card's small
/// chart. Plain numbers: the view draws them with boxes, not paths.
#[derive(Debug, Clone, Default)]
pub struct SpeedHistory {
    /// Bytes in each finished-or-running bucket, oldest first; the last one is the running bucket.
    buckets: VecDeque<u64>,
    /// Index of the running bucket (time / BUCKET_SECS).
    at: u64,
    last: Option<u64>,
}

impl SpeedHistory {
    pub fn push(&mut self, t: f64, done: u64) {
        let ix = (t.max(0.0) / BUCKET_SECS) as u64;
        let delta = match self.last {
            Some(l) if done >= l => done - l,
            _ => 0,
        };
        self.last = Some(done);
        if self.buckets.is_empty() {
            self.buckets.push_back(0);
            self.at = ix;
        }
        while self.at < ix {
            // a stretch with no report is a stretch of 0
            self.buckets.push_back(0);
            self.at += 1;
            if self.buckets.len() > HISTORY_BARS {
                self.buckets.pop_front();
            }
        }
        if let Some(b) = self.buckets.back_mut() {
            *b += delta;
        }
    }

    /// Bytes a second in each bar, oldest first; the running bar is measured over the time it has
    /// had so far (at least half a second), so it does not look empty just because it is young.
    pub fn bars(&self, now: f64) -> Vec<f64> {
        let n = self.buckets.len();
        self.buckets
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                let secs = if i + 1 == n { (now - self.at as f64 * BUCKET_SECS).clamp(0.5, BUCKET_SECS) } else { BUCKET_SECS };
                b as f64 / secs
            })
            .collect()
    }
}

/// What a card shows of an operation's pace: bars from 0 to 1, the fastest at 1.
pub fn normalized(bars: &[f64]) -> Vec<f32> {
    let peak = bars.iter().copied().fold(0.0_f64, f64::max);
    if peak <= 0.0 {
        return vec![0.0; bars.len()];
    }
    bars.iter().map(|&b| (b / peak) as f32).collect()
}

/// Everything a view prints of one operation's pace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pace {
    /// Bytes a second, when the engine counts bytes.
    pub speed: Option<f64>,
    pub eta_secs: Option<f64>,
}

/// The counters of one running operation and what they say.
#[derive(Debug, Clone, Default)]
pub struct OpStats {
    meter: SpeedMeter,
    history: SpeedHistory,
    /// The meter counts bytes (otherwise the engine's units: a percentage only).
    on_bytes: Option<bool>,
}

impl OpStats {
    /// A progress report: the engine's units `(done, total)` and, when it counts them, the bytes
    /// `(done, total)` with `total == 0` meaning it does not.
    pub fn update(&mut self, t: f64, units: (u32, u32), bytes: (u64, u64)) {
        let use_bytes = bytes.1 > 0;
        if self.on_bytes.is_some_and(|b| b != use_bytes) {
            // the engine changed under us (our engine handed leftovers to the shell's): two scales must not mix
            self.meter = SpeedMeter { paused: self.meter.paused, ..Default::default() };
        }
        self.on_bytes = Some(use_bytes);
        let done = if use_bytes { bytes.0 } else { u64::from(units.0) };
        self.meter.push(t, done);
        if use_bytes {
            self.history.push(t, bytes.0);
        }
    }

    pub fn pause(&mut self) {
        self.meter.pause();
    }

    pub fn resume(&mut self, t: f64) {
        self.meter.resume(t);
    }

    /// The speed (only for bytes) and the time left, from the average at `now`.
    pub fn pace(&self, now: f64, units: (u32, u32), bytes: (u64, u64)) -> Pace {
        let use_bytes = self.on_bytes == Some(true);
        let rate = self.meter.rate(now);
        let remaining = if use_bytes { bytes.1.saturating_sub(bytes.0) } else { u64::from(units.1.saturating_sub(units.0)) };
        // no total (git, network) = no time left
        let known = if use_bytes { bytes.1 > 0 } else { units.1 > 0 };
        Pace { speed: if use_bytes { rate } else { None }, eta_secs: if known { eta_secs(remaining, rate) } else { None } }
    }

    pub fn bars(&self, now: f64) -> Vec<f32> {
        normalized(&self.history.bars(now))
    }
}

// ---------------------------------------------------------------------------------------------
// The status bar's strip
// ---------------------------------------------------------------------------------------------

/// What the strip needs to know of each operation to pick one; oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brief {
    pub running: bool,
    /// It has run long enough to be worth showing ([`CARD_APPEAR_MS`]): a delete of one file is
    /// over before anyone could read a strip, and the status bar must not jump for it.
    pub noticed: bool,
    /// Waiting for an answer about names that already exist.
    pub asking: bool,
    pub failed: bool,
    /// A failure the user has looked at or sent away.
    pub dismissed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strip {
    Nothing,
    /// A running operation (the one with a question first, otherwise the newest) and how many
    /// others run besides it.
    Running { index: usize, others: usize },
    /// A failure nobody has looked at yet, while nothing runs.
    Failed { index: usize },
}

pub fn strip_pick(ops: &[Brief]) -> Strip {
    // a question shows at once (the copy is stopped on it); the rest wait to be noticed
    let shown = |o: &Brief| o.running && (o.noticed || o.asking);
    let running = ops.iter().filter(|o| shown(o)).count();
    // a question cannot wait behind a newer operation: the copy is stopped until it is answered
    let pick = ops.iter().rposition(|o| shown(o) && o.asking).or_else(|| ops.iter().rposition(shown));
    if let Some(index) = pick {
        return Strip::Running { index, others: running - 1 };
    }
    match ops.iter().rposition(|o| o.failed && !o.dismissed) {
        Some(index) => Strip::Failed { index },
        None => Strip::Nothing,
    }
}

/// What the strip shows at a width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripFit {
    pub buttons: bool,
    pub chip: bool,
    pub text: bool,
    pub speed: bool,
    pub eta: bool,
}

/// The least the strip is: the bar and the percent.
const BASE_W: f32 = 96.0 + 8.0 + 36.0;
const BUTTONS_W: f32 = 56.0;
const CHIP_W: f32 = 34.0;
const TEXT_W: f32 = 150.0;
const SPEED_W: f32 = 72.0;
const ETA_W: f32 = 84.0;
const GAP_W: f32 = 10.0;

/// What fits in `avail` pixels. It gives way from the end: the time left first, then the speed,
/// then the text; the bar and the percent stay, and the buttons and the "+n" chip are the last
/// optional things to go because they are what the user acts with. A piece that does not fit
/// takes everything after it with it, so the strip never has a gap in the middle.
pub fn strip_fit(avail: f32, others: usize) -> StripFit {
    let mut used = BASE_W;
    let mut take = |w: f32, still: &mut bool| -> bool {
        if *still && used + GAP_W + w <= avail {
            used += GAP_W + w;
            true
        } else {
            *still = false;
            false
        }
    };
    let mut still = true;
    let buttons = take(BUTTONS_W, &mut still);
    let chip = others > 0 && take(CHIP_W, &mut still);
    let text = take(TEXT_W, &mut still);
    let speed = take(SPEED_W, &mut still);
    let eta = take(ETA_W, &mut still);
    StripFit { buttons, chip, text, speed, eta }
}

/// What else the status bar holds besides the strip, so the room left for the strip can be
/// worked out without measuring a laid-out window (an estimate of widths, not a measurement:
/// the strip also clips itself, so a wrong guess never makes it overlap its neighbours).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BarContents {
    /// "12 345 öğe".
    pub items: bool,
    /// "3 öğe seçili · 12,3 MB".
    pub selected: bool,
    /// "Yükleniyor…" or a property being read.
    pub busy: bool,
    /// The git controls (changes, sync, branch).
    pub git: bool,
    /// The "open in editor" button.
    pub ide: bool,
    /// "Güncelleme hazır".
    pub update: bool,
}

/// The status bar's own padding (12 px each side) and the strip's margins (12 px each side).
const BAR_FRAME_W: f32 = 48.0;

/// Pixels the strip can have in a status bar `bar_w` wide (the content panel, not the window:
/// the sidebar is not part of it).
pub fn strip_room(bar_w: f32, c: BarContents) -> f32 {
    let mut used = BAR_FRAME_W;
    for (on, w) in [(c.items, 80.0), (c.selected, 170.0), (c.busy, 90.0), (c.git, 250.0), (c.ide, 64.0), (c.update, 140.0)] {
        if on {
            used += w + GAP_W;
        }
    }
    (bar_w - used).max(0.0)
}

// ---------------------------------------------------------------------------------------------
// The floating card
// ---------------------------------------------------------------------------------------------

/// A card appears this long after an operation starts, so a short one never flashes it.
pub const CARD_APPEAR_MS: u64 = 600;
/// A finished copy's card stays this long (with "open the destination" and "undo").
pub const CARD_DONE_MS: u64 = 4000;
/// A cancelled one's, shorter: nothing is left to act on.
pub const CARD_CANCELLED_MS: u64 = 2000;
/// Cards on screen at most; the rest are a "+n".
pub const MAX_CARDS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Cancelled,
    Failed,
}

/// What a card needs to know of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardIn {
    /// `None` while it runs.
    pub outcome: Option<Outcome>,
    /// Milliseconds since it started (while it runs), or how long it ran (once it ended).
    pub ran_ms: u64,
    /// Milliseconds since it ended (0 while it runs).
    pub since_end_ms: u64,
    /// Waiting for an answer.
    pub asking: bool,
    /// The user sent the card away.
    pub dismissed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Hidden,
    Running,
    /// A question: the card does not wait for the delay, the operation is stopped on it.
    Asking,
    Result(Outcome),
}

pub fn card_stage(c: &CardIn) -> Stage {
    if c.dismissed {
        return Stage::Hidden;
    }
    match c.outcome {
        None if c.asking => Stage::Asking,
        None if c.ran_ms >= CARD_APPEAR_MS => Stage::Running,
        None => Stage::Hidden,
        // a failure always says so, however short the operation was; it stays until sent away
        Some(Outcome::Failed) => Stage::Result(Outcome::Failed),
        // the result of an operation whose card never showed would be a flash out of nowhere
        Some(_) if c.ran_ms < CARD_APPEAR_MS => Stage::Hidden,
        Some(Outcome::Done) if c.since_end_ms < CARD_DONE_MS => Stage::Result(Outcome::Done),
        Some(Outcome::Cancelled) if c.since_end_ms < CARD_CANCELLED_MS => Stage::Result(Outcome::Cancelled),
        Some(_) => Stage::Hidden,
    }
}

/// The cards to draw and how many more there would have been.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stack {
    /// Ids from the top card to the bottom one (the newest sits nearest the corner).
    pub shown: Vec<u64>,
    pub more: usize,
}

/// `cards` are (id, stage), oldest first. A question, then a failure, then what runs, then a
/// result fills the three places; among equals the newer wins.
pub fn stack(cards: &[(u64, Stage)]) -> Stack {
    let rank = |s: Stage| match s {
        Stage::Asking => 0,
        Stage::Result(Outcome::Failed) => 1,
        Stage::Running => 2,
        Stage::Result(_) => 3,
        Stage::Hidden => 4,
    };
    let mut live: Vec<(usize, u64)> = cards.iter().enumerate().filter(|(_, c)| c.1 != Stage::Hidden).map(|(i, c)| (i, c.0)).collect();
    // best first; among equals the newest (the later index) first
    live.sort_by_key(|&(i, _)| (rank(cards[i].1), std::cmp::Reverse(i)));
    let more = live.len().saturating_sub(MAX_CARDS);
    live.truncate(MAX_CARDS);
    // drawn by age: older above, newest at the bottom
    live.sort_by_key(|&(i, _)| i);
    Stack { shown: live.into_iter().map(|(_, id)| id).collect(), more }
}

/// `name` cut to at most `max` characters with "…" in the middle, the end (the extension) kept
/// longer than the start: "yaz_tatili_2025_final_v3.mp4" -> "yaz_tatili…_v3.mp4".
pub fn middle_truncate(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max || max < 3 {
        return if max < 3 && chars.len() > max { chars.into_iter().take(max).collect() } else { name.to_string() };
    }
    let keep = max - 1;
    let ext = name.rfind('.').map_or(0, |i| name[i..].chars().count());
    // the tail holds the extension and a few letters before it, but never most of the room
    let tail = (ext + 3).clamp(keep * 2 / 5, keep * 3 / 5).min(keep - 1);
    let head = keep - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

// ---------------------------------------------------------------------------------------------
// The Status Center button's ring
// ---------------------------------------------------------------------------------------------

/// The ring is drawn from pictures, one per step of its arc: the whole circle is this many steps.
pub const RING_STEPS: usize = 40;
/// The arc that goes round before the first progress report is a quarter circle.
pub const SPIN_SWEEP: usize = RING_STEPS / 4;
/// One turn of that arc.
pub const SPIN_MS: u64 = 1200;
/// How often the turning arc is redrawn. It has [`RING_STEPS`] positions in [`SPIN_MS`], so one
/// picture per frame at this rate shows every position; more would only make the whole window
/// redraw at the monitor's rate (60 to 144 times a second) for nothing, for as long as an
/// operation with an unknown total runs.
pub const SPIN_FPS: f32 = RING_STEPS as f32 * 1000.0 / SPIN_MS as f32;
/// A finished ring closes and then fades this long.
pub const RING_FADE_MS: u64 = 700;
/// The attention pulse.
pub const PULSE_MS: u64 = 550;

/// Which picture the ring is, and how visible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ring {
    None,
    /// Only the faint track (animations off, before the first report: there is no amount to show).
    Track,
    /// The arc from 12 o'clock clockwise, `steps` of [`RING_STEPS`] long (1 ..= RING_STEPS).
    Arc { steps: usize, alpha: f32 },
    /// A quarter-circle arc starting at `start` (of [`RING_STEPS`]).
    Spin { start: usize },
}

/// The ring for a moment. `fraction`: the overall progress of what runs (`None` before any
/// operation has reported a total). `running`: something runs. `since_end_ms`: how long ago
/// the last operation ended, when none runs. `ms`: any steadily growing clock (the spin's phase).
pub fn ring(running: bool, fraction: Option<f32>, since_end_ms: Option<u64>, animations: bool, ms: u64) -> Ring {
    if running {
        return match fraction {
            Some(f) => Ring::Arc { steps: arc_steps(f), alpha: 1.0 },
            None if animations => Ring::Spin { start: ((ms % SPIN_MS) * RING_STEPS as u64 / SPIN_MS) as usize },
            None => Ring::Track,
        };
    }
    match since_end_ms {
        // closed, then gone. Without animations it just goes: no fade to watch
        Some(e) if animations && e < RING_FADE_MS => Ring::Arc { steps: RING_STEPS, alpha: 1.0 - e as f32 / RING_FADE_MS as f32 },
        _ => Ring::None,
    }
}

/// The steps of a ring that shows `fraction`: at least one while anything is done at all, so a
/// start is not an empty circle; the last step is only reached at the very end.
pub fn arc_steps(fraction: f32) -> usize {
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let s = (f * RING_STEPS as f32).round() as usize;
    if f >= 1.0 { RING_STEPS } else { s.clamp(1, RING_STEPS - 1) }
}

/// The attention pulse at `ms` after its cause: how far it has grown (0 ..= 1) and how visible it
/// still is. `None` once it is over.
pub fn pulse(ms: u64) -> Option<(f32, f32)> {
    if ms >= PULSE_MS {
        return None;
    }
    let t = ms as f32 / PULSE_MS as f32;
    // out-cubic growth, linear fade
    let grow = 1.0 - (1.0 - t).powi(3);
    Some((grow, 0.85 * (1.0 - t)))
}

/// The pulse on the animation's own clock: `t` runs 0 to 1 over [`PULSE_MS`]. The animation has a
/// fixed length, so the shape never depends on how old the pulse was when it was first drawn.
pub fn pulse_at(t: f32) -> (f32, f32) {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 };
    pulse((t * PULSE_MS as f32) as u64).unwrap_or((1.0, 0.0))
}

/// How visible a closed ring still is at `t` (0 to 1 over [`RING_FADE_MS`]).
pub fn fade_alpha(t: f32) -> f32 {
    1.0 - if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 }
}

/// The coverage (0 = nothing, 255 = full) of an arc of the ring on a `size` x `size` picture:
/// from `start` steps past 12 o'clock, clockwise, `sweep` steps long (a full circle when it is
/// [`RING_STEPS`] or more), `thickness` thick with its outer edge `margin` pixels inside the
/// picture. Supersampled 4 x 4, with rounded ends. Row by row, one byte a pixel.
pub fn ring_coverage(size: usize, margin: f32, thickness: f32, start: usize, sweep: usize) -> Vec<u8> {
    let c = size as f32 / 2.0;
    let r_out = c - margin;
    let r_in = r_out - thickness;
    let r_mid = (r_out + r_in) / 2.0;
    let full = sweep >= RING_STEPS;
    let step = std::f32::consts::TAU / RING_STEPS as f32;
    let a0 = (start % RING_STEPS) as f32 * step;
    let span = sweep.min(RING_STEPS) as f32 * step;
    // the end caps: discs on the ring's middle line
    let cap = |a: f32| (c + r_mid * a.sin(), c - r_mid * a.cos());
    let (c0, c1) = (cap(a0), cap(a0 + span));
    let half = thickness / 2.0;
    let mut out = vec![0u8; size * size];
    for y in 0..size {
        for x in 0..size {
            let mut hit = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let (dx, dy) = (px - c, py - c);
                    let r = (dx * dx + dy * dy).sqrt();
                    let on_ring = r >= r_in && r <= r_out;
                    let inside = if full || !on_ring {
                        on_ring
                    } else {
                        // clockwise angle from 12 o'clock
                        let a = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
                        (a - a0).rem_euclid(std::f32::consts::TAU) <= span
                    };
                    let in_cap = !full && sweep > 0 && {
                        let d = |p: (f32, f32)| ((px - p.0).powi(2) + (py - p.1).powi(2)).sqrt();
                        d(c0) <= half || d(c1) <= half
                    };
                    if inside || in_cap {
                        hit += 1;
                    }
                }
            }
            out[y * size + x] = (hit * 255 / 16) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the average ----

    /// A steady 10 units a second for `secs` seconds, a report every 100 ms.
    fn steady(m: &mut SpeedMeter, from: f64, secs: f64, rate: f64, base: u64) -> u64 {
        let mut t = from;
        let mut done = base;
        while t < from + secs {
            t += 0.1;
            done = base + ((t - from) * rate) as u64;
            m.push(t, done);
        }
        done
    }

    #[test]
    fn a_steady_copy_reads_its_speed() {
        let mut m = SpeedMeter::default();
        steady(&mut m, 0.0, 5.0, 1_000_000.0, 0);
        let r = m.rate(5.0).unwrap();
        assert!((r - 1_000_000.0).abs() < 30_000.0, "{r}");
    }

    #[test]
    fn too_little_data_gives_no_speed() {
        let mut m = SpeedMeter::default();
        assert_eq!(m.rate(1.0), None);
        m.push(0.1, 1000);
        assert_eq!(m.rate(0.1), None, "one report is not a rate");
        m.push(0.2, 2000);
        assert_eq!(m.rate(0.2), None, "a tenth of a second is not enough to tell");
        m.push(1.0, 10_000);
        assert!(m.rate(1.0).is_some());
    }

    #[test]
    fn a_burst_is_smoothed_and_leaves_the_window() {
        let mut m = SpeedMeter::default();
        // 1 MB/s for 6 s, then one second of 50 MB/s (a file from the cache), then 1 MB/s again
        let mut done = steady(&mut m, 0.0, 6.0, 1_000_000.0, 0);
        done = steady(&mut m, 6.0, 1.0, 50_000_000.0, done);
        let during = m.rate(7.0).unwrap();
        assert!(during > 5_000_000.0 && during < 20_000_000.0, "the burst is averaged with the 4 s before it: {during}");
        steady(&mut m, 7.0, 6.0, 1_000_000.0, done);
        let after = m.rate(13.0).unwrap();
        assert!((after - 1_000_000.0).abs() < 100_000.0, "ten seconds later the burst is forgotten: {after}");
    }

    #[test]
    fn a_stall_runs_the_speed_out_to_zero() {
        let mut m = SpeedMeter::default();
        let done = steady(&mut m, 0.0, 5.0, 1_000_000.0, 0);
        assert!(m.rate(5.2).unwrap() > 500_000.0, "a short silence is not a stall");
        // the disk stops answering: no reports
        let mid = m.rate(7.0).unwrap();
        assert!(mid > 0.0 && mid < 1_000_000.0, "falling: {mid}");
        assert_eq!(m.rate(12.0), Some(0.0), "after the window has passed it is 0");
        // and it recovers
        steady(&mut m, 12.0, 3.0, 2_000_000.0, done);
        assert!(m.rate(15.0).unwrap() > 1_000_000.0);
    }

    #[test]
    fn pausing_does_not_pull_the_average_down() {
        let mut m = SpeedMeter::default();
        let done = steady(&mut m, 0.0, 5.0, 1_000_000.0, 0);
        m.pause();
        assert_eq!(m.rate(8.0), None, "no speed while paused");
        assert!(m.is_paused());
        m.push(9.0, done);
        // an hour later
        m.resume(3600.0);
        assert_eq!(m.rate(3600.0), None, "starting over");
        steady(&mut m, 3600.0, 3.0, 1_000_000.0, done);
        let r = m.rate(3603.0).unwrap();
        assert!((r - 1_000_000.0).abs() < 50_000.0, "the hour away is not counted: {r}");
    }

    #[test]
    fn a_counter_that_restarts_starts_the_average_over() {
        let mut m = SpeedMeter::default();
        steady(&mut m, 0.0, 3.0, 1_000_000.0, 0);
        // the copy is done and its check begins to count from 0
        m.push(3.1, 0);
        m.push(4.0, 500);
        let r = m.rate(4.0).unwrap();
        assert!(r < 1000.0, "{r}");
        // time never runs backwards
        m.push(1.0, 999_999_999);
        assert!(m.rate(4.0).unwrap() < 1000.0);
    }

    #[test]
    fn the_time_left() {
        assert_eq!(eta_secs(1000, Some(100.0)), Some(10.0));
        assert_eq!(eta_secs(0, Some(100.0)), None, "nothing left");
        assert_eq!(eta_secs(1000, Some(0.0)), None, "a stalled copy has no end in sight");
        assert_eq!(eta_secs(1000, None), None);
        assert_eq!(eta_secs(1000, Some(f64::NAN)), None);
        assert_eq!(eta_secs(u64::MAX, Some(1.0)), None, "years are not an estimate");
    }

    #[test]
    fn stats_give_speed_in_bytes_and_eta_from_the_same_rate() {
        let mut s = OpStats::default();
        let total = 100_000_000u64;
        let mut t = 0.0;
        while t < 5.0 {
            t += 0.1;
            let done = (t * 10_000_000.0) as u64;
            s.update(t, ((done * 1000 / total) as u32, 1000), (done, total));
        }
        let done = (t * 10_000_000.0) as u64;
        let pace = s.pace(t, ((done * 1000 / total) as u32, 1000), (done, total));
        let speed = pace.speed.unwrap();
        assert!((speed - 10_000_000.0).abs() < 400_000.0, "{speed}");
        let eta = pace.eta_secs.unwrap();
        assert!((eta - 5.0).abs() < 0.5, "{eta}");
        assert_eq!(s.bars(t).len(), 3, "five seconds = three bars of two");
    }

    #[test]
    fn an_engine_without_bytes_gives_a_time_left_but_no_speed() {
        let mut s = OpStats::default();
        let mut t = 0.0;
        while t < 4.0 {
            t += 0.1;
            s.update(t, ((t * 100.0) as u32, 1000), (0, 0));
        }
        let pace = s.pace(t, ((t * 100.0) as u32, 1000), (0, 0));
        assert_eq!(pace.speed, None);
        let eta = pace.eta_secs.unwrap();
        assert!((eta - 6.0).abs() < 0.6, "{eta}");
    }

    #[test]
    fn when_the_byte_count_ends_the_units_carry_on_with_a_time_left() {
        // our engine copies by bytes; then the shell takes the leftovers (or a check runs) and
        // the counter is cleared: the bytes' average must not freeze into a speed of 0 and no time left
        let mut s = OpStats::default();
        let mut t = 0.0;
        while t < 4.0 {
            t += 0.1;
            s.update(t, (1000, 1000), ((t * 1_000_000.0) as u64, 8_000_000));
        }
        assert!(s.pace(t, (1000, 1000), ((t * 1_000_000.0) as u64, 8_000_000)).speed.is_some());
        // the check starts counting its own units from 0
        let mut u = 0u32;
        while u < 400 {
            t += 0.1;
            u += 40;
            s.update(t, (u, 1000), (0, 0));
        }
        let pace = s.pace(t, (u, 1000), (0, 0));
        assert_eq!(pace.speed, None, "units are not bytes");
        let eta = pace.eta_secs.expect("a time left from the units");
        assert!((eta - 1.5).abs() < 0.5, "{eta}");
    }

    #[test]
    fn an_unknown_total_has_no_time_left() {
        let mut s = OpStats::default();
        for i in 1..40 {
            s.update(i as f64 * 0.1, (0, 0), (0, 0));
        }
        assert_eq!(s.pace(4.0, (0, 0), (0, 0)), Pace::default());
        // and the first report of a copy that has not counted anything yet has no speed either
        let fresh = OpStats::default();
        assert_eq!(fresh.pace(0.0, (0, 0), (0, 0)), Pace::default());
    }

    #[test]
    fn a_pause_in_the_stats_hides_the_pace() {
        let mut s = OpStats::default();
        for i in 1..60 {
            let t = i as f64 * 0.1;
            s.update(t, (i * 10, 1000), ((i as u64) * 1_000_000, 100_000_000));
        }
        assert!(s.pace(6.0, (590, 1000), (59_000_000, 100_000_000)).speed.is_some());
        s.pause();
        assert_eq!(s.pace(7.0, (590, 1000), (59_000_000, 100_000_000)), Pace::default());
        s.resume(10.0);
        assert_eq!(s.pace(10.0, (590, 1000), (59_000_000, 100_000_000)).speed, None);
    }

    #[test]
    fn the_history_is_bars_of_two_seconds_and_a_stall_is_a_hole() {
        let mut h = SpeedHistory::default();
        let mut done = 0u64;
        // 0..4 s at 1 MB/s
        for i in 1..40 {
            done += 100_000;
            h.push(i as f64 * 0.1, done);
        }
        // nothing from 4 to 8 s, then a report at 8.1
        done += 100_000;
        h.push(8.1, done);
        let bars = h.bars(8.1);
        assert_eq!(bars.len(), 5);
        assert!((bars[0] - 1_000_000.0).abs() < 150_000.0 && (bars[1] - 1_000_000.0).abs() < 150_000.0, "{bars:?}");
        assert_eq!((bars[2], bars[3]), (0.0, 0.0), "{bars:?}");
        let n = normalized(&bars);
        assert!(n.iter().all(|&v| (0.0..=1.0).contains(&v)));
        assert!(n[0] > 0.85);
        assert_eq!(normalized(&[0.0, 0.0]), vec![0.0, 0.0]);
        assert!(normalized(&[]).is_empty());
    }

    #[test]
    fn the_history_keeps_the_last_minute() {
        let mut h = SpeedHistory::default();
        for i in 0..400 {
            h.push(i as f64, i as u64 * 1000);
        }
        assert_eq!(h.bars(400.0).len(), HISTORY_BARS);
    }

    // ---- the strip ----

    fn run(asking: bool) -> Brief {
        Brief { running: true, noticed: true, asking, failed: false, dismissed: false }
    }
    fn fresh() -> Brief {
        Brief { running: true, noticed: false, asking: false, failed: false, dismissed: false }
    }
    fn fail(dismissed: bool) -> Brief {
        Brief { running: false, noticed: true, asking: false, failed: true, dismissed }
    }
    fn done() -> Brief {
        Brief { running: false, noticed: true, asking: false, failed: false, dismissed: false }
    }

    #[test]
    fn a_short_operation_never_flashes_the_strip() {
        // just started: nothing (so the status bar does not jump on Home or Settings either)
        assert_eq!(strip_pick(&[fresh()]), Strip::Nothing);
        // an older one keeps the strip and does not count the fresh one as "+1" yet
        assert_eq!(strip_pick(&[run(false), fresh()]), Strip::Running { index: 0, others: 0 });
        // a question does not wait
        let ask = Brief { asking: true, ..fresh() };
        assert_eq!(strip_pick(&[ask]), Strip::Running { index: 0, others: 0 });
        // a failure is not delayed either
        assert_eq!(strip_pick(&[Brief { running: false, failed: true, ..fresh() }]), Strip::Failed { index: 0 });
    }

    #[test]
    fn the_strip_shows_the_newest_running_one_and_counts_the_rest() {
        assert_eq!(strip_pick(&[]), Strip::Nothing);
        assert_eq!(strip_pick(&[done(), fail(true)]), Strip::Nothing);
        assert_eq!(strip_pick(&[run(false)]), Strip::Running { index: 0, others: 0 });
        assert_eq!(strip_pick(&[run(false), done(), run(false), run(false)]), Strip::Running { index: 3, others: 2 });
        // a finished one after a running one does not take the strip
        assert_eq!(strip_pick(&[run(false), done()]), Strip::Running { index: 0, others: 0 });
    }

    #[test]
    fn a_question_comes_before_a_newer_operation() {
        assert_eq!(strip_pick(&[run(true), run(false)]), Strip::Running { index: 0, others: 1 });
        assert_eq!(strip_pick(&[run(true), run(true), run(false)]), Strip::Running { index: 1, others: 2 });
    }

    #[test]
    fn a_failure_is_shown_until_it_is_looked_at_and_never_over_a_running_one() {
        assert_eq!(strip_pick(&[fail(false)]), Strip::Failed { index: 0 });
        assert_eq!(strip_pick(&[fail(false), fail(false), done()]), Strip::Failed { index: 1 });
        assert_eq!(strip_pick(&[fail(true)]), Strip::Nothing);
        assert_eq!(strip_pick(&[fail(false), run(false)]), Strip::Running { index: 1, others: 0 });
    }

    #[test]
    fn a_narrow_window_loses_the_time_then_the_speed_then_the_text() {
        let full = strip_fit(2000.0, 2);
        assert_eq!(full, StripFit { buttons: true, chip: true, text: true, speed: true, eta: true });
        // find where each piece drops, walking the width down
        let mut order = Vec::new();
        let mut prev = full;
        let mut w = 1000.0;
        while w > 100.0 {
            let f = strip_fit(w, 2);
            for (name, was, is) in [("eta", prev.eta, f.eta), ("speed", prev.speed, f.speed), ("text", prev.text, f.text), ("chip", prev.chip, f.chip), ("buttons", prev.buttons, f.buttons)] {
                if was && !is {
                    order.push(name);
                }
            }
            prev = f;
            w -= 1.0;
        }
        assert_eq!(order, ["eta", "speed", "text", "chip", "buttons"]);
    }

    #[test]
    fn the_strip_has_no_gap_in_the_middle() {
        for w in 0..1200 {
            let f = strip_fit(w as f32, 1);
            // a piece that is on needs every piece before it
            assert!(!f.chip || f.buttons, "{w}");
            assert!(!f.text || f.chip, "{w}");
            assert!(!f.speed || f.text, "{w}");
            assert!(!f.eta || f.speed, "{w}");
        }
        // without others there is no chip, and nothing else waits for it
        let f = strip_fit(1000.0, 0);
        assert!(!f.chip && f.buttons && f.text && f.speed && f.eta);
        // the bar and the percent are always there: even nothing fits is the base
        assert_eq!(strip_fit(0.0, 0), StripFit { buttons: false, chip: false, text: false, speed: false, eta: false });
    }

    // ---- the card ----

    fn running(ms: u64) -> CardIn {
        CardIn { outcome: None, ran_ms: ms, since_end_ms: 0, asking: false, dismissed: false }
    }
    fn ended(o: Outcome, ran: u64, since: u64) -> CardIn {
        CardIn { outcome: Some(o), ran_ms: ran, since_end_ms: since, asking: false, dismissed: false }
    }

    #[test]
    fn a_card_waits_before_it_appears() {
        assert_eq!(card_stage(&running(0)), Stage::Hidden);
        assert_eq!(card_stage(&running(599)), Stage::Hidden);
        assert_eq!(card_stage(&running(600)), Stage::Running);
        assert_eq!(card_stage(&running(60_000)), Stage::Running);
    }

    #[test]
    fn a_short_operation_never_flashes_a_card_not_even_its_result() {
        assert_eq!(card_stage(&ended(Outcome::Done, 200, 10)), Stage::Hidden);
        assert_eq!(card_stage(&ended(Outcome::Cancelled, 599, 10)), Stage::Hidden);
    }

    #[test]
    fn a_result_stays_four_seconds() {
        assert_eq!(card_stage(&ended(Outcome::Done, 5000, 0)), Stage::Result(Outcome::Done));
        assert_eq!(card_stage(&ended(Outcome::Done, 5000, 3999)), Stage::Result(Outcome::Done));
        assert_eq!(card_stage(&ended(Outcome::Done, 5000, 4000)), Stage::Hidden);
        assert_eq!(card_stage(&ended(Outcome::Cancelled, 5000, 1999)), Stage::Result(Outcome::Cancelled));
        assert_eq!(card_stage(&ended(Outcome::Cancelled, 5000, 2000)), Stage::Hidden);
    }

    #[test]
    fn a_failure_stays_until_sent_away_even_after_a_short_run() {
        assert_eq!(card_stage(&ended(Outcome::Failed, 50, 0)), Stage::Result(Outcome::Failed));
        assert_eq!(card_stage(&ended(Outcome::Failed, 50, 3_600_000)), Stage::Result(Outcome::Failed));
        let mut c = ended(Outcome::Failed, 50, 10);
        c.dismissed = true;
        assert_eq!(card_stage(&c), Stage::Hidden);
        let mut r = running(10_000);
        r.dismissed = true;
        assert_eq!(card_stage(&r), Stage::Hidden);
    }

    #[test]
    fn a_question_shows_at_once() {
        let mut c = running(100);
        c.asking = true;
        assert_eq!(card_stage(&c), Stage::Asking);
    }

    #[test]
    fn at_most_three_cards_and_a_count_of_the_rest() {
        let run = Stage::Running;
        let s = stack(&[(1, run), (2, run), (3, run)]);
        assert_eq!((s.shown.as_slice(), s.more), (&[1, 2, 3][..], 0));
        let s = stack(&[(1, run), (2, run), (3, run), (4, run), (5, run)]);
        assert_eq!(s.shown, vec![3, 4, 5], "the newest three, drawn oldest on top");
        assert_eq!(s.more, 2);
        assert_eq!(stack(&[(1, Stage::Hidden)]), Stack::default());
        assert_eq!(stack(&[]), Stack::default());
    }

    #[test]
    fn a_question_and_a_failure_beat_what_merely_runs() {
        let run = Stage::Running;
        let s = stack(&[(1, Stage::Asking), (2, run), (3, run), (4, run), (5, Stage::Result(Outcome::Failed)), (6, Stage::Result(Outcome::Done))]);
        assert_eq!(s.shown, vec![1, 4, 5], "the question, the failure and the newest runner");
        assert_eq!(s.more, 3);
        // a finished copy gives its place to a running one
        let s = stack(&[(1, Stage::Result(Outcome::Done)), (2, run), (3, run), (4, run)]);
        assert_eq!(s.shown, vec![2, 3, 4]);
        assert_eq!(s.more, 1);
    }

    #[test]
    fn long_names_are_cut_in_the_middle_and_keep_their_ending() {
        assert_eq!(middle_truncate("kısa.txt", 20), "kısa.txt");
        assert_eq!(middle_truncate("tam yirmi karakter.", 19), "tam yirmi karakter.");
        let cut = middle_truncate("yaz_tatili_2025_final_v3.mp4", 20);
        assert_eq!(cut.chars().count(), 20, "{cut}");
        assert!(cut.starts_with("yaz_ta") && cut.ends_with("v3.mp4") && cut.contains('…'), "{cut}");
        // no extension: both ends
        let cut = middle_truncate("abcdefghijklmnopqrstuvwxyz", 11);
        assert_eq!(cut.chars().count(), 11);
        assert!(cut.starts_with("abc") && cut.ends_with("xyz") && cut.contains('…'), "{cut}");
        // an extension longer than the room does not eat the start
        let cut = middle_truncate("a.verylongextensionhere", 10);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.starts_with("a.v"), "{cut}");
        // multi-byte characters count as one
        let cut = middle_truncate("çğüşıöçğüşıö_çok_uzun_ad.pdf", 12);
        assert_eq!(cut.chars().count(), 12);
        assert!(cut.ends_with(".pdf"));
        // degenerate widths
        assert_eq!(middle_truncate("abcdef", 0), "");
        assert_eq!(middle_truncate("abcdef", 2), "ab");
        assert_eq!(middle_truncate("", 5), "");
    }

    // ---- the ring ----

    #[test]
    fn the_ring_follows_the_progress() {
        assert_eq!(arc_steps(0.0), 1, "a start is not an empty circle");
        assert_eq!(arc_steps(0.5), RING_STEPS / 2);
        assert_eq!(arc_steps(0.999), RING_STEPS - 1, "closed only at the end");
        assert_eq!(arc_steps(1.0), RING_STEPS);
        assert_eq!(arc_steps(7.0), RING_STEPS);
        assert_eq!(arc_steps(-1.0), 1);
        assert_eq!(arc_steps(f32::NAN), 1);
        let mut last = 0;
        for i in 0..=100 {
            let s = arc_steps(i as f32 / 100.0);
            assert!(s >= last, "never shrinks as the progress grows");
            last = s;
        }
    }

    #[test]
    fn before_the_first_report_a_short_arc_goes_round() {
        assert_eq!(ring(true, None, None, true, 0), Ring::Spin { start: 0 });
        assert_eq!(ring(true, None, None, true, SPIN_MS / 2), Ring::Spin { start: RING_STEPS / 2 });
        assert_eq!(ring(true, None, None, true, SPIN_MS), Ring::Spin { start: 0 });
        let starts: std::collections::BTreeSet<usize> = (0..SPIN_MS).step_by(10).map(|ms| match ring(true, None, None, true, ms) {
            Ring::Spin { start } => start,
            r => panic!("{r:?}"),
        }).collect();
        assert_eq!(starts.len(), RING_STEPS, "it visits every position");
        assert!(starts.iter().all(|&s| s < RING_STEPS));
    }

    #[test]
    fn with_animations_off_the_ring_only_shows_the_amount() {
        assert_eq!(ring(true, None, None, false, 500), Ring::Track, "nothing to show, nothing moves");
        assert_eq!(ring(true, Some(0.5), None, false, 500), Ring::Arc { steps: RING_STEPS / 2, alpha: 1.0 });
        assert_eq!(ring(false, None, Some(100), false, 0), Ring::None, "it just goes at the end");
    }

    #[test]
    fn a_finished_ring_closes_and_fades() {
        assert_eq!(ring(false, None, None, true, 0), Ring::None, "nothing animates when nothing runs");
        match ring(false, Some(1.0), Some(0), true, 0) {
            Ring::Arc { steps, alpha } => assert_eq!((steps, alpha), (RING_STEPS, 1.0)),
            r => panic!("{r:?}"),
        }
        match ring(false, None, Some(RING_FADE_MS / 2), true, 0) {
            Ring::Arc { steps, alpha } => assert!(steps == RING_STEPS && (alpha - 0.5).abs() < 0.01),
            r => panic!("{r:?}"),
        }
        assert_eq!(ring(false, None, Some(RING_FADE_MS), true, 0), Ring::None);
        assert_eq!(ring(false, None, Some(60_000), true, 0), Ring::None);
    }

    #[test]
    fn the_pulse_is_short_and_fades() {
        let (g0, a0) = pulse(0).unwrap();
        assert_eq!(g0, 0.0);
        assert!(a0 > 0.8);
        let (g1, a1) = pulse(PULSE_MS / 2).unwrap();
        assert!(g1 > 0.5 && a1 < a0);
        assert_eq!(pulse(PULSE_MS), None);
        assert_eq!(pulse(10_000), None);
    }

    #[test]
    fn the_pulse_and_the_fade_follow_their_own_fixed_clock() {
        // an animation of a fixed length hands over 0 to 1: the shape is the designed one at every point
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let want = pulse((t * PULSE_MS as f32) as u64).unwrap_or((1.0, 0.0));
            assert_eq!(pulse_at(t), want, "{t}");
        }
        assert_eq!(pulse_at(0.0).0, 0.0);
        assert_eq!(pulse_at(1.0), (1.0, 0.0));
        // it only ever gets bigger and fainter, and never leaves 0 to 1
        let mut last = (0.0, 1.0);
        for i in 0..=100 {
            let (g, a) = pulse_at(i as f32 / 100.0);
            assert!(g >= last.0 && a <= last.1 && (0.0..=1.0).contains(&g) && (0.0..=1.0).contains(&a));
            last = (g, a);
        }
        assert_eq!(fade_alpha(0.0), 1.0);
        assert_eq!(fade_alpha(0.5), 0.5);
        assert_eq!(fade_alpha(1.0), 0.0);
        assert_eq!(fade_alpha(7.0), 0.0);
        assert_eq!(fade_alpha(f32::NAN), 0.0);
    }

    #[test]
    fn the_turning_arc_is_drawn_once_per_position_and_no_more() {
        // one picture per frame shows every position, and the rate is far below a monitor's
        let frame_ms = 1000.0 / SPIN_FPS;
        let step_ms = SPIN_MS as f32 / RING_STEPS as f32;
        assert!((frame_ms - step_ms).abs() < 0.01, "{frame_ms} vs {step_ms}");
        assert!(std::hint::black_box(SPIN_FPS) < 60.0, "{SPIN_FPS}");
    }

    #[test]
    fn the_strips_room_is_what_the_status_bar_leaves() {
        let empty = BarContents::default();
        // nothing else in the bar: all but the frame
        assert_eq!(strip_room(1000.0, empty), 952.0);
        let busy = BarContents { items: true, selected: true, git: true, ide: true, update: true, ..empty };
        assert!(strip_room(1000.0, busy) < strip_room(1000.0, BarContents { items: true, ..empty }));
        // never negative
        assert_eq!(strip_room(100.0, busy), 0.0);
        assert_eq!(strip_room(0.0, empty), 0.0);
        // the compact overlay (420 px, no sidebar) with just the count still has room for the bar and buttons
        let compact = strip_room(420.0 - 10.0, BarContents { items: true, ..empty });
        assert!(compact >= 140.0 + 10.0 + 56.0, "{compact}");
        let f = strip_fit(compact, 0);
        assert!(f.buttons);
        // more in the bar, less for the strip
        let a = strip_room(900.0, BarContents { items: true, ..empty });
        let b = strip_room(900.0, BarContents { items: true, git: true, ..empty });
        assert!(b < a);
    }

    // ---- the picture of the arc ----

    fn at(px: &[u8], size: usize, x: usize, y: usize) -> u8 {
        px[y * size + x]
    }

    #[test]
    fn the_full_ring_is_a_ring() {
        let size = 64;
        let px = ring_coverage(size, 3.0, 4.0, 0, RING_STEPS);
        assert_eq!(px.len(), size * size);
        // outer radius 29, inner 25: the middle of the band is full, the centre and the corner are empty
        assert_eq!(at(&px, size, 32, 32 - 27), 255, "top of the band");
        assert_eq!(at(&px, size, 32 + 27, 32), 255, "right");
        assert_eq!(at(&px, size, 32, 32 + 27), 255, "bottom");
        assert_eq!(at(&px, size, 32 - 27, 32), 255, "left");
        assert_eq!(at(&px, size, 32, 32), 0);
        assert_eq!(at(&px, size, 1, 1), 0);
        // the edge is smooth
        assert!(px.iter().any(|&v| v > 0 && v < 255));
        // symmetric left/right and up/down
        for y in 0..size {
            for x in 0..size {
                let a = at(&px, size, x, y) as i32;
                let b = at(&px, size, size - 1 - x, y) as i32;
                assert!((a - b).abs() <= 1, "{x},{y}");
            }
        }
    }

    #[test]
    fn an_arc_starts_at_the_top_and_goes_clockwise() {
        let size = 64;
        // a quarter: from 12 o'clock to 3 o'clock
        let px = ring_coverage(size, 3.0, 4.0, 0, RING_STEPS / 4);
        assert!(at(&px, size, 32 + 10, 32 - 27) > 200, "just right of the top is inside");
        assert!(at(&px, size, 32 + 27, 32) > 100, "the 3 o'clock end (a rounded cap)");
        assert_eq!(at(&px, size, 32 - 10, 32 - 27), 0, "just left of the top is outside");
        assert_eq!(at(&px, size, 32, 32 + 27), 0, "the bottom is outside");
        assert_eq!(at(&px, size, 32 - 27, 32), 0, "the left is outside");
        // the same quarter started half a turn later is the other side
        let px = ring_coverage(size, 3.0, 4.0, RING_STEPS / 2, RING_STEPS / 4);
        assert!(at(&px, size, 32 - 10, 32 + 25) > 200, "past 6 o'clock towards 9");
        assert_eq!(at(&px, size, 32 + 10, 32 - 27), 0);
    }

    #[test]
    fn more_steps_cover_more() {
        let sum = |steps| ring_coverage(64, 3.0, 4.0, 0, steps).iter().map(|&v| u64::from(v)).sum::<u64>();
        let mut last = 0;
        for s in 1..=RING_STEPS {
            let v = sum(s);
            assert!(v > last, "step {s}");
            last = v;
        }
        let full = sum(RING_STEPS);
        let half = sum(RING_STEPS / 2);
        assert!(half * 2 > full && half * 2 < full + full / 4, "half is about half (plus the caps): {half} {full}");
    }
}
