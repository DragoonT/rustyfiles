//! H.264 picture management for frame pictures: picture order count of types 0, 1 and 2
//! (ITU-T H.264, 8.2.1), reference marking by sliding window or memory management control
//! operations (8.2.5), and the output order with bumping (C.4). The pictures themselves live
//! on the GPU; this keeps track of which surface holds which picture.

use std::collections::VecDeque;

use crate::dpb::{OutFrame, RefMark};
use crate::h264::{Mmco, SliceHeader, Sps};
use crate::{Error, Result};

/// A decoded frame in the buffer.
#[derive(Debug, Clone)]
pub struct Pic {
    pub surface: usize,
    pub frame_num: u32,
    /// TopFieldOrderCnt and BottomFieldOrderCnt.
    pub top_poc: i32,
    pub bottom_poc: i32,
    pub pts: i64,
    pub mark: RefMark,
    /// LongTermFrameIdx, for long-term frames.
    pub long_idx: u32,
    pub needed_for_output: bool,
}

impl Pic {
    pub fn poc(&self) -> i32 {
        self.top_poc.min(self.bottom_poc)
    }
}

/// What starting a picture works out.
#[derive(Debug, Clone, Copy)]
pub struct Started {
    pub top_poc: i32,
    pub bottom_poc: i32,
    frame_num_offset: i32,
    poc_msb: i32,
}

pub struct Dpb {
    pics: Vec<Pic>,
    pool: usize,
    started: bool,
    prev_poc_msb: i32,
    prev_poc_lsb: i32,
    prev_frame_num: u32,
    prev_frame_num_offset: i32,
    prev_ref_frame_num: u32,
    ready: VecDeque<OutFrame>,
    held: Option<usize>,
}

impl Dpb {
    /// A buffer over `pool` surfaces.
    pub fn new(pool: usize) -> Self {
        Self { pics: Vec::new(), pool, started: false, prev_poc_msb: 0, prev_poc_lsb: 0, prev_frame_num: 0, prev_frame_num_offset: 0, prev_ref_frame_num: 0, ready: VecDeque::new(), held: None }
    }

    /// The frames kept as reference, in a stable order (what DXVA calls RefFrameList).
    pub fn ref_pics(&self) -> Vec<&Pic> {
        self.pics.iter().filter(|p| p.mark != RefMark::Unused).collect()
    }

    /// A surface nothing refers to any more.
    pub fn alloc(&self) -> Option<usize> {
        (0..self.pool).find(|&s| !self.pics.iter().any(|p| p.surface == s) && !self.ready.iter().any(|f| f.surface == s) && self.held != Some(s))
    }

    pub fn release_held(&mut self) {
        self.held = None;
    }

    pub fn next_output(&mut self) -> Option<OutFrame> {
        let f = self.ready.pop_front()?;
        self.held = Some(f.surface);
        Some(f)
    }

    pub fn has_output(&self) -> bool {
        !self.ready.is_empty()
    }

    /// An end of sequence NAL unit: the next picture is an IDR picture.
    pub fn end_of_sequence(&mut self) {}

    fn bump(&mut self) -> bool {
        let Some(i) = self.pics.iter().enumerate().filter(|(_, p)| p.needed_for_output).min_by_key(|(_, p)| p.poc()).map(|(i, _)| i) else {
            return false;
        };
        let p = &mut self.pics[i];
        p.needed_for_output = false;
        let f = OutFrame { surface: p.surface, poc: p.poc(), pts: p.pts };
        if p.mark == RefMark::Unused {
            self.pics.remove(i);
        }
        self.ready.push_back(f);
        true
    }

    fn remove_unused(&mut self) {
        self.pics.retain(|p| p.needed_for_output || p.mark != RefMark::Unused);
    }

    /// End of the stream: everything waiting is output.
    pub fn flush(&mut self) {
        while self.bump() {}
        self.remove_unused();
    }

    fn waiting(&self) -> usize {
        self.pics.iter().filter(|p| p.needed_for_output).count()
    }

    /// Picture order count of the picture about to be decoded. `None` when it is not decoded
    /// (the stream does not start with an IDR or intra picture yet). Frame number gaps are
    /// reported as unsupported.
    pub fn start_picture(&mut self, sh: &SliceHeader, sps: &Sps) -> Result<Option<Started>> {
        let idr = sh.is_idr();
        if !self.started && !idr && !sh.is_intra() {
            return Ok(None);
        }
        let max_frame_num = 1i32 << sps.log2_max_frame_num;
        if self.started && !idr && sh.frame_num != self.prev_ref_frame_num && sh.frame_num != (self.prev_ref_frame_num + 1) % max_frame_num as u32 {
            return Err(Error::Unsupported("H.264 frame_num gap (lost or dropped pictures)"));
        }
        let frame_num_offset = if idr {
            0
        } else if self.prev_frame_num > sh.frame_num {
            self.prev_frame_num_offset + max_frame_num
        } else {
            self.prev_frame_num_offset
        };
        let (top, bottom, msb) = match sps.poc_type {
            0 => {
                let (prev_msb, prev_lsb) = if idr || !self.started { (0, 0) } else { (self.prev_poc_msb, self.prev_poc_lsb) };
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = sh.poc_lsb as i32;
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb + max_lsb
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb - max_lsb
                } else {
                    prev_msb
                };
                let top = msb + lsb;
                (top, top + sh.delta_poc_bottom, msb)
            }
            1 => {
                let n = sps.offset_for_ref_frame.len() as i32;
                let mut abs = if n != 0 { frame_num_offset + sh.frame_num as i32 } else { 0 };
                if sh.nal_ref_idc == 0 && abs > 0 {
                    abs -= 1;
                }
                let mut expected = 0i32;
                if abs > 0 {
                    let per_cycle: i32 = sps.offset_for_ref_frame.iter().sum();
                    let cycle = (abs - 1) / n;
                    let in_cycle = ((abs - 1) % n) as usize;
                    expected = cycle * per_cycle + sps.offset_for_ref_frame[..=in_cycle].iter().sum::<i32>();
                }
                if sh.nal_ref_idc == 0 {
                    expected += sps.offset_for_non_ref_pic;
                }
                let top = expected + sh.delta_poc[0];
                (top, top + sps.offset_for_top_to_bottom_field + sh.delta_poc[1], 0)
            }
            _ => {
                let t = if idr {
                    0
                } else if sh.nal_ref_idc == 0 {
                    2 * (frame_num_offset + sh.frame_num as i32) - 1
                } else {
                    2 * (frame_num_offset + sh.frame_num as i32)
                };
                (t, t, 0)
            }
        };
        if idr {
            // nothing of the earlier pictures is used for reference
            for p in &mut self.pics {
                p.mark = RefMark::Unused;
            }
        }
        Ok(Some(Started { top_poc: top, bottom_poc: bottom, frame_num_offset, poc_msb: msb }))
    }

    /// The picture is decoded into `surface`: it is marked, joins the buffer, and the pictures
    /// that are due come out.
    pub fn finish_picture(&mut self, surface: usize, st: &Started, sh: &SliceHeader, sps: &Sps, pts: i64) -> Result<()> {
        let idr = sh.is_idr();
        let is_ref = sh.nal_ref_idc != 0;
        let max_frame_num = 1i32 << sps.log2_max_frame_num;
        let wrap = |p: &Pic| -> i32 {
            if p.frame_num > sh.frame_num { p.frame_num as i32 - max_frame_num } else { p.frame_num as i32 }
        };
        let (mut top, mut bottom) = (st.top_poc, st.bottom_poc);
        let mut mmco5 = false;
        let mut cur_long: Option<u32> = None;
        if is_ref {
            if idr {
                if sh.long_term_reference {
                    cur_long = Some(0);
                }
            } else if !sh.adaptive_marking {
                // sliding window (8.2.5.3)
                let max_refs = (sps.max_num_ref_frames as usize).max(1);
                while self.pics.iter().filter(|p| p.mark != RefMark::Unused).count() >= max_refs {
                    let Some(i) = self.pics.iter().enumerate().filter(|(_, p)| p.mark == RefMark::Short).min_by_key(|(_, p)| wrap(p)).map(|(i, _)| i) else { break };
                    self.pics[i].mark = RefMark::Unused;
                }
            } else {
                let curr_pic_num = sh.frame_num as i32;
                for op in &sh.mmco {
                    match *op {
                        Mmco::ShortUnused { diff_minus1 } => {
                            let x = curr_pic_num - (diff_minus1 as i32 + 1);
                            if let Some(i) = self.pics.iter().position(|p| p.mark == RefMark::Short && wrap(p) == x) {
                                self.pics[i].mark = RefMark::Unused;
                            }
                        }
                        Mmco::LongUnused { long_term_pic_num } => {
                            if let Some(i) = self.pics.iter().position(|p| p.mark == RefMark::Long && p.long_idx == long_term_pic_num) {
                                self.pics[i].mark = RefMark::Unused;
                            }
                        }
                        Mmco::ShortToLong { diff_minus1, long_term_frame_idx } => {
                            let x = curr_pic_num - (diff_minus1 as i32 + 1);
                            let target = self.pics.iter().position(|p| p.mark == RefMark::Short && wrap(p) == x);
                            for (i, p) in self.pics.iter_mut().enumerate() {
                                if p.mark == RefMark::Long && p.long_idx == long_term_frame_idx && Some(i) != target {
                                    p.mark = RefMark::Unused;
                                }
                            }
                            if let Some(i) = target {
                                self.pics[i].mark = RefMark::Long;
                                self.pics[i].long_idx = long_term_frame_idx;
                            }
                        }
                        Mmco::MaxLongIdx { plus1 } => {
                            for p in &mut self.pics {
                                if p.mark == RefMark::Long && p.long_idx + 1 > plus1 {
                                    p.mark = RefMark::Unused;
                                }
                            }
                        }
                        Mmco::Clear => {
                            for p in &mut self.pics {
                                p.mark = RefMark::Unused;
                            }
                            mmco5 = true;
                        }
                        Mmco::CurrentToLong { long_term_frame_idx } => {
                            for p in &mut self.pics {
                                if p.mark == RefMark::Long && p.long_idx == long_term_frame_idx {
                                    p.mark = RefMark::Unused;
                                }
                            }
                            cur_long = Some(long_term_frame_idx);
                        }
                    }
                }
            }
        }
        if mmco5 {
            let t = top.min(bottom);
            top -= t;
            bottom -= t;
        }

        // what the next picture's order count and frame number build on
        self.prev_frame_num = if mmco5 { 0 } else { sh.frame_num };
        self.prev_frame_num_offset = if mmco5 { 0 } else { st.frame_num_offset };
        if is_ref {
            self.prev_ref_frame_num = if mmco5 { 0 } else { sh.frame_num };
            if mmco5 {
                self.prev_poc_msb = 0;
                self.prev_poc_lsb = top;
            } else {
                self.prev_poc_msb = st.poc_msb;
                self.prev_poc_lsb = sh.poc_lsb as i32;
            }
        }

        // output (C.4.4, C.4.5)
        if idr || mmco5 {
            if idr && sh.no_output_of_prior_pics && self.started {
                self.pics.clear();
                self.ready.clear();
            } else {
                self.flush();
            }
        }
        self.started = true;
        self.remove_unused();
        let cur = Pic {
            surface,
            frame_num: if mmco5 { 0 } else { sh.frame_num },
            top_poc: top,
            bottom_poc: bottom,
            pts,
            mark: if !is_ref {
                RefMark::Unused
            } else if cur_long.is_some() {
                RefMark::Long
            } else {
                RefMark::Short
            },
            long_idx: cur_long.unwrap_or(0),
            needed_for_output: true,
        };
        let size = sps.dpb_size();
        while self.pics.len() >= size {
            if !is_ref && self.pics.iter().filter(|p| p.needed_for_output).all(|p| cur.poc() < p.poc()) {
                // a non-reference picture that comes before everything waiting goes straight out
                self.ready.push_back(OutFrame { surface, poc: cur.poc(), pts });
                return Ok(());
            }
            if !self.bump() {
                return Err(Error::Invalid("decoded picture buffer overflow"));
            }
        }
        self.pics.push(cur);
        if let Some(reorder) = sps.max_num_reorder_frames {
            while self.waiting() > reorder as usize && self.bump() {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::nal_type;

    fn sps() -> Sps {
        Sps { log2_max_frame_num: 4, log2_max_poc_lsb: 4, max_num_ref_frames: 2, width_mbs: 40, height_mbs: 23, level_idc: 30, max_dec_frame_buffering: Some(3), max_num_reorder_frames: Some(1), ..Default::default() }
    }

    fn hdr(idr: bool, ref_idc: u8, frame_num: u32, lsb: u32) -> SliceHeader {
        SliceHeader { nal_unit_type: if idr { nal_type::IDR } else { 1 }, nal_ref_idc: ref_idc, frame_num, poc_lsb: lsb, slice_type: if idr { 2 } else { 0 }, ..Default::default() }
    }

    fn run(d: &mut Dpb, sps: &Sps, sh: &SliceHeader, out: &mut Vec<i32>) {
        let st = d.start_picture(sh, sps).unwrap().unwrap();
        let s = d.alloc().unwrap();
        d.finish_picture(s, &st, sh, sps, 0).unwrap();
        while let Some(f) = d.next_output() {
            out.push(f.poc);
            d.release_held();
        }
    }

    #[test]
    fn b_frames_come_out_in_order_and_poc_wraps() {
        let sps = sps();
        let mut d = Dpb::new(8);
        let mut out = Vec::new();
        run(&mut d, &sps, &hdr(true, 3, 0, 0), &mut out);
        // decode order: P (poc 2n), non-reference B (poc 2n-1), ...: 40 pairs wrap the 4-bit lsb
        let mut frame_num = 0;
        for n in 1..40u32 {
            frame_num = (frame_num + 1) % 16;
            run(&mut d, &sps, &hdr(false, 2, frame_num, (4 * n) & 15), &mut out);
            run(&mut d, &sps, &hdr(false, 0, (frame_num + 1) % 16, (4 * n - 2) & 15), &mut out);
        }
        d.flush();
        while let Some(f) = d.next_output() {
            out.push(f.poc);
            d.release_held();
        }
        let want: Vec<i32> = std::iter::once(0).chain((1..40).flat_map(|n| [4 * n - 2, 4 * n])).collect();
        assert_eq!(out, want);
    }

    #[test]
    fn sliding_window_keeps_two_references() {
        let sps = sps();
        let mut d = Dpb::new(8);
        let mut out = Vec::new();
        run(&mut d, &sps, &hdr(true, 3, 0, 0), &mut out);
        for n in 1..6u32 {
            run(&mut d, &sps, &hdr(false, 2, n, 2 * n), &mut out);
            assert!(d.ref_pics().len() <= 2);
        }
        let nums: Vec<u32> = d.ref_pics().iter().map(|p| p.frame_num).collect();
        assert!(nums.contains(&5) && nums.contains(&4), "{nums:?}");
    }

    #[test]
    fn frame_num_gap_is_reported() {
        let sps = sps();
        let mut d = Dpb::new(8);
        let mut out = Vec::new();
        run(&mut d, &sps, &hdr(true, 3, 0, 0), &mut out);
        assert!(d.start_picture(&hdr(false, 2, 3, 6), &sps).is_err());
    }

    #[test]
    fn poc_type_1_follows_the_cycle() {
        // one reference frame per cycle, 2 apart; non-reference pictures sit 1 before
        let sps = Sps { poc_type: 1, offset_for_ref_frame: vec![2], offset_for_non_ref_pic: -1, ..sps() };
        let mut d = Dpb::new(8);
        let mut out = Vec::new();
        run(&mut d, &sps, &hdr(true, 3, 0, 0), &mut out);
        let st = d.start_picture(&hdr(false, 2, 1, 0), &sps).unwrap().unwrap();
        assert_eq!((st.top_poc, st.bottom_poc), (2, 2));
        let sh = hdr(false, 2, 1, 0);
        let s = d.alloc().unwrap();
        d.finish_picture(s, &st, &sh, &sps, 0).unwrap();
        // a non-reference picture with frame_num 2: absFrameNum 1 after the decrement
        let st = d.start_picture(&hdr(false, 0, 2, 0), &sps).unwrap().unwrap();
        assert_eq!(st.top_poc, 2 - 1);
    }

    #[test]
    fn long_term_marking_and_mmco5() {
        let sps = Sps { max_num_ref_frames: 3, ..sps() };
        let mut d = Dpb::new(8);
        let mut out = Vec::new();
        run(&mut d, &sps, &hdr(true, 3, 0, 0), &mut out);
        run(&mut d, &sps, &hdr(false, 2, 1, 2), &mut out);
        // frame 2 turns the IDR picture (CurrPicNum 2 - 2) into long-term index 0 and itself too
        let mut sh = hdr(false, 2, 2, 4);
        sh.adaptive_marking = true;
        sh.mmco = vec![Mmco::MaxLongIdx { plus1: 2 }, Mmco::ShortToLong { diff_minus1: 1, long_term_frame_idx: 0 }, Mmco::CurrentToLong { long_term_frame_idx: 1 }];
        run(&mut d, &sps, &sh, &mut out);
        let marks: Vec<(u32, RefMark, u32)> = d.ref_pics().iter().map(|p| (p.frame_num, p.mark, p.long_idx)).collect();
        assert!(marks.contains(&(0, RefMark::Long, 0)), "{marks:?}");
        assert!(marks.contains(&(1, RefMark::Short, 0)), "{marks:?}");
        assert!(marks.contains(&(2, RefMark::Long, 1)), "{marks:?}");
        // MMCO 4 with a smaller maximum drops index 1
        let mut sh = hdr(false, 2, 3, 6);
        sh.adaptive_marking = true;
        sh.mmco = vec![Mmco::MaxLongIdx { plus1: 1 }];
        run(&mut d, &sps, &sh, &mut out);
        assert!(!d.ref_pics().iter().any(|p| p.mark == RefMark::Long && p.long_idx == 1));
        // MMCO 5: everything out, the picture itself restarts at order count 0 and frame number 0
        let mut sh = hdr(false, 2, 4, 8);
        sh.adaptive_marking = true;
        sh.mmco = vec![Mmco::Clear];
        let before = out.len();
        run(&mut d, &sps, &sh, &mut out);
        assert!(out.len() > before);
        let refs = d.ref_pics();
        assert_eq!(refs.len(), 1);
        assert_eq!((refs[0].frame_num, refs[0].poc()), (0, 0));
    }
}
