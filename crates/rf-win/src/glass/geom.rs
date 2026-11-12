//! Rectangle sums for the glass engine, with no Windows type in them so they can be tested to the last texel.
//!
//! One job uses them: [`covered_by`], whether a glass is hidden from sight by the windows above it (a union of rectangles contains it).
//!
//! All rectangles are half open, `left..right` and `top..bottom`, in whatever pixel space the caller works in.

/// A rectangle, half open: it holds the pixels `l..r` by `t..b`. One with no area (`r <= l` or `b <= t`) is empty and equal to every
/// other empty one in effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rc {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Rc {
    pub(super) const fn new(l: i32, t: i32, r: i32, b: i32) -> Rc {
        Rc { l, t, r, b }
    }

    /// From a corner and a size (sizes that overflow are cut to the range of `i32`).
    pub(super) fn xywh(x: i32, y: i32, w: u32, h: u32) -> Rc {
        let end = |p: i32, n: u32| (p as i64 + n as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Rc { l: x, t: y, r: end(x, w), b: end(y, h) }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }

    pub(super) fn intersect(&self, o: &Rc) -> Rc {
        Rc { l: self.l.max(o.l), t: self.t.max(o.t), r: self.r.min(o.r), b: self.b.min(o.b) }
    }

    pub(super) fn meets(&self, o: &Rc) -> bool {
        !self.intersect(o).is_empty()
    }
}

/// `a` without `b`: up to four rectangles that do not overlap and together hold exactly the pixels of `a` that are not in `b`.
pub(super) fn minus(a: Rc, b: Rc) -> Vec<Rc> {
    if a.is_empty() {
        return Vec::new();
    }
    let i = a.intersect(&b);
    if i.is_empty() {
        return vec![a];
    }
    let mut out = Vec::with_capacity(4);
    // the band above and the band below the cut take the whole width; the pieces beside it only the cut's height
    if i.t > a.t {
        out.push(Rc::new(a.l, a.t, a.r, i.t));
    }
    if i.l > a.l {
        out.push(Rc::new(a.l, i.t, i.l, i.b));
    }
    if i.r < a.r {
        out.push(Rc::new(i.r, i.t, a.r, i.b));
    }
    if i.b < a.b {
        out.push(Rc::new(a.l, i.b, a.r, a.b));
    }
    out
}

/// A target cut into more pieces than this by the covers is not told apart: `covered_by` then says "not covered".
const MAX_PIECES: usize = 48;

/// Whether the union of `covers` holds every pixel of `target`. An empty target is covered. Pieces are counted: past a handful of
/// covers that cut the target into too many pieces the answer is "not covered", which only ever makes the caller do MORE work.
pub(super) fn covered_by(target: Rc, covers: &[Rc]) -> bool {
    if target.is_empty() {
        return true;
    }
    let mut left = vec![target];
    for c in covers.iter().filter(|c| !c.is_empty() && c.meets(&target)) {
        let mut next = Vec::with_capacity(left.len() + 3);
        for p in &left {
            next.extend(minus(*p, *c));
        }
        left = next;
        if left.is_empty() {
            return true;
        }
        if left.len() > 4 * MAX_PIECES {
            return false;
        }
    }
    left.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc(l: i32, t: i32, r: i32, b: i32) -> Rc {
        Rc::new(l, t, r, b)
    }

    /// Every pixel of a small grid decided by `f`, to compare sets with brute force.
    fn pixels(w: i32, h: i32, f: impl Fn(i32, i32) -> bool) -> Vec<bool> {
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect()
    }

    fn inside(r: &Rc, x: i32, y: i32) -> bool {
        x >= r.l && x < r.r && y >= r.t && y < r.b
    }

    #[test]
    fn minus_is_exact_and_disjoint() {
        let (w, h) = (14, 12);
        let cases = [
            (rc(0, 0, 10, 10), rc(3, 3, 6, 6)),
            (rc(0, 0, 10, 10), rc(-5, -5, 20, 20)),
            (rc(0, 0, 10, 10), rc(10, 0, 12, 10)),
            (rc(2, 2, 9, 9), rc(0, 4, 5, 6)),
            (rc(2, 2, 9, 9), rc(5, 0, 7, 12)),
            (rc(2, 2, 9, 9), rc(0, 0, 0, 0)),
            (rc(5, 5, 5, 8), rc(0, 0, 9, 9)),
        ];
        for (a, b) in cases {
            let parts = minus(a, b);
            assert!(parts.len() <= 4);
            let got = pixels(w, h, |x, y| parts.iter().any(|p| inside(p, x, y)));
            let want = pixels(w, h, |x, y| !a.is_empty() && inside(&a, x, y) && !(inside(&b, x, y) && !b.is_empty()));
            assert_eq!(got, want, "{a:?} minus {b:?}");
            let count = pixels(w, h, |x, y| parts.iter().filter(|p| inside(p, x, y)).count() > 1);
            assert!(count.iter().all(|c| !c), "the pieces of {a:?} minus {b:?} overlap");
        }
    }

    #[test]
    fn covered_by_is_the_union_test() {
        let t = rc(10, 10, 110, 90);
        assert!(covered_by(t, &[rc(0, 0, 200, 200)]), "one window over it");
        assert!(covered_by(t, &[rc(0, 0, 60, 200), rc(50, 0, 200, 200)]), "two windows that overlap and together cover it");
        assert!(!covered_by(t, &[rc(0, 0, 60, 200), rc(61, 0, 200, 200)]), "a one pixel column is left open");
        assert!(!covered_by(t, &[rc(0, 0, 200, 89)]), "the last row is open");
        assert!(covered_by(t, &[rc(0, 0, 200, 50), rc(0, 50, 200, 90), rc(0, 0, 5, 5)]), "stacked windows and one that does not touch");
        assert!(!covered_by(t, &[]), "nothing above it");
        assert!(covered_by(rc(5, 5, 5, 50), &[]), "no area to see");
        // four windows that meet in the middle of the target and leave a hole there
        let t = rc(0, 0, 100, 100);
        assert!(!covered_by(t, &[rc(0, 0, 49, 49), rc(51, 0, 100, 49), rc(0, 51, 49, 100), rc(51, 51, 100, 100)]));
        assert!(covered_by(t, &[rc(0, 0, 51, 51), rc(49, 0, 100, 51), rc(0, 49, 51, 100), rc(49, 49, 100, 100)]));
    }

    #[test]
    fn covered_by_agrees_with_counting_pixels() {
        let mut rng = Lcg(7);
        for _ in 0..400 {
            let t = rand_rect(&mut rng, 30, 20);
            let covers: Vec<Rc> = (0..rng.below(5)).map(|_| rand_rect(&mut rng, 30, 20)).collect();
            let brute = (t.t..t.b).all(|y| (t.l..t.r).all(|x| covers.iter().any(|c| inside(c, x, y))));
            assert_eq!(covered_by(t, &covers), brute || t.is_empty(), "{t:?} by {covers:?}");
        }
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    fn rand_rect(rng: &mut Lcg, w: i32, h: i32) -> Rc {
        let (x, y) = (rng.below(w as u64) as i32 - 3, rng.below(h as u64) as i32 - 3);
        let (rw, rh) = (1 + rng.below(w as u64 / 2) as i32, 1 + rng.below(h as u64 / 2) as i32);
        rc(x, y, x + rw, y + rh)
    }

}
