//! Context switches per second of every thread of a process (read only), from NtQuerySystemInformation(SystemProcessInformation):
//! `cargo run --release -p rf-win --example thread_wakes -- <pid> [seconds]`. A thread that waits on a timer or a channel and is woken
//! N times a second shows about N context switches a second. Thread names: see the thread_cpu example (same thread ids).

use std::time::{Duration, Instant};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> i32;
}

/// (thread id, context switches) of every thread of `pid`.
fn sample(pid: u64) -> Vec<(u64, u32)> {
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let mut ret = 0u32;
        let st = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr(), buf.len() as u32, &mut ret) };
        if st == 0xC0000004u32 as i32 {
            buf = vec![0u8; ret as usize + (1 << 20)];
            continue;
        }
        assert!(st >= 0, "NtQuerySystemInformation failed: {st:#x}");
        break;
    }
    let rd32 = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let rd64 = |o: usize| u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
    let mut off = 0usize;
    let mut out = vec![];
    loop {
        let next = rd32(off) as usize;
        let n = rd32(off + 4) as usize;
        if rd64(off + 80) == pid {
            for i in 0..n {
                let t = off + 256 + i * 80;
                out.push((rd64(t + 48), rd32(t + 64)));
            }
            break;
        }
        if next == 0 {
            break;
        }
        off += next;
    }
    out
}

fn main() {
    let pid: u64 = std::env::args().nth(1).expect("pid").parse().unwrap();
    let secs: f64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let (a, t0) = (sample(pid), Instant::now());
    std::thread::sleep(Duration::from_secs_f64(secs));
    let (b, el) = (sample(pid), t0.elapsed().as_secs_f64());
    let mut rows: Vec<(f64, u64)> = b
        .iter()
        .filter_map(|(id, c)| a.iter().find(|(i, _)| i == id).map(|(_, c0)| (c.wrapping_sub(*c0) as f64 / el, *id)))
        .collect();
    rows.sort_by(|x, y| y.0.total_cmp(&x.0));
    println!("{} threads, {el:.1} s between the two samples; context switches per second, top 12:", rows.len());
    for (r, id) in rows.iter().take(12) {
        println!("{r:>8.1}/s  thread {id}");
    }
}
