use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use minisign_verify::{PublicKey, Signature};

use super::error::IntegrityMismatch;

const MAX_SIG_BYTES: u64 = 64 * 1024;

const SIG_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

const EMBEDDED_PUBKEY_CURRENT: Option<&str> = option_env!("PANEFLOW_MINISIGN_PUBKEY");

const EMBEDDED_PUBKEY_NEXT: Option<&str> = option_env!("PANEFLOW_MINISIGN_PUBKEY_NEXT");

fn embedded_public_keys() -> Vec<PublicKey> {
    [
        ("PANEFLOW_MINISIGN_PUBKEY", EMBEDDED_PUBKEY_CURRENT),
        ("PANEFLOW_MINISIGN_PUBKEY_NEXT", EMBEDDED_PUBKEY_NEXT),
    ]
    .into_iter()
    .filter_map(|(name, slot)| {
        let b64 = slot?.trim();
        if b64.is_empty() {
            return None;
        }
        match PublicKey::from_base64(b64) {
            Ok(pk) => Some(pk),
            Err(e) => {
                log::error!(
                    "self-update/signature: embedded {name} is not a valid minisign key: {e}"
                );
                None
            }
        }
    })
    .collect()
}

pub(crate) fn has_embedded_key() -> bool {
    !embedded_public_keys().is_empty()
}

fn reject(reason: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(IntegrityMismatch {
        expected: "valid minisign signature".to_string(),
        got: reason.into(),
    })
}

fn verify_with_keys(artifact: &Path, sig_text: &str, keys: &[PublicKey]) -> Result<()> {
    if keys.is_empty() {
        return Err(reject(
            "no verification key embedded in this build - refusing to install an unverifiable update",
        ));
    }

    let signature =
        Signature::decode(sig_text).map_err(|e| reject(format!("signature is malformed: {e}")))?;

    let mut key_id_matched = false;
    for key in keys {
        let mut verifier = match key.verify_stream(&signature) {
            Ok(v) => v,
            Err(_) => continue,
        };
        key_id_matched = true;

        let mut file = std::fs::File::open(artifact)
            .with_context(|| format!("open {} for signature check", artifact.display()))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .context("read artifact chunk for signature check")?;
            if n == 0 {
                break;
            }
            verifier.update(&buf[..n]);
        }
        if verifier.finalize().is_ok() {
            return Ok(());
        }
    }

    if key_id_matched {
        Err(reject(
            "artifact does not match its signature - corrupt or tampered",
        ))
    } else {
        Err(reject(
            "signature was not made by any key trusted by this build",
        ))
    }
}

pub(crate) fn verify_detached_file(artifact: &Path, sig_text: &str) -> Result<()> {
    verify_with_keys(artifact, sig_text, &embedded_public_keys())
}

pub(crate) fn fetch_and_verify(artifact: &Path, asset_url: &str) -> Result<()> {
    if !has_embedded_key() {
        return Err(reject(
            "no verification key embedded in this build - refusing to install an unverifiable update",
        ));
    }

    let sig_url = format!("{asset_url}.minisig");
    let sig_text = fetch_signature_text(&sig_url)?;
    verify_detached_file(artifact, &sig_text)
        .with_context(|| format!("verify minisign signature of {}", artifact.display()))
}

fn fetch_signature_text(sig_url: &str) -> Result<String> {
    let mut response = ureq::get(sig_url)
        .config()
        .timeout_global(Some(SIG_HTTP_TIMEOUT))
        .build()
        .header(
            "User-Agent",
            &format!("paneflow/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .with_context(|| "Could not fetch update signature. Try again when online.".to_string())?;

    let status = response.status();
    if !status.is_success() {
        if status.as_u16() == 404 {
            return Err(reject(
                "this release is not signed (no .minisig published) - download the latest version from the releases page",
            ));
        }
        return Err(reject(format!(
            "could not fetch update signature (HTTP {status})"
        )));
    }

    let reader = response.body_mut().as_reader();
    let mut bounded = Read::take(reader, MAX_SIG_BYTES + 1);
    let mut text = String::new();
    let read = bounded
        .read_to_string(&mut text)
        .context("read .minisig body")?;
    if read as u64 > MAX_SIG_BYTES {
        return Err(reject("update signature is implausibly large - aborting"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn gen_keypair() -> (PublicKey, String) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pub_text = kp.pk.to_box().unwrap().into_string();
        let b64 = pub_text.lines().nth(1).unwrap().to_string();
        let vk = PublicKey::from_base64(&b64).unwrap();
        (vk, b64)
    }

    fn sign(data: &[u8]) -> (PublicKey, String) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pub_text = kp.pk.to_box().unwrap().into_string();
        let b64 = pub_text.lines().nth(1).unwrap().to_string();
        let vk = PublicKey::from_base64(&b64).unwrap();
        let sig_box = minisign::sign(Some(&kp.pk), &kp.sk, Cursor::new(data), None, None).unwrap();
        (vk, sig_box.into_string())
    }

    fn write_tmp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("artifact.bin");
        std::fs::write(&p, bytes).unwrap();
        (dir, p)
    }

    #[test]
    fn verifies_a_correctly_signed_artifact() {
        let data = b"paneflow-0.3.9-x86_64.tar.gz payload";
        let (vk, sig) = sign(data);
        let (_d, path) = write_tmp(data);
        assert!(verify_with_keys(&path, &sig, &[vk]).is_ok());
    }

    #[test]
    fn rejects_a_tampered_artifact() {
        let data = b"the genuine release payload";
        let (vk, sig) = sign(data);
        let (_d, path) = write_tmp(b"a malicious replacement payload");
        let err = verify_with_keys(&path, &sig, &[vk]).unwrap_err();
        assert!(
            err.downcast_ref::<IntegrityMismatch>().is_some(),
            "tampered artifact must produce an IntegrityMismatch tag, got: {err:#}"
        );
        assert!(matches!(
            super::super::error::UpdateError::classify(&err),
            super::super::error::UpdateError::IntegrityMismatch { .. }
        ));
    }

    #[test]
    fn rejects_signature_from_an_untrusted_key() {
        let data = b"payload signed by a key we do not trust";
        let (_vk_a, sig) = sign(data);
        let (vk_b, _b64) = gen_keypair();
        let (_d, path) = write_tmp(data);
        let err = verify_with_keys(&path, &sig, &[vk_b]).unwrap_err();
        assert!(err.downcast_ref::<IntegrityMismatch>().is_some());
    }

    #[test]
    fn fails_closed_when_no_key_is_embedded() {
        let data = b"payload";
        let (_vk, sig) = sign(data);
        let (_d, path) = write_tmp(data);
        let err = verify_with_keys(&path, &sig, &[]).unwrap_err();
        assert!(
            err.to_string().contains("no verification key"),
            "got: {err:#}"
        );
    }

    #[test]
    fn rejects_a_malformed_signature() {
        let (_d, path) = write_tmp(b"payload");
        let (vk, _b64) = gen_keypair();
        let err = verify_with_keys(&path, "not a minisig at all", &[vk]).unwrap_err();
        assert!(err.downcast_ref::<IntegrityMismatch>().is_some());
    }

    #[test]
    fn dual_key_accepts_either_slot() {
        let data = b"signed by the rotation key";
        let kp_current = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let b64_current = kp_current
            .pk
            .to_box()
            .unwrap()
            .into_string()
            .lines()
            .nth(1)
            .unwrap()
            .to_string();
        let vk_current = PublicKey::from_base64(&b64_current).unwrap();

        let (vk_next, sig) = sign(data);
        let (_d, path) = write_tmp(data);

        assert!(verify_with_keys(&path, &sig, &[vk_current, vk_next]).is_ok());
    }

    #[test]
    fn empty_embedded_slots_yield_no_keys() {
        assert!(!has_embedded_key());
        assert!(embedded_public_keys().is_empty());
    }
}
