use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use paneflow_agent_config::screen_rules::{RuleOrigin, SCREEN_ENGINE, parse_base_rules};
use paneflow_minisign::{PublicKey, VerifyError};
use serde::Deserialize;

use crate::host::SessionHost;
use crate::screen_rule_registry::{RemoteRules, ScreenRuleRegistry};

pub const SCREEN_CATALOG_PUBLIC_KEY: Option<&str> =
    Some("RWR1CN0Y0B97k5PUrQVRNmpoByKP7wydRb8nKNl+++KUb7koi0fGocc8");

pub const MAX_CATALOG_BYTES: u64 = 1 << 20;

pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);

pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

const TAG_PREFIX: &str = "screen-catalog-v";

const TAGS_URL: &str =
    "https://api.github.com/repos/arthjean/paneflow/git/matching-refs/tags/screen-catalog-v";

const CATALOG_FILE: &str = "screen-catalog.json";

const SIGNATURE_FILE: &str = "screen-catalog.json.minisig";

const WAKE_STEP: Duration = Duration::from_secs(30);

pub trait CatalogSource {
    fn latest_version(&self) -> Result<Option<u64>, String>;
    fn download(&self, version: u64) -> Result<(Vec<u8>, String), String>;
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogBody {
    engine: u32,
    version: u64,
    runtimes: BTreeMap<String, String>,
}

pub fn accept(
    body: &[u8],
    signature: &str,
    keys: &[PublicKey],
    cached_version: Option<u64>,
) -> Result<RemoteRules, String> {
    if body.len() as u64 > MAX_CATALOG_BYTES || signature.len() as u64 > MAX_CATALOG_BYTES {
        return Err(format!("larger than {MAX_CATALOG_BYTES} bytes"));
    }
    let verified =
        paneflow_minisign::verify_bytes(body, signature, keys).map_err(|error| match error {
            VerifyError::NoKey => "this build embeds no catalog signing key".to_string(),
            other => format!("signature: {other}"),
        })?;
    let (engine, version) = signed_engine_and_version(verified.trusted_comment())?;
    if engine != SCREEN_ENGINE {
        return Err(format!(
            "engine {engine} differs from the engine {SCREEN_ENGINE} of this build"
        ));
    }
    if let Some(cached) = cached_version.filter(|cached| version <= *cached) {
        return Err(format!(
            "version {version} is not newer than the cached version {cached}"
        ));
    }
    let catalog: CatalogBody =
        serde_json::from_slice(body).map_err(|error| format!("malformed catalog: {error}"))?;
    if catalog.engine != engine || catalog.version != version {
        return Err(format!(
            "the catalog body (engine {}, version {}) disagrees with its signed comment (engine {engine}, version {version})",
            catalog.engine, catalog.version
        ));
    }
    let mut runtimes = BTreeMap::new();
    for (slug, text) in catalog.runtimes {
        let rules = parse_base_rules(&text, RuleOrigin::Remote(version))
            .map_err(|error| format!("runtime {slug}: {error}"))?;
        runtimes.insert(slug, rules);
    }
    Ok(RemoteRules { version, runtimes })
}

fn signed_engine_and_version(comment: &str) -> Result<(u32, u64), String> {
    let mut engine = None;
    let mut version = None;
    for token in comment.split_whitespace() {
        if let Some(value) = token.strip_prefix("engine=") {
            engine = value.parse::<u32>().ok();
        } else if let Some(value) = token.strip_prefix("version=") {
            version = value.parse::<u64>().ok();
        }
    }
    match (engine, version) {
        (Some(engine), Some(version)) => Ok((engine, version)),
        _ => Err(format!(
            "the signed comment {comment:?} does not carry engine=<E> version=<N>"
        )),
    }
}

pub fn embedded_keys() -> Vec<PublicKey> {
    SCREEN_CATALOG_PUBLIC_KEY
        .and_then(|key| match PublicKey::from_base64(key.trim()) {
            Ok(key) => Some(key),
            Err(error) => {
                log::error!("paneflow-host: the embedded screen catalog key is invalid: {error}");
                None
            }
        })
        .into_iter()
        .collect()
}

pub fn load_cache(home: &Path, keys: &[PublicKey], registry: &ScreenRuleRegistry) {
    let dir = paneflow_home::screen_catalog_cache_dir_in(home);
    let (Ok(body), Ok(signature)) = (
        read_bounded(&dir.join(CATALOG_FILE)),
        std::fs::read_to_string(dir.join(SIGNATURE_FILE)),
    ) else {
        return;
    };
    match accept(&body, &signature, keys, None) {
        Ok(remote) => registry.apply_remote(remote),
        Err(reason) => log::warn!("paneflow-host: the cached screen catalog is ignored: {reason}"),
    }
}

pub fn remote_catalog_enabled(home: &Path) -> bool {
    paneflow_config::loader::load_config_from_path(&home.join("paneflow.json"))
        .agents
        .as_ref()
        .is_none_or(|agents| agents.resolved_remote_screen_catalog())
}

pub fn check(
    home: &Path,
    keys: &[PublicKey],
    registry: &ScreenRuleRegistry,
    source: &dyn CatalogSource,
) {
    if !remote_catalog_enabled(home) || keys.is_empty() {
        return;
    }
    let cached = registry.remote_version();
    let latest = match source.latest_version() {
        Ok(latest) => latest,
        Err(error) => {
            log::info!("paneflow-host: the screen catalog is unreachable, rules stay: {error}");
            return;
        }
    };
    let Some(latest) = latest.filter(|latest| cached.is_none_or(|cached| *latest > cached)) else {
        return;
    };
    let (body, signature) = match source.download(latest) {
        Ok(downloaded) => downloaded,
        Err(error) => {
            registry.reject_remote(error);
            return;
        }
    };
    match accept(&body, &signature, keys, cached) {
        Ok(remote) => {
            if let Err(error) = write_cache(home, &body, &signature) {
                log::warn!("paneflow-host: cannot cache the screen catalog: {error}");
            }
            registry.apply_remote(remote);
        }
        Err(reason) => registry.reject_remote(reason),
    }
}

fn write_cache(home: &Path, body: &[u8], signature: &str) -> std::io::Result<()> {
    let dir = paneflow_home::screen_catalog_cache_dir_in(home);
    std::fs::create_dir_all(&dir)?;
    for (name, bytes) in [(CATALOG_FILE, body), (SIGNATURE_FILE, signature.as_bytes())] {
        let staged = dir.join(format!("{name}.tmp"));
        std::fs::write(&staged, bytes)?;
        std::fs::rename(&staged, dir.join(name))?;
    }
    Ok(())
}

fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub struct GithubReleases;

impl GithubReleases {
    fn get(url: &str) -> Result<Vec<u8>, String> {
        let mut response = ureq::get(url)
            .config()
            .https_only(true)
            .max_redirects(5)
            .timeout_global(Some(HTTP_TIMEOUT))
            .build()
            .header(
                "User-Agent",
                &format!("paneflow-host/{}", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .map_err(|error| format!("{url}: {error}"))?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_CATALOG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{url}: {error}"))?;
        if bytes.len() as u64 > MAX_CATALOG_BYTES {
            return Err(format!("{url} is larger than {MAX_CATALOG_BYTES} bytes"));
        }
        Ok(bytes)
    }
}

impl CatalogSource for GithubReleases {
    fn latest_version(&self) -> Result<Option<u64>, String> {
        let refs: Vec<serde_json::Value> = serde_json::from_slice(&Self::get(TAGS_URL)?)
            .map_err(|error| format!("malformed tag list: {error}"))?;
        Ok(refs
            .iter()
            .filter_map(|reference| reference.get("ref")?.as_str())
            .filter_map(|reference| {
                reference
                    .strip_prefix("refs/tags/")?
                    .strip_prefix(TAG_PREFIX)
            })
            .filter_map(|version| version.parse::<u64>().ok())
            .max())
    }

    fn download(&self, version: u64) -> Result<(Vec<u8>, String), String> {
        let base =
            format!("https://github.com/arthjean/paneflow/releases/download/{TAG_PREFIX}{version}");
        let body = Self::get(&format!("{base}/{CATALOG_FILE}"))?;
        let signature = String::from_utf8(Self::get(&format!("{base}/{SIGNATURE_FILE}"))?)
            .map_err(|_| "the catalog signature is not UTF-8".to_string())?;
        Ok((body, signature))
    }
}

pub fn spawn(host: &Arc<SessionHost>) {
    let weak = Arc::downgrade(host);
    let spawned = std::thread::Builder::new()
        .name("paneflow-host-screen-catalog".into())
        .spawn(move || run(weak));
    if let Err(error) = spawned {
        log::warn!("paneflow-host: cannot start the screen catalog check: {error}");
    }
}

fn run(host: Weak<SessionHost>) {
    let keys = embedded_keys();
    {
        let Some(host) = host.upgrade() else {
            return;
        };
        load_cache(host.home(), &keys, host.screen_rules());
        if keys.is_empty() {
            host.screen_rules()
                .reject_remote("this build embeds no catalog signing key".to_string());
            return;
        }
    }
    let mut next = Instant::now() + FIRST_CHECK_DELAY;
    loop {
        while Instant::now() < next {
            std::thread::sleep(WAKE_STEP.min(next.saturating_duration_since(Instant::now())));
            if host.strong_count() == 0 {
                return;
            }
        }
        let Some(live) = host.upgrade() else {
            return;
        };
        check(live.home(), &keys, live.screen_rules(), &GithubReleases);
        drop(live);
        next = Instant::now() + CHECK_INTERVAL;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_build_embeds_one_valid_catalog_key() {
        assert_eq!(embedded_keys().len(), 1);
    }
    use std::cell::Cell;
    use std::io::Cursor;

    fn keypair() -> (minisign::KeyPair, PublicKey) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let text = pair.pk.to_box().unwrap().into_string();
        let public = PublicKey::from_base64(text.lines().nth(1).unwrap()).unwrap();
        (pair, public)
    }

    fn catalog(engine: u32, version: u64, idle_pattern: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "engine": engine,
            "version": version,
            "runtimes": {
                "claude-code": format!(
                    "engine = {engine}\n[[rules]]\nid = \"idle-prompt\"\nstate = \"idle\"\nany = ['{idle_pattern}']\n"
                ),
            },
        }))
        .unwrap()
    }

    fn sign(pair: &minisign::KeyPair, body: &[u8], engine: u32, version: u64) -> String {
        minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(body),
            Some(&format!("engine={engine} version={version}")),
            None,
        )
        .unwrap()
        .into_string()
    }

    struct Served {
        latest: Option<u64>,
        body: Vec<u8>,
        signature: String,
        calls: Cell<usize>,
    }

    impl CatalogSource for Served {
        fn latest_version(&self) -> Result<Option<u64>, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.latest)
        }

        fn download(&self, _version: u64) -> Result<(Vec<u8>, String), String> {
            self.calls.set(self.calls.get() + 1);
            Ok((self.body.clone(), self.signature.clone()))
        }
    }

    fn served(pair: &minisign::KeyPair, version: u64, signed_version: u64) -> Served {
        let body = catalog(SCREEN_ENGINE, version, "remote-prompt");
        Served {
            latest: Some(version),
            signature: sign(pair, &body, SCREEN_ENGINE, signed_version),
            body,
            calls: Cell::new(0),
        }
    }

    #[test]
    fn a_newer_signed_catalog_is_applied_cached_and_reloaded_offline() {
        let home = tempfile::tempdir().unwrap();
        let (pair, public) = keypair();
        let registry = ScreenRuleRegistry::with_builtin();
        check(
            home.path(),
            std::slice::from_ref(&public),
            &registry,
            &served(&pair, 3, 3),
        );
        assert_eq!(registry.remote_version(), Some(3));
        let rules = registry.rules_for("claude-code").unwrap();
        assert_eq!(rules[0].origin, RuleOrigin::Remote(3));

        let restarted = ScreenRuleRegistry::with_builtin();
        load_cache(home.path(), &[public], &restarted);
        assert_eq!(
            restarted.remote_version(),
            Some(3),
            "the cached catalog is active again without any network"
        );
    }

    type SignedCatalog<'a> = Box<dyn Fn() -> (Vec<u8>, String) + 'a>;

    #[test]
    fn a_rejected_catalog_keeps_the_active_rules_and_names_its_reason() {
        let (pair, public) = keypair();
        let (stranger, _) = keypair();
        let cases: Vec<(&str, SignedCatalog<'_>)> = vec![
            (
                "signature",
                Box::new(|| {
                    let body = catalog(SCREEN_ENGINE, 5, "x");
                    let signature = sign(&stranger, &body, SCREEN_ENGINE, 5);
                    (body, signature)
                }),
            ),
            (
                "engine 3 differs",
                Box::new(|| {
                    let body = catalog(3, 5, "x");
                    let signature = sign(&pair, &body, 3, 5);
                    (body, signature)
                }),
            ),
            (
                "version 2 is not newer than the cached version 2",
                Box::new(|| {
                    let body = catalog(SCREEN_ENGINE, 2, "x");
                    let signature = sign(&pair, &body, SCREEN_ENGINE, 2);
                    (body, signature)
                }),
            ),
            (
                "version 1 is not newer than the cached version 2",
                Box::new(|| {
                    let body = catalog(SCREEN_ENGINE, 1, "x");
                    let signature = sign(&pair, &body, SCREEN_ENGINE, 1);
                    (body, signature)
                }),
            ),
            (
                "larger than",
                Box::new(|| {
                    let mut body = catalog(SCREEN_ENGINE, 5, "x");
                    body.extend(std::iter::repeat_n(b' ', MAX_CATALOG_BYTES as usize));
                    let signature = sign(&pair, &body, SCREEN_ENGINE, 5);
                    (body, signature)
                }),
            ),
            (
                "invalid regex",
                Box::new(|| {
                    let body = catalog(SCREEN_ENGINE, 5, "(open");
                    let signature = sign(&pair, &body, SCREEN_ENGINE, 5);
                    (body, signature)
                }),
            ),
            (
                "disagrees with its signed comment",
                Box::new(|| {
                    let body = catalog(SCREEN_ENGINE, 5, "x");
                    let signature = sign(&pair, &body, SCREEN_ENGINE, 6);
                    (body, signature)
                }),
            ),
        ];
        for (expected, build) in cases {
            let home = tempfile::tempdir().unwrap();
            let registry = ScreenRuleRegistry::with_builtin();
            check(
                home.path(),
                std::slice::from_ref(&public),
                &registry,
                &served(&pair, 2, 2),
            );
            assert_eq!(registry.remote_version(), Some(2));
            let (body, signature) = build();
            let source = Served {
                latest: Some(9),
                body,
                signature,
                calls: Cell::new(0),
            };
            check(
                home.path(),
                std::slice::from_ref(&public),
                &registry,
                &source,
            );
            assert_eq!(registry.remote_version(), Some(2), "{expected}");
            let rejection = registry
                .status("claude-code")
                .remote_rejection
                .unwrap_or_default();
            assert!(rejection.contains(expected), "{expected}: {rejection}");
            assert!(
                registry.rules_for("claude-code").unwrap()[0].origin == RuleOrigin::Remote(2),
                "{expected}: the active rules stay"
            );
        }
    }

    #[test]
    fn disabling_the_remote_catalog_makes_no_network_call() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            r#"{"agents": {"remote_screen_catalog": false}}"#,
        )
        .unwrap();
        let (pair, public) = keypair();
        let registry = ScreenRuleRegistry::with_builtin();
        let source = served(&pair, 3, 3);
        check(home.path(), &[public], &registry, &source);
        assert_eq!(source.calls.get(), 0);
        assert_eq!(registry.remote_version(), None);

        std::fs::write(home.path().join("paneflow.json"), "{}").unwrap();
        assert!(remote_catalog_enabled(home.path()), "the default is on");
    }

    #[test]
    fn a_build_without_a_catalog_key_never_downloads() {
        let home = tempfile::tempdir().unwrap();
        let (pair, _) = keypair();
        let registry = ScreenRuleRegistry::with_builtin();
        let source = served(&pair, 3, 3);
        check(home.path(), &[], &registry, &source);
        assert_eq!(source.calls.get(), 0);
    }

    #[test]
    fn an_unchanged_latest_version_downloads_nothing() {
        let home = tempfile::tempdir().unwrap();
        let (pair, public) = keypair();
        let registry = ScreenRuleRegistry::with_builtin();
        check(
            home.path(),
            std::slice::from_ref(&public),
            &registry,
            &served(&pair, 3, 3),
        );
        let again = served(&pair, 3, 3);
        check(home.path(), &[public], &registry, &again);
        assert_eq!(again.calls.get(), 1, "only the tag list is read");
        assert_eq!(registry.status("claude-code").remote_rejection, None);
    }
}
