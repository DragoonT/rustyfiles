//! Windowless probe: what it costs to hand a place to a RUNNING rustyfiles, by the two ways the desktop can do it.
//!
//! It plays the running rustyfiles itself: it takes the single-instance mutex of a test instance and serves its pipe, so that no window
//! of any program is made and nothing of the user's is reached. Then, `RUNS` times each:
//!
//! * PIPE: writes the line to the pipe from this process (what the shell host does now). Timed to the line arriving.
//! * PROCESS: starts `rustyfiles.exe <argument>` (what the host did before). That copy finds the mutex held, forwards its line over
//!   the pipe and ends without a window. Timed to the line arriving, and to the process ending.
//!
//! Run it with a test instance, a settings folder of its own and a temp folder of its own (the started copy tidies its temp folder
//! at start, so it is pointed at an empty one):
//!
//! ```text
//! set RUSTYFILES_INSTANCE=probe-open
//! set RUSTYFILES_CONFIG_DIR=<an empty temp folder>
//! set TEMP=<another empty temp folder>
//! cargo run --offline -p rf-win --example open_handover_probe -- <path of rustyfiles.exe> [runs]
//! ```
//!
//! Nothing is written outside those folders and the registry is not touched.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use rf_win::single_instance::{Role, acquire_app, instance_name, send_to, serve_app};

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(exe) = args.first() else {
        eprintln!("usage: open_handover_probe <rustyfiles.exe> [runs]");
        std::process::exit(2);
    };
    if std::env::var_os("RUSTYFILES_INSTANCE").is_none() || std::env::var_os("RUSTYFILES_CONFIG_DIR").is_none() {
        eprintln!("refusing to run: RUSTYFILES_INSTANCE and RUSTYFILES_CONFIG_DIR must be set, so that the user's own rustyfiles is never reached");
        std::process::exit(2);
    }
    let runs: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let app = instance_name();
    let _guard = match acquire_app(&app, "") {
        Role::Primary(g) => g,
        Role::Forwarded => {
            eprintln!("another copy of this test instance runs: stop it first");
            std::process::exit(1);
        }
    };
    let (tx, rx) = mpsc::channel::<(String, Instant)>();
    let tx = std::sync::Mutex::new(tx);
    serve_app(&app, move |m| {
        let _ = tx.lock().unwrap().send((m, Instant::now()));
    });
    // wait for the pipe to be up
    let up = Instant::now();
    while !send_to(&app, "probe: hello") {
        assert!(up.elapsed() < Duration::from_secs(3), "the pipe never came up");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = rx.recv_timeout(Duration::from_secs(3));

    let (mut pipe_ms, mut proc_msg_ms, mut proc_exit_ms) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..runs {
        // PIPE
        let t0 = Instant::now();
        assert!(send_to(&app, "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), "pipe write failed");
        let (_, at) = rx.recv_timeout(Duration::from_secs(3)).expect("the line did not arrive");
        pipe_ms.push(at.duration_since(t0).as_secs_f64() * 1000.0);

        // PROCESS
        let t0 = Instant::now();
        let mut child = std::process::Command::new(exe)
            .arg("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("could not start the program");
        let got = rx.recv_timeout(Duration::from_secs(30));
        let msg_at = got.as_ref().map(|(_, at)| at.duration_since(t0).as_secs_f64() * 1000.0).unwrap_or(f64::NAN);
        let status = child.wait().expect("wait");
        let exit_at = t0.elapsed().as_secs_f64() * 1000.0;
        println!("run {i}: pipe {:.2} ms | process: line after {:.0} ms, ended after {:.0} ms ({status})", pipe_ms[i], msg_at, exit_at);
        proc_msg_ms.push(msg_at);
        proc_exit_ms.push(exit_at);
    }
    println!(
        "median over {runs}: pipe write to line arriving {:.2} ms; started copy: line arriving {:.0} ms, process ended {:.0} ms",
        median(&mut pipe_ms),
        median(&mut proc_msg_ms),
        median(&mut proc_exit_ms)
    );
}
