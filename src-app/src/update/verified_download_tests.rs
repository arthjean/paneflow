use super::*;
use std::io::{BufRead, BufReader, Cursor};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

const STALL: Duration = Duration::from_millis(400);
const PAUSE_UNDER_STALL: Duration = Duration::from_millis(150);
const SCALED_OLD_GLOBAL: Duration = Duration::from_millis(800);
const SLOW_CHUNKS: usize = 10;

#[derive(Clone)]
enum Reply {
    Redirect(String),
    Body {
        bytes: Vec<u8>,
        chunks: usize,
        pause: Duration,
    },
    StallAfter {
        bytes: Vec<u8>,
        sent: usize,
        hold: Duration,
    },
}

struct Server {
    base: String,
}

impl Server {
    fn start(routes: impl Fn(&str, &str) -> Option<Reply> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let routes = Arc::new(routes);
        let base_for_thread = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let routes = routes.clone();
                let base = base_for_thread.clone();
                std::thread::spawn(move || serve(stream, &base, routes.as_ref()));
            }
        });
        Self { base }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

fn serve(
    stream: TcpStream,
    base: &str,
    routes: &(dyn Fn(&str, &str) -> Option<Reply> + Send + Sync),
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
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
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    let mut stream = stream;
    match routes(base, &path) {
        None => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
        Some(Reply::Redirect(location)) => {
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            );
        }
        Some(Reply::Body {
            bytes,
            chunks,
            pause,
        }) => {
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            );
            let size = bytes.len().div_ceil(chunks.max(1)).max(1);
            for (index, chunk) in bytes.chunks(size).enumerate() {
                if index > 0 {
                    std::thread::sleep(pause);
                }
                if stream
                    .write_all(chunk)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
        }
        Some(Reply::StallAfter { bytes, sent, hold }) => {
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(&bytes[..sent]);
            let _ = stream.flush();
            std::thread::sleep(hold);
        }
    }
}

fn loopback_http(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:")
}

fn test_transfer(total: Duration) -> AssetTransfer {
    AssetTransfer {
        http: HttpTimeouts {
            global: None,
            connect: Some(Duration::from_secs(5)),
            send_request: Some(Duration::from_secs(5)),
            stall: Some(STALL),
        },
        total,
        redirect_allowed: loopback_http,
    }
}

struct Signer {
    public: PublicKey,
    pair: minisign::KeyPair,
}

impl Signer {
    fn new() -> Self {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().expect("keypair");
        let text = pair.pk.to_box().expect("public box").into_string();
        let public = PublicKey::from_base64(text.lines().nth(1).expect("key line")).expect("key");
        Self { public, pair }
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        minisign::sign(
            Some(&self.pair.pk),
            &self.pair.sk,
            Cursor::new(data),
            None,
            None,
        )
        .expect("sign")
        .into_string()
        .into_bytes()
    }
}

fn payload() -> Vec<u8> {
    (0..64 * 1024).map(|i| (i % 251) as u8).collect()
}

fn slow_asset_server(payload: Vec<u8>, signature: Vec<u8>) -> Server {
    Server::start(move |_, path| match path {
        "/asset.tar.gz" => Some(Reply::Body {
            bytes: payload.clone(),
            chunks: SLOW_CHUNKS,
            pause: PAUSE_UNDER_STALL,
        }),
        "/asset.tar.gz.minisig" => Some(Reply::Body {
            bytes: signature.clone(),
            chunks: 1,
            pause: Duration::ZERO,
        }),
        _ => None,
    })
}

#[test]
fn a_slow_transfer_longer_than_the_old_global_timeout_completes() {
    let signer = Signer::new();
    let payload = payload();
    let server = slow_asset_server(payload.clone(), signer.sign(&payload));
    let url = server.url("/asset.tar.gz");

    let old = get_following_redirects(
        &url,
        HttpTimeouts::whole_request(SCALED_OLD_GLOBAL),
        loopback_http,
    )
    .expect("headers arrive before the old global timeout")
    .body_mut()
    .read_to_vec();
    assert!(
        old.is_err(),
        "the previous whole-request timeout aborts a transfer that never stalls"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("asset.tar.gz");
    let started = Instant::now();
    download_verified_asset_with(
        &url,
        &dest,
        1 << 20,
        "test",
        &test_transfer(Duration::from_secs(30)),
        std::slice::from_ref(&signer.public),
    )
    .expect("a transfer whose pauses stay under the stall limit completes");
    assert!(started.elapsed() > SCALED_OLD_GLOBAL);
    assert_eq!(std::fs::read(&dest).expect("dest"), payload);
    assert!(!dir.path().join("asset.tar.gz.partial").exists());
}

#[test]
fn a_server_that_stops_sending_fails_at_the_stall_limit() {
    let signer = Signer::new();
    let payload = payload();
    let signature = signer.sign(&payload);
    let hold = Duration::from_secs(5);
    let server = Server::start(move |_, path| match path {
        "/asset.tar.gz" => Some(Reply::StallAfter {
            bytes: payload.clone(),
            sent: 1024,
            hold,
        }),
        "/asset.tar.gz.minisig" => Some(Reply::Body {
            bytes: signature.clone(),
            chunks: 1,
            pause: Duration::ZERO,
        }),
        _ => None,
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("asset.tar.gz");
    let started = Instant::now();
    let err = download_verified_asset_with(
        &server.url("/asset.tar.gz"),
        &dest,
        1 << 20,
        "test",
        &test_transfer(Duration::from_secs(30)),
        std::slice::from_ref(&signer.public),
    )
    .expect_err("a stalled transfer fails");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= STALL && elapsed < hold,
        "failed after {elapsed:?}"
    );
    assert_eq!(UpdateError::classify(&err), UpdateError::Timeout, "{err:#}");
    assert!(!dest.exists());
    assert!(!dir.path().join("asset.tar.gz.partial").exists());
}

#[test]
fn a_transfer_past_the_total_cap_fails_even_when_it_never_stalls() {
    let signer = Signer::new();
    let payload = payload();
    let server = slow_asset_server(payload.clone(), signer.sign(&payload));
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("asset.tar.gz");
    let err = download_verified_asset_with(
        &server.url("/asset.tar.gz"),
        &dest,
        1 << 20,
        "test",
        &test_transfer(PAUSE_UNDER_STALL * 3),
        std::slice::from_ref(&signer.public),
    )
    .expect_err("the copy loop enforces the total cap");
    assert_eq!(UpdateError::classify(&err), UpdateError::Timeout, "{err:#}");
    assert!(format!("{err:#}").contains("took longer than"), "{err:#}");
    assert!(!dest.exists());
    assert!(!dir.path().join("asset.tar.gz.partial").exists());
}

#[test]
fn redirects_are_followed_hop_by_hop_and_each_target_is_checked() {
    let signer = Signer::new();
    let payload = payload();
    let signature = signer.sign(&payload);
    let served = payload.clone();
    let server = Server::start(move |base, path| {
        if let Some(rest) = path.strip_prefix("/hop/") {
            let (left, suffix) = rest.split_once('/').unwrap_or((rest, ""));
            let left: usize = left.parse().ok()?;
            let next = if left == 0 {
                format!("{base}/asset.tar.gz{suffix}")
            } else {
                format!("{base}/hop/{}/{suffix}", left - 1)
            };
            return Some(Reply::Redirect(next));
        }
        match path {
            "/asset.tar.gz" => Some(Reply::Body {
                bytes: served.clone(),
                chunks: 1,
                pause: Duration::ZERO,
            }),
            "/asset.tar.gz.minisig" => Some(Reply::Body {
                bytes: signature.clone(),
                chunks: 1,
                pause: Duration::ZERO,
            }),
            "/elsewhere" => Some(Reply::Redirect("http://127.0.0.2:9/asset".to_string())),
            "/downgrade" => Some(Reply::Redirect("https://evil.example/asset".to_string())),
            _ => None,
        }
    });
    let transfer = test_transfer(Duration::from_secs(30));
    let keys = std::slice::from_ref(&signer.public);
    let dir = tempfile::tempdir().expect("tempdir");

    let five = dir.path().join("five");
    download_verified_asset_with(
        &server.url("/hop/4/"),
        &five,
        1 << 20,
        "test",
        &transfer,
        keys,
    )
    .expect("five redirects are followed");
    assert_eq!(std::fs::read(&five).expect("five"), payload);

    let six = dir.path().join("six");
    let err = download_verified_asset_with(
        &server.url("/hop/5/"),
        &six,
        1 << 20,
        "test",
        &transfer,
        keys,
    )
    .expect_err("a sixth redirect is refused");
    assert!(
        format!("{err:#}").contains("gave up after 5 redirects"),
        "{err:#}"
    );
    assert!(!six.exists());

    for refused in ["/elsewhere", "/downgrade"] {
        let dest = dir.path().join("refused");
        let err = download_verified_asset_with(
            &server.url(refused),
            &dest,
            1 << 20,
            "test",
            &transfer,
            keys,
        )
        .expect_err("a redirect outside the allowed hosts is refused");
        assert!(
            format!("{err:#}").contains("refusing a redirect"),
            "{err:#}"
        );
        assert!(!dest.exists());
    }
}

#[test]
fn an_invalid_signature_refuses_the_install_and_leaves_no_file() {
    let signer = Signer::new();
    let impostor = Signer::new();
    let payload = payload();
    let server = slow_asset_server(payload.clone(), impostor.sign(&payload));
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("asset.tar.gz");
    let err = download_verified_asset_with(
        &server.url("/asset.tar.gz"),
        &dest,
        1 << 20,
        "test",
        &test_transfer(Duration::from_secs(30)),
        std::slice::from_ref(&signer.public),
    )
    .expect_err("a signature from an untrusted key is refused");
    assert!(
        matches!(
            UpdateError::classify(&err),
            UpdateError::IntegrityMismatch { .. }
        ),
        "{err:#}"
    );
    assert!(!dest.exists(), "the destination is never created");
    assert!(
        !dir.path().join("asset.tar.gz.partial").exists(),
        "the partial download is removed"
    );
}

#[test]
fn the_release_transfer_streams_without_a_whole_request_deadline() {
    assert!((ASSET_TRANSFER.redirect_allowed)(
        "https://release-assets.githubusercontent.com/x"
    ));
    assert!(!(ASSET_TRANSFER.redirect_allowed)(
        "http://release-assets.githubusercontent.com/x"
    ));
    assert!(!(ASSET_TRANSFER.redirect_allowed)("https://evil.example/x"));
    let http = ASSET_TRANSFER.http;
    assert_eq!(http.global, None);
    assert_eq!(http.connect, Some(Duration::from_secs(30)));
    assert_eq!(http.send_request, Some(Duration::from_secs(30)));
    assert_eq!(http.stall, Some(Duration::from_secs(60)));
    assert_eq!(ASSET_TRANSFER.total, Duration::from_secs(15 * 60));
}

#[test]
fn append_suffix_preserves_full_name() {
    assert_eq!(
        append_suffix(Path::new("/tmp/foo.tar.gz"), ".partial").unwrap(),
        PathBuf::from("/tmp/foo.tar.gz.partial")
    );
}

#[test]
fn append_suffix_rejects_pathless_input() {
    assert!(append_suffix(Path::new("/"), ".partial").is_err());
}
