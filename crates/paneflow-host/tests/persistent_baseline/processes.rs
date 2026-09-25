use super::*;

pub(super) struct WorkerProcess {
    pub(super) child: std::process::Child,
    pub(super) endpoint: PathBuf,
}

impl WorkerProcess {
    pub(super) fn start(home: &Path) -> Option<Self> {
        let executable = std::env::var_os("PANEFLOW_BENCH_CONTROLLER")?;
        Some(Self::start_enabled(home, executable))
    }

    pub(super) fn start_enabled(home: &Path, executable: std::ffi::OsString) -> Self {
        let log = std::fs::File::create(home.join("worker-benchmark.log")).unwrap();
        let mut command = Command::new(executable);
        command
            .args(["serve", "run", "--home"])
            .arg(home)
            .env("PANEFLOW_HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().expect("benchmark worker starts");
        let mut worker = Self {
            child,
            endpoint: paneflow_home::serve_endpoint_path(home),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if HostControl::connect_with_deadline(
                &worker.endpoint,
                "persistent-bench",
                Duration::from_millis(500),
            )
            .is_ok()
            {
                return worker;
            }
            assert!(worker.child.try_wait().unwrap().is_none(), "worker exited");
            assert!(Instant::now() < deadline, "worker startup watchdog");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        if let Ok(mut control) = HostControl::connect_with_deadline(
            &self.endpoint,
            "persistent-bench",
            Duration::from_secs(2),
        ) {
            let _ = control.request("worker.shutdown", json!({}));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

pub(super) struct Follower {
    pub(super) stop: Arc<AtomicBool>,
    finished: std::sync::mpsc::Receiver<Result<paneflow_host::client::OutputEnd, String>>,
    pub(super) attach_ms: f64,
    pub(super) checkpoint_bytes: usize,
}

pub(super) struct DesktopProcess {
    pub(super) child: std::process::Child,
    endpoint: PathBuf,
    pub(super) restored_ms: f64,
    pub(super) surfaces: Value,
}

impl DesktopProcess {
    pub(super) fn start(home: &Path, sessions: &[SessionId]) -> Option<Self> {
        std::env::var_os("PANEFLOW_BENCH_DESKTOP")?;
        Some(Self::start_enabled(home, sessions))
    }

    pub(super) fn start_enabled(home: &Path, sessions: &[SessionId]) -> Self {
        let executable = std::env::var_os("PANEFLOW_BENCH_CONTROLLER")
            .expect("desktop benchmark requires PANEFLOW_BENCH_CONTROLLER");
        let workspaces: Vec<_> = sessions
            .chunks(32)
            .enumerate()
            .map(|(index, sessions)| {
                json!({
                    "title": format!("Persistent baseline {index}"),
                    "cwd": home.display().to_string(),
                    "tabs": sessions.iter().map(|session| json!({
                        "title": session.as_str(),
                        "layout": {"type": "pane", "surfaces": [{"surface_type": "terminal", "session": session, "custom_name": session.as_str()}]},
                    })).collect::<Vec<_>>(),
                    "active_tab": 0,
                })
            })
            .collect();
        let saved = json!({"version": 3, "active_workspace": 0, "workspaces": workspaces});
        let _: paneflow_config::schema::SessionState =
            serde_json::from_value(saved.clone()).unwrap();
        std::fs::write(
            home.join("session.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.join("paneflow.json"),
            br#"{"telemetry":{"enabled":false},"terminal":{"cursor_blink":"off"}}"#,
        )
        .unwrap();
        #[cfg(windows)]
        let endpoint = PathBuf::from(format!(
            r"\\.\pipe\paneflow-persistent-bench-{}",
            std::process::id()
        ));
        #[cfg(not(windows))]
        let endpoint = home.join("desktop.sock");
        let log_path = home.join(format!("desktop-{}.log", sessions.len()));
        let log = std::fs::File::create(&log_path).unwrap();
        let started = Instant::now();
        let child = Command::new(executable)
            .env("PANEFLOW_HOME", home)
            .env("PANEFLOW_SOCKET_PATH", &endpoint)
            .env("PANEFLOW_ALLOW_SOCKET_OVERRIDE", "1")
            .env(
                "PANEFLOW_UPDATE_FEED_URL",
                "http://127.0.0.1:9/fixture.json",
            )
            .env("RUST_LOG", "info")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("native desktop starts");
        let mut desktop = Self {
            child,
            endpoint: endpoint.clone(),
            restored_ms: 0.0,
            surfaces: Value::Null,
        };
        let ipc = IpcClient::new(endpoint.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut listed = 0;
        let mut ready_prefix = 0;
        let mut last_attempt = String::from("no attempt yet");
        loop {
            assert!(
                desktop.child.try_wait().unwrap().is_none(),
                "desktop exited\n{}",
                log_tail(&log_path)
            );
            assert!(
                Instant::now() < deadline,
                "desktop restoration watchdog: {listed} of {} surfaces listed, the first {ready_prefix} showing fixture idle; last attempt: {last_attempt}\n{}\n{}",
                sessions.len(),
                log_tail(&log_path),
                stall_samples(desktop.child.id(), home)
            );
            if !paneflow_ipc_client::socket_is_listening(&endpoint) {
                last_attempt = "the desktop socket is not listening".into();
            } else {
                match ipc.call("surface.list", json!({})) {
                    Err(error) => last_attempt = format!("surface.list failed: {error:?}"),
                    Ok(surfaces) => {
                        let mut complete = false;
                        if let Some(entries) = surfaces["surfaces"].as_array() {
                            listed = entries.len();
                            last_attempt = format!("surface.list answered {listed} entries");
                            if listed == sessions.len() {
                                ready_prefix = entries
                                    .iter()
                                    .take_while(|surface| {
                                        ipc.call(
                                            "surface.read",
                                            json!({"surface_id": surface["surface_id"], "lines": 24}),
                                        )
                                        .is_ok_and(|result| {
                                            result.to_string().contains("fixture idle")
                                        })
                                    })
                                    .count();
                                complete = ready_prefix == listed;
                            }
                        } else {
                            last_attempt = format!("surface.list returned {surfaces}");
                        }
                        if complete {
                            desktop.restored_ms = started.elapsed().as_secs_f64() * 1000.0;
                            desktop.surfaces = surfaces;
                            return desktop;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(target_os = "macos")]
fn stall_samples(desktop_pid: u32, home: &Path) -> String {
    let dir = std::env::var_os("PANEFLOW_BENCH_OUT")
        .map(PathBuf::from)
        .and_then(|out| out.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| home.to_path_buf());
    let related = Command::new("pgrep")
        .arg("-f")
        .arg(home)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let pids: Vec<u32> = std::iter::once(desktop_pid)
        .chain(
            related
                .split_whitespace()
                .filter_map(|pid| pid.parse().ok()),
        )
        .collect();
    let running: Vec<_> = pids
        .iter()
        .filter_map(|pid| {
            let path = dir.join(format!("watchdog-sample-{pid}.txt"));
            Command::new("sample")
                .arg(pid.to_string())
                .arg("3")
                .arg("-file")
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()
                .map(|child| (child, path))
        })
        .collect();
    let mut report = Vec::new();
    for (mut child, path) in running {
        let _ = child.wait();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        report.push(format!("== {}", path.display()));
        report.extend(
            text.lines()
                .filter(|line| line.starts_with("Process:"))
                .map(str::to_owned),
        );
        report.extend(
            text.lines()
                .skip_while(|line| !line.starts_with("Sort by top of stack"))
                .take(16)
                .map(str::to_owned),
        );
    }
    report.join("\n")
}

#[cfg(not(target_os = "macos"))]
fn stall_samples(_desktop_pid: u32, _home: &Path) -> String {
    String::new()
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let lines: Vec<_> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

impl Drop for DesktopProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn warm_desktop_panes(
    desktop: &DesktopProcess,
    client: &mut HostClient,
    sessions: &[SessionId],
) -> Value {
    let ipc = IpcClient::new(desktop.endpoint.clone());
    let surfaces = desktop.surfaces["surfaces"].as_array().unwrap();
    assert_eq!(surfaces.len(), sessions.len());
    let started = Instant::now();
    for (surface, session) in surfaces.iter().zip(sessions) {
        let focused = ipc
            .call(
                "surface.focus",
                json!({"surface_id": surface["surface_id"]}),
            )
            .unwrap();
        assert_eq!(focused["focused"], true);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observed = client.inspect(session).unwrap().manifest.launch;
            if observed.cols == 80 && observed.rows == 24 {
                break;
            }
            if Instant::now() >= deadline {
                return json!({"pending": "a restored pane did not adopt the calibrated 80x24 window", "session": session, "cols": observed.cols, "rows": observed.rows});
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    if let Some(first) = surfaces.first() {
        let focused = ipc
            .call("surface.focus", json!({"surface_id": first["surface_id"]}))
            .unwrap();
        assert_eq!(focused["focused"], true);
    }
    json!({"all_80x24": true, "panes": sessions.len(), "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0, "preparation": "each pane focused once through existing IPC; first pane restored visible before sampling"})
}

#[cfg(windows)]
pub(super) fn fit_desktop_grid(
    desktop: &DesktopProcess,
    client: &mut HostClient,
    session: &SessionId,
) -> Value {
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    struct Search {
        pid: u32,
        window: isize,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
        fn IsWindowVisible(window: isize) -> i32;
        fn GetWindowRect(window: isize, rect: *mut Rect) -> i32;
        fn SetWindowPos(
            window: isize,
            after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    unsafe extern "system" fn find(window: isize, data: isize) -> i32 {
        let search = unsafe { &mut *(data as *mut Search) };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
        }
        if pid == search.pid && unsafe { IsWindowVisible(window) } != 0 {
            search.window = window;
            0
        } else {
            1
        }
    }
    let mut search = Search {
        pid: desktop.child.id(),
        window: 0,
    };
    unsafe {
        EnumWindows(find, (&raw mut search) as isize);
    }
    if search.window == 0 {
        return json!({"pending": "the owned native window was not found"});
    }
    let mut width_range = (800, 1800);
    let mut height_range = (500, 1200);
    let started = Instant::now();
    for attempt in 0..20 {
        let current = client.inspect(session).unwrap().manifest.launch;
        let mut rect = Rect::default();
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(search.window, &mut pid);
        }
        assert_eq!(
            pid,
            desktop.child.id(),
            "only the owned desktop can be resized"
        );
        assert_ne!(unsafe { GetWindowRect(search.window, &mut rect) }, 0);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if current.cols == 80 && current.rows == 24 {
            std::thread::sleep(Duration::from_millis(250));
            let stable = client.inspect(session).unwrap().manifest.launch;
            if stable.cols == 80 && stable.rows == 24 {
                return json!({"result": "80x24 observed", "attempts": attempt, "window_width": width, "window_height": height, "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0});
            }
        }
        let next_dimension = |observed, target, current, range: &mut (i32, i32)| {
            if observed == target {
                return current;
            }
            if observed > target {
                range.1 = current - 1;
            } else {
                range.0 = current + 1;
            }
            (range.0 + (range.1 - range.0) / 2).clamp(1, 3000)
        };
        let next_width = next_dimension(current.cols, 80, width, &mut width_range);
        let next_height = next_dimension(current.rows, 24, height, &mut height_range);
        assert_ne!(
            unsafe { SetWindowPos(search.window, 0, 0, 0, next_width, next_height, 0x16) },
            0
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    json!({"pending": "80x24 window calibration did not converge within twenty bounded resize attempts"})
}

#[cfg(not(windows))]
pub(super) fn fit_desktop_grid(
    _desktop: &DesktopProcess,
    _client: &mut HostClient,
    _session: &SessionId,
) -> Value {
    json!({"pending": "automatic native window calibration is implemented only on Windows; observed dimensions are recorded"})
}

impl Follower {
    pub(super) fn attach(endpoint: &Path, session: &SessionId) -> Self {
        let mut client =
            HostClient::connect(endpoint, &ClientHello::local("persistent-follower")).unwrap();
        let started = Instant::now();
        let attachment = client.attach(session, None).unwrap();
        let attach_ms = started.elapsed().as_secs_f64() * 1000.0;
        let checkpoint_bytes = attachment.checkpoint.snapshot.len();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let (sender, finished) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(format!("baseline-follower-{session}"))
            .spawn(move || {
                let generation = attachment.checkpoint.generation;
                let offset = attachment.checkpoint.offset;
                let session = attachment.session.clone();
                drop(attachment);
                let result = client
                    .output(
                        &session,
                        Some(generation),
                        offset,
                        true,
                        |_, _| true,
                        || !stopping.load(Ordering::Acquire),
                    )
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            })
            .unwrap();
        Self {
            stop,
            finished,
            attach_ms,
            checkpoint_bytes,
        }
    }

    pub(super) fn finish(&self) {
        self.stop.store(true, Ordering::Release);
        self.finished
            .recv_timeout(Duration::from_secs(5))
            .expect("follower shutdown watchdog")
            .expect("follower completes");
    }
}

pub(super) fn paused_follower_probe(client: &mut HostClient, endpoint: &Path) -> Value {
    let session = client
        .create(&CreateSession {
            shell: Some(fixture_executable().display().to_string()),
            args: vec!["echo".to_string()],
            cols: Some(80),
            rows: Some(24),
            ..CreateSession::default()
        })
        .unwrap();
    let session_id = session.manifest.session;
    let mut follower =
        HostClient::connect(endpoint, &ClientHello::local("paused-follower")).unwrap();
    let attachment = follower.attach(&session_id, None).unwrap();
    let generation = attachment.checkpoint.generation;
    let offset = attachment.checkpoint.offset;
    drop(attachment);
    let (paused_tx, paused_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let followed_id = session_id.clone();
    std::thread::spawn(move || {
        let mut first = true;
        let result = follower.output(
            &followed_id,
            Some(generation),
            offset,
            true,
            |_, _| {
                if first {
                    first = false;
                    paused_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                false
            },
            || true,
        );
        let _ = done_tx.send(result.map_err(|error| error.to_string()));
    });
    client
        .input(&session_id, generation, b"paused-follower-probe\r")
        .unwrap();
    paused_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let pause_started = Instant::now();
    std::thread::sleep(Duration::from_millis(300));
    let control_started = Instant::now();
    assert!(client.inspect(&session_id).unwrap().live);
    let control_ms = control_started.elapsed().as_secs_f64() * 1000.0;
    let attach_started = Instant::now();
    client.attach(&session_id, Some(generation)).unwrap();
    let reattach_ms = attach_started.elapsed().as_secs_f64() * 1000.0;
    resume_tx.send(()).unwrap();
    let paused_ms = pause_started.elapsed().as_secs_f64() * 1000.0;
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    client.stop(&session_id, Some(generation)).unwrap();
    json!({
        "result": "pass",
        "watchdog_s": 5,
        "paused_ms": paused_ms,
        "independent_inspect_ms": control_ms,
        "independent_reattach_ms": reattach_ms,
        "scope": "one follower pauses receipt; independent real IPC inspect and attachment complete; no queue-capacity claim",
    })
}
