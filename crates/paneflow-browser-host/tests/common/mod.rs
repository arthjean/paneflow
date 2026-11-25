#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use paneflow_browser_client::{BrowserClient, ClientEvent, HostConfig};
use paneflow_browser_proto::{Event, ProfileName, Reply, TabId};

pub const DEADLINE: Duration = Duration::from_secs(60);

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub const FIXTURES: &[&str] = &["app.html", "form.html", "video.html", "injection.html"];

pub struct FixtureServer {
    pub port: u16,
}

impl FixtureServer {
    pub fn start() -> Self {
        Self::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
    }

    pub fn start_on_all_interfaces() -> Self {
        Self::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
    }

    fn bind(address: SocketAddr) -> Self {
        let listener = TcpListener::bind(address).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || serve(stream));
            }
        });
        Self { port }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn url_on(&self, host: IpAddr, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

const PERMISSIONS_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Permissions</title></head><body>
<script>
window.results = {};
navigator.geolocation.getCurrentPosition(() => results.geolocation = 'granted', e => results.geolocation = 'denied:' + e.code);
Notification.requestPermission().then(p => results.notifications = p);
navigator.mediaDevices.getUserMedia({video: true, audio: true}).then(() => results.media = 'granted', e => results.media = 'denied:' + e.name);
</script></body></html>"#;

const DOWNLOAD_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Download</title></head><body>
<a id="get" href="/test/file.bin">Get the file</a></body></html>"#;

const HIDDEN_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Hidden</title></head><body>
<button id="shown" type="button" onclick="document.getElementById('later').hidden=false">Show the late button</button>
<button id="later" type="button" hidden>Late button</button>
<div style="visibility:hidden"><button type="button">Invisible button</button></div>
</body></html>"#;

const AUTH_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Auth</title></head><body>auth
<script>fetch('/api/cart', {headers: {'Authorization': 'Bearer secret-token-123', 'X-Trace': 'visible-trace'}});</script></body></html>"#;

const SLOW_HEAD: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Slow page</title></head><body><h1>Arrived first</h1>"#;

pub const SLOW_PAGE_DELAY: Duration = Duration::from_secs(4);

fn big_page() -> String {
    let mut page = String::from(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Big</title></head><body>"#,
    );
    for index in 0..3000 {
        page.push_str(&format!(
            "<button type=\"button\">Button number {index}</button>"
        ));
    }
    page.push_str("</body></html>");
    page
}

fn respond_slowly(stream: &mut TcpStream) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(SLOW_HEAD.as_bytes());
    let _ = stream.flush();
    std::thread::sleep(SLOW_PAGE_DELAY);
    let _ = stream.write_all(b"<p>Arrived last</p></body></html>");
}

const COOKIE_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Cookies</title></head><body>cookies</body></html>"#;

fn serve(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header == "\r\n" || header.is_empty() {
            break;
        }
    }
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    match path {
        "/" => respond_file(&mut stream, "app.html"),
        "/api/cart" => respond(
            &mut stream,
            "200 OK",
            "application/json",
            "",
            br#"{"items":[{"name":"Mug","qty":2},{"name":"Poster","qty":1}]}"#,
        ),
        "/api/prices" => respond(
            &mut stream,
            "500 Internal Server Error",
            "application/json",
            "",
            br#"{"error":"price service unavailable"}"#,
        ),
        "/test/permissions" => respond(
            &mut stream,
            "200 OK",
            "text/html",
            "",
            PERMISSIONS_PAGE.as_bytes(),
        ),
        "/test/download" => respond(
            &mut stream,
            "200 OK",
            "text/html",
            "",
            DOWNLOAD_PAGE.as_bytes(),
        ),
        "/test/file.bin" => respond(
            &mut stream,
            "200 OK",
            "application/octet-stream",
            "Content-Disposition: attachment; filename=\"file.bin\"\r\n",
            &[7u8; 4096],
        ),
        "/test/hidden" => respond(
            &mut stream,
            "200 OK",
            "text/html",
            "",
            HIDDEN_PAGE.as_bytes(),
        ),
        "/test/auth" => respond(&mut stream, "200 OK", "text/html", "", AUTH_PAGE.as_bytes()),
        "/test/big" => respond(
            &mut stream,
            "200 OK",
            "text/html",
            "",
            big_page().as_bytes(),
        ),
        "/test/slow" => respond_slowly(&mut stream),
        "/test/redirect" => {
            let location = format!("Location: {query}\r\n");
            respond(&mut stream, "302 Found", "text/plain", &location, b"moved")
        }
        "/test/cookie" => {
            let set = if query.is_empty() {
                String::new()
            } else {
                format!("Set-Cookie: {query}; Path=/; Max-Age=86400\r\n")
            };
            respond(
                &mut stream,
                "200 OK",
                "text/html",
                &set,
                COOKIE_PAGE.as_bytes(),
            )
        }
        other => respond_file(&mut stream, other.trim_start_matches('/')),
    }
}

fn respond_file(stream: &mut TcpStream, name: &str) {
    let safe = !name.is_empty() && !name.contains("..") && !name.contains('/');
    let path = fixtures_dir().join(name);
    match std::fs::read(&path) {
        Ok(body) if safe => {
            let content_type = match path.extension().and_then(|ext| ext.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("webm") => "video/webm",
                _ => "application/octet-stream",
            };
            respond(stream, "200 OK", content_type, "", &body);
        }
        _ => respond(stream, "404 Not Found", "text/plain", "", b"not found"),
    }
}

pub fn http_get(port: u16, path: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, response[split + 4..].to_vec())
}

pub fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

pub fn host_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-browser-host"))
}

pub struct Harness {
    pub home: Option<tempfile::TempDir>,
    pub client: BrowserClient,
    pub events: Receiver<ClientEvent>,
    pub seen: Vec<ClientEvent>,
}

pub fn config(home: &Path) -> HostConfig {
    let mut config = HostConfig::for_home(home.to_path_buf());
    config.binary = host_binary();
    config.window_environment = std::env::vars_os()
        .filter(|(key, _)| key != "HOME")
        .chain([(OsString::from("HOME"), home.join("user").into_os_string())])
        .collect();
    std::fs::create_dir_all(home.join("user")).unwrap();
    config
}

impl Harness {
    pub fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let config = config(home.path());
        Self::with_config(home, config)
    }

    pub fn with_config(home: tempfile::TempDir, config: HostConfig) -> Self {
        let mut harness = Self::sharing(config);
        harness.home = Some(home);
        harness
    }

    pub fn sharing(config: HostConfig) -> Self {
        let (client, events) = BrowserClient::new(config);
        Self {
            home: None,
            client,
            events,
            seen: Vec::new(),
        }
    }

    pub fn profile(name: &str) -> ProfileName {
        ProfileName::new(name).unwrap()
    }

    pub fn open(&mut self, profile: &str, url: &str) -> TabId {
        let (tab, ticket) = self.client.open_tab(Self::profile(profile), url);
        assert_eq!(ticket.wait(DEADLINE), Ok(Reply::Done), "open {url}");
        tab
    }

    pub fn wait_for(
        &mut self,
        what: &str,
        mut matches: impl FnMut(&ClientEvent) -> bool,
    ) -> ClientEvent {
        if let Some(found) = self.seen.iter().position(&mut matches) {
            return self.seen.remove(found);
        }
        let deadline = Instant::now() + DEADLINE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) if matches(&event) => return event,
                Ok(event) => self.seen.push(event),
                Err(_) => panic!("never saw {what}; saw {:#?}", self.seen),
            }
        }
    }

    pub fn wait_loaded(&mut self, tab: TabId, url_part: &str) -> i32 {
        let event = self.wait_for(&format!("tab {} loading {url_part}", tab.0), |event| {
            matches!(event, ClientEvent::Browser(Event::Loaded { tab: t, url, .. }) if *t == tab && url.contains(url_part))
        });
        match event {
            ClientEvent::Browser(Event::Loaded { http_status, .. }) => http_status,
            _ => unreachable!(),
        }
    }

    pub fn evaluate(&self, tab: TabId, expression: &str) -> serde_json::Value {
        let ticket = self.client.devtools(
            tab,
            "Runtime.evaluate",
            serde_json::json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        );
        match ticket.wait(DEADLINE) {
            Ok(Reply::DevTools(result)) => result["result"]["value"].clone(),
            other => panic!("evaluate {expression}: {other:?}"),
        }
    }

    pub fn evaluate_until(
        &self,
        tab: TabId,
        expression: &str,
        done: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let value = self.evaluate(tab, expression);
            if done(&value) {
                return value;
            }
            assert!(Instant::now() < deadline, "{expression} stayed {value}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn host_pid(&self) -> u32 {
        self.client.host_pid().expect("a running host")
    }

    pub fn shutdown(&mut self) {
        if self.client.host_pid().is_some() {
            let _ = self.client.shutdown().wait(DEADLINE);
            self.wait_for("the host to exit", |event| {
                matches!(
                    event,
                    ClientEvent::HostExited { .. } | ClientEvent::HostCrashed { .. }
                )
            });
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(pid) = self.client.host_pid() {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
}

pub fn process_tree(root: u32) -> BTreeSet<u32> {
    let mut tree = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        if !tree.insert(pid) {
            continue;
        }
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let children =
                std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
            stack.extend(
                children
                    .split_whitespace()
                    .filter_map(|child| child.parse::<u32>().ok()),
            );
        }
    }
    tree
}

pub fn command_line(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .unwrap_or_default()
}

pub fn environment(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/environ"))
        .map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
                .map(|entry| String::from_utf8_lossy(entry).to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub fn listening_tcp_inodes() -> BTreeSet<u64> {
    let mut inodes = BTreeSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let text = std::fs::read_to_string(table).unwrap_or_default();
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.get(3) == Some(&"0A")
                && let Some(inode) = fields.get(9).and_then(|inode| inode.parse().ok())
            {
                inodes.insert(inode);
            }
        }
    }
    inodes
}

pub fn socket_inodes(pid: u32) -> BTreeSet<u64> {
    let mut inodes = BTreeSet::new();
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return inodes;
    };
    for fd in fds.flatten() {
        if let Ok(target) = std::fs::read_link(fd.path()) {
            let target = target.to_string_lossy().to_string();
            if let Some(inode) = target
                .strip_prefix("socket:[")
                .and_then(|rest| rest.strip_suffix(']'))
                && let Ok(inode) = inode.parse()
            {
                inodes.insert(inode);
            }
        }
    }
    inodes
}

pub fn is_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| {
            stat.rsplit(')')
                .next()
                .and_then(|rest| rest.split_whitespace().next())
                .is_some_and(|state| state != "Z" && state != "X")
        })
        .unwrap_or(false)
}
