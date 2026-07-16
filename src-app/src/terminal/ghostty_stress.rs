use std::time::{Duration, Instant};

use paneflow_config::schema::TerminalSurfaceProfile;

use super::ghostty_session::{GhosttySession, GhosttyUiEvent};
use super::pty_session::SpawnParams;
use super::types::{ShellQuoting, TerminalWindowSize};

const CYCLES: usize = 200;
const RESIZES_PER_CYCLE: usize = 200;
const CYCLE_TIMEOUT: Duration = Duration::from_secs(8);

fn run_cycle(surface_id: u64) {
    let params = SpawnParams {
        shell: "/bin/sh".into(),
        shell_quoting: ShellQuoting::Posix,
        extra_args: vec![
            "-c".into(),
            "IFS= read -r line; printf 'PANEFLOW_STRESS:%s\\n' \"$line\"".into(),
        ],
        env: std::collections::HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "paneflow".into()),
        ]),
        cwd: std::env::current_dir().expect("stress cwd"),
        cols: 80,
        rows: 24,
        profile: TerminalSurfaceProfile::Normal,
        surface_id,
    };
    let (session, pending, mut events) =
        GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
    let spawned = session
        .start(pending, params, None, 10_000)
        .expect("stress PTY starts");
    let pid = spawned.child_pid;
    assert!(pid > 0, "stress child must expose its PID");
    session.promote();
    for index in 0..RESIZES_PER_CYCLE {
        session.resize(TerminalWindowSize::new(
            1 + index % 160,
            1 + index % 80,
            8,
            16,
        ));
    }
    assert!(session.write(format!("cycle-{surface_id}\r").into_bytes()));

    let deadline = Instant::now() + CYCLE_TIMEOUT;
    let mut exits = 0;
    while Instant::now() < deadline && exits == 0 {
        while let Ok(event) = events.try_recv() {
            match event {
                GhosttyUiEvent::ChildExited { .. } => exits += 1,
                GhosttyUiEvent::RuntimeFailed(error) => {
                    panic!("Ghostty stress runtime failed: {error}")
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(exits, 1, "stress child must publish one exit");
    let (content, _) =
        session.render_content(TerminalWindowSize::new(40, 40, 8, 16), -100, 100, false);
    let rendered = content.cells.iter().map(|cell| cell.c).collect::<String>();
    assert!(rendered.contains(&format!("PANEFLOW_STRESS:cycle-{surface_id}")));
    session.shutdown();

    // SAFETY: signal 0 performs a read-only process existence check.
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "stress child must be reaped without a zombie"
    );
}

#[test]
#[ignore = "EP-005 promotion gate: 200 PTY cycles with 200 resizes each"]
fn ghostty_spawn_resize_close_stress_has_no_residual_growth() {
    for warmup in 0..5 {
        run_cycle(warmup);
    }
    let rss_start = super::backend_corpus::resident_set_bytes();
    for cycle in 0..CYCLES {
        run_cycle((cycle + 5) as u64);
    }
    let rss_end = super::backend_corpus::resident_set_bytes();
    let allowed = rss_start.saturating_add(rss_start / 20);
    assert!(
        rss_end <= allowed,
        "residual RSS grew beyond 5%: start={rss_start}, end={rss_end}, allowed={allowed}"
    );
}
