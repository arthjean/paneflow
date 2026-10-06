use super::*;

use paneflow_host::work_counters::{
    CounterSample, DESKTOP_COUNTERS, HOST_COUNTERS, WORKER_COUNTERS,
};

pub(super) const HARDWARE_STATES: [&str; 4] =
    ["idle-4-panes", "agent-thinking", "stream-4", "panes-8"];
pub(super) const HARDWARE_WINDOW: Duration = Duration::from_secs(60);
pub(super) const FRAME_TIME_COLUMNS: [&str; 3] = ["MsBetweenPresents", "FrameTime", "frametime"];

pub(super) fn not_measured(reason: impl Into<String>) -> Value {
    json!({"not_measured": reason.into()})
}

fn nearest_rank(sorted: &[f64], percent: f64) -> f64 {
    let rank = ((percent / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub(super) fn distribution(samples: &[f64], unit: &str) -> Value {
    if samples.is_empty() {
        return not_measured("the source produced no sample during the window");
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({
        "unit": unit,
        "samples": sorted.len(),
        "p50": nearest_rank(&sorted, 50.0),
        "p95": nearest_rank(&sorted, 95.0),
        "max": sorted[sorted.len() - 1],
    })
}

pub(super) fn parse_nvidia_smi(text: &str) -> BTreeMap<String, Vec<f64>> {
    let mut devices = BTreeMap::<String, Vec<f64>>::new();
    for line in text.lines() {
        let mut fields = line.split(',').map(str::trim);
        let (Some(index), Some(busy)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let Ok(busy) = busy.parse::<f64>() {
            devices
                .entry(format!("nvidia-smi gpu{index}"))
                .or_default()
                .push(busy);
        }
    }
    devices
}

fn csv_fields(line: &str) -> Vec<String> {
    line.split(',')
        .map(|field| field.trim().trim_matches('"').to_string())
        .collect()
}

pub(super) fn parse_typeperf(text: &str, pid: u32) -> Vec<f64> {
    let mut lines = text.lines().filter(|line| line.starts_with('"'));
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let instance = format!("pid_{pid}_");
    let columns: Vec<usize> = csv_fields(header)
        .iter()
        .enumerate()
        .filter(|(_, name)| name.contains(&instance) && name.contains("engtype_3D"))
        .map(|(index, _)| index)
        .collect();
    if columns.is_empty() {
        return Vec::new();
    }
    lines
        .filter_map(|line| {
            let fields = csv_fields(line);
            let values: Vec<f64> = columns
                .iter()
                .filter_map(|index| fields.get(*index)?.parse::<f64>().ok())
                .collect();
            (!values.is_empty()).then(|| values.iter().sum())
        })
        .collect()
}

pub(super) fn parse_frame_log(text: &str) -> Result<Vec<f64>, String> {
    let mut lines = text.lines();
    let column = loop {
        let Some(line) = lines.next() else {
            return Err(format!(
                "no header with a frame time column ({})",
                FRAME_TIME_COLUMNS.join(", ")
            ));
        };
        let fields = csv_fields(line);
        if let Some(index) = fields
            .iter()
            .position(|field| FRAME_TIME_COLUMNS.contains(&field.as_str()))
        {
            break index;
        }
    };
    Ok(lines
        .filter_map(|line| csv_fields(line).get(column)?.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect())
}

#[cfg(any(target_os = "linux", windows))]
fn run_for(program: &str, args: &[String], window: Duration) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{program} is unavailable: {error}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + window;
    let mut status = None;
    while Instant::now() < deadline {
        if let Some(exited) = child.try_wait().map_err(|error| error.to_string())? {
            status = Some(exited);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let text = reader.join().unwrap_or_default();
    match status {
        Some(exited) if !exited.success() && text.trim().is_empty() => {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            Err(format!("{program} exited with {exited}: {}", stderr.trim()))
        }
        _ => Ok(text),
    }
}

#[cfg(any(target_os = "linux", windows))]
fn source(samples: Result<Vec<f64>, String>) -> Value {
    match samples {
        Ok(samples) if samples.is_empty() => not_measured("the tool produced no sample"),
        Ok(samples) => distribution(&samples, "percent busy"),
        Err(reason) => not_measured(reason),
    }
}

#[cfg(target_os = "linux")]
fn gpu_sources(_desktop_pid: u32, window: Duration) -> BTreeMap<String, Value> {
    let seconds = window.as_secs().max(1);
    let nvidia = std::thread::spawn(move || {
        let args = [
            "--query-gpu=index,utilization.gpu",
            "--format=csv,noheader,nounits",
            "-lms",
            "1000",
        ]
        .map(String::from);
        run_for("nvidia-smi", &args, window).map(|text| parse_nvidia_smi(&text))
    });
    let amd_cards: Vec<PathBuf> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("card"))
        .map(|entry| entry.path().join("device/gpu_busy_percent"))
        .filter(|path| path.is_file())
        .collect();
    let mut amd = vec![Vec::new(); amd_cards.len()];
    for _ in 0..seconds {
        for (card, samples) in amd_cards.iter().zip(amd.iter_mut()) {
            if let Some(busy) = std::fs::read_to_string(card)
                .ok()
                .and_then(|text| text.trim().parse::<f64>().ok())
            {
                samples.push(busy);
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let mut sources = BTreeMap::new();
    match nvidia
        .join()
        .unwrap_or_else(|_| Err("the nvidia-smi reader panicked".to_string()))
    {
        Ok(devices) if devices.is_empty() => {
            sources.insert(
                "nvidia-smi".to_string(),
                not_measured("nvidia-smi reported no device"),
            );
        }
        Ok(devices) => {
            for (device, samples) in devices {
                sources.insert(device, source(Ok(samples)));
            }
        }
        Err(reason) => {
            sources.insert("nvidia-smi".to_string(), not_measured(reason));
        }
    }
    if amd_cards.is_empty() {
        sources.insert(
            "amdgpu gpu_busy_percent".to_string(),
            not_measured("no /sys/class/drm/card*/device/gpu_busy_percent: no amdgpu device"),
        );
    }
    for (card, samples) in amd_cards.iter().zip(amd) {
        sources.insert(format!("amdgpu {}", card.display()), source(Ok(samples)));
    }
    sources
}

#[cfg(windows)]
fn gpu_sources(desktop_pid: u32, window: Duration) -> BTreeMap<String, Value> {
    let args = [
        r"\GPU Engine(*)\Utilization Percentage".to_string(),
        "-si".to_string(),
        "1".to_string(),
        "-sc".to_string(),
        window.as_secs().max(1).to_string(),
    ];
    let samples = run_for("typeperf", &args, window + Duration::from_secs(15))
        .map(|text| parse_typeperf(&text, desktop_pid));
    BTreeMap::from([(
        "typeperf GPU Engine 3D (desktop process)".to_string(),
        source(samples),
    )])
}

#[cfg(not(any(target_os = "linux", windows)))]
fn gpu_sources(_desktop_pid: u32, _window: Duration) -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "gpu".to_string(),
        not_measured("no GPU utilization source is wired for this platform"),
    )])
}

fn frame_times() -> Value {
    let Some(path) = std::env::var_os("PANEFLOW_HW_FRAME_LOG") else {
        return not_measured(if cfg!(target_os = "macos") {
            "no frame time source on macOS"
        } else {
            "no frame log given: set PANEFLOW_HW_FRAME_LOG to a MangoHud (Linux) or PresentMon (Windows) CSV of the window"
        });
    };
    match std::fs::read_to_string(&path) {
        Err(error) => not_measured(format!(
            "{} is unreadable: {error}",
            Path::new(&path).display()
        )),
        Ok(text) => match parse_frame_log(&text) {
            Ok(samples) => {
                let mut value = distribution(&samples, "ms");
                value["log"] = json!(Path::new(&path).display().to_string());
                value
            }
            Err(reason) => not_measured(format!("{}: {reason}", Path::new(&path).display())),
        },
    }
}

fn role_pid(variable: &str) -> Option<u32> {
    std::env::var(variable)
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
}

fn counter_sample(role: &str) -> Option<CounterSample> {
    match role {
        "desktop" => {
            let endpoint = std::env::var_os("PANEFLOW_HW_DESKTOP_ENDPOINT")
                .map(PathBuf::from)
                .or_else(|| paneflow_home::ipc_endpoint().map(|endpoint| endpoint.path))?;
            Some(
                match IpcClient::new(endpoint).call("system.counters", json!({})) {
                    Ok(result) => {
                        paneflow_host::work_counters::sample("desktop", &result, DESKTOP_COUNTERS)
                    }
                    Err(error) => pending_sample(
                        "desktop",
                        DESKTOP_COUNTERS,
                        format!("system.counters failed: {error:?}"),
                    ),
                },
            )
        }
        "host" => {
            let endpoint = paneflow_home::host_endpoint_path_for_current_home()?;
            Some(
                match HostClient::connect(&endpoint, &ClientHello::control("perf-hardware"))
                    .map_err(|error| error.to_string())
                    .and_then(|mut client| {
                        client
                            .call("host.status", json!({}))
                            .map_err(|error| error.to_string())
                    }) {
                    Ok(status) => {
                        paneflow_host::work_counters::sample("host", &status, HOST_COUNTERS)
                    }
                    Err(error) => pending_sample(
                        "host",
                        HOST_COUNTERS,
                        format!("host.status failed: {error}"),
                    ),
                },
            )
        }
        _ => {
            let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
            Some(
                match HostControl::connect_with_deadline(
                    &endpoint,
                    "perf-hardware",
                    Duration::from_secs(2),
                )
                .map_err(|error| error.to_string())
                .and_then(|mut control| {
                    control
                        .request("worker.status", json!({}))
                        .map_err(|error| error.to_string())
                }) {
                    Ok(status) => {
                        paneflow_host::work_counters::sample("worker", &status, WORKER_COUNTERS)
                    }
                    Err(error) => pending_sample(
                        "worker",
                        WORKER_COUNTERS,
                        format!("worker.status failed: {error}"),
                    ),
                },
            )
        }
    }
}

fn pending_sample(process: &str, names: &[&str], reason: String) -> CounterSample {
    CounterSample {
        process: process.to_string(),
        identity: None,
        readings: names
            .iter()
            .map(|name| {
                (
                    name.to_string(),
                    paneflow_host::work_counters::Reading::Pending(reason.clone()),
                )
            })
            .collect(),
    }
}

pub(super) fn display_backend(environment: &[u8]) -> &'static str {
    let set = |name: &str| {
        environment.split(|byte| *byte == 0).any(|entry| {
            entry
                .strip_prefix(name.as_bytes())
                .and_then(|rest| rest.strip_prefix(b"="))
                .is_some_and(|value| !value.is_empty())
        })
    };
    if set("WAYLAND_DISPLAY") {
        "wayland"
    } else if set("DISPLAY") {
        "x11"
    } else {
        "unknown"
    }
}

pub(super) fn session_type() -> String {
    if !cfg!(target_os = "linux") {
        return std::env::consts::OS.to_string();
    }
    std::env::var("PANEFLOW_HW_DESKTOP_PID")
        .ok()
        .and_then(|pid| std::fs::read(format!("/proc/{}/environ", pid.trim())).ok())
        .map_or_else(
            || "unknown".to_string(),
            |environment| display_backend(&environment).to_string(),
        )
}

pub(super) fn measure(state: &str, label: &str, window: Duration) -> Value {
    let roles = [
        ("desktop", role_pid("PANEFLOW_HW_DESKTOP_PID")),
        ("host", role_pid("PANEFLOW_HW_HOST_PID")),
        ("worker", role_pid("PANEFLOW_HW_WORKER_PID")),
    ];
    let desktop_pid = roles[0]
        .1
        .expect("PANEFLOW_HW_DESKTOP_PID names the desktop to measure");
    let counters_before: Vec<Option<CounterSample>> = roles
        .iter()
        .map(|(role, pid)| pid.and_then(|_| counter_sample(role)))
        .collect();
    let cpu_before: Vec<Option<Attribution>> =
        roles.iter().map(|(_, pid)| pid.map(thread_cpu)).collect();
    let started = Instant::now();
    let gpu = gpu_sources(desktop_pid, window);
    std::thread::sleep(window.saturating_sub(started.elapsed()));
    let elapsed = started.elapsed();
    let mut processes = serde_json::Map::new();
    for (index, (role, pid)) in roles.iter().enumerate() {
        let Some(pid) = pid else {
            processes.insert(
                role.to_string(),
                not_measured(format!("no paneflow {role} process was running")),
            );
            continue;
        };
        let after = thread_cpu(*pid);
        let mut sample = process_sample(
            *pid,
            cpu_before[index]
                .as_ref()
                .expect("sampled before the window"),
            &after,
            elapsed,
        );
        let counters_after = counter_sample(role);
        sample["work_counters"] = match (&counters_before[index], &counters_after) {
            (Some(before), Some(after)) => active::window_json(before, after),
            _ => not_measured(format!("no {role} endpoint for the current PANEFLOW_HOME")),
        };
        processes.insert(role.to_string(), sample);
    }
    let root_renders = match &processes
        .get("desktop")
        .map(|desktop| &desktop["work_counters"]["root_renders"])
    {
        Some(Value::Number(count)) => json!(count),
        Some(Value::Object(reason)) if reason.contains_key("pending") => {
            not_measured(reason["pending"].as_str().unwrap_or("pending").to_string())
        }
        _ => not_measured("the desktop reported no root_renders counter"),
    };
    json!({
        "suite": "paneflow-hardware-protocol",
        "schema_version": 1,
        "platform": platform(),
        "session": session_type(),
        "state": state,
        "label": label,
        "version": std::env::var("PANEFLOW_HW_VERSION").ok(),
        "stamp": stamp(),
        "window_s": elapsed.as_secs_f64(),
        "machine": machine(),
        "processes": processes,
        "root_renders": root_renders,
        "gpu": gpu,
        "frame_time_ms": frame_times(),
        "note": "GPU percentages are device-wide except typeperf, which is the desktop process's 3D engines; a source that could not be read is recorded not_measured with its reason, never 0",
    })
}
