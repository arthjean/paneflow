#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::fmt;
use std::io::{self, Read};

pub use minisign_verify::{PublicKey, Signature};

const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum VerifyError {
    NoKey,
    Malformed(String),
    UntrustedKey,
    Mismatch,
    Open(io::Error),
    Read(io::Error),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoKey => formatter.write_str("no verification key is embedded in this build"),
            Self::Malformed(reason) => write!(formatter, "the signature is malformed: {reason}"),
            Self::UntrustedKey => {
                formatter.write_str("the signature was not made by any key this build trusts")
            }
            Self::Mismatch => formatter.write_str("the content does not match its signature"),
            Self::Open(error) => write!(formatter, "cannot open the signed content: {error}"),
            Self::Read(error) => write!(formatter, "cannot read the signed content: {error}"),
        }
    }
}

impl std::error::Error for VerifyError {}

pub fn verify<R: Read>(
    signature_text: &str,
    keys: &[PublicKey],
    mut open: impl FnMut() -> io::Result<R>,
) -> Result<Signature, VerifyError> {
    if keys.is_empty() {
        return Err(VerifyError::NoKey);
    }
    let signature = Signature::decode(signature_text)
        .map_err(|error| VerifyError::Malformed(error.to_string()))?;
    let mut key_id_matched = false;
    for key in keys {
        let verified = {
            let Ok(mut verifier) = key.verify_stream(&signature) else {
                continue;
            };
            key_id_matched = true;
            let mut reader = open().map_err(VerifyError::Open)?;
            let mut buffer = vec![0u8; CHUNK_BYTES];
            loop {
                let read = reader.read(&mut buffer).map_err(VerifyError::Read)?;
                if read == 0 {
                    break;
                }
                verifier.update(&buffer[..read]);
            }
            verifier.finalize().is_ok()
        };
        if verified {
            return Ok(signature);
        }
    }
    Err(if key_id_matched {
        VerifyError::Mismatch
    } else {
        VerifyError::UntrustedKey
    })
}

pub fn verify_bytes(
    content: &[u8],
    signature_text: &str,
    keys: &[PublicKey],
) -> Result<Signature, VerifyError> {
    verify(signature_text, keys, || Ok(content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn keypair() -> (minisign::KeyPair, PublicKey) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let text = pair.pk.to_box().unwrap().into_string();
        let public = PublicKey::from_base64(text.lines().nth(1).unwrap()).unwrap();
        (pair, public)
    }

    fn sign(pair: &minisign::KeyPair, content: &[u8], trusted: &str) -> String {
        minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(content),
            Some(trusted),
            None,
        )
        .unwrap()
        .into_string()
    }

    #[test]
    fn a_valid_signature_returns_its_authenticated_trusted_comment() {
        let (pair, public) = keypair();
        let signature = sign(&pair, b"catalog", "engine=2 version=7");
        let verified = verify_bytes(b"catalog", &signature, &[public]).unwrap();
        assert_eq!(verified.trusted_comment(), "engine=2 version=7");
    }

    #[test]
    fn tampered_content_untrusted_keys_garbage_and_no_key_all_fail_closed() {
        let (pair, public) = keypair();
        let (_, stranger) = keypair();
        let signature = sign(&pair, b"catalog", "engine=2 version=7");
        assert!(matches!(
            verify_bytes(b"catalog!", &signature, std::slice::from_ref(&public)),
            Err(VerifyError::Mismatch)
        ));
        assert!(matches!(
            verify_bytes(b"catalog", &signature, &[stranger]),
            Err(VerifyError::UntrustedKey)
        ));
        assert!(matches!(
            verify_bytes(b"catalog", "not a signature", std::slice::from_ref(&public)),
            Err(VerifyError::Malformed(_))
        ));
        assert!(matches!(
            verify_bytes(b"catalog", &signature, &[]),
            Err(VerifyError::NoKey)
        ));
    }

    #[test]
    fn a_rewritten_trusted_comment_breaks_the_global_signature() {
        let (pair, public) = keypair();
        let signature = sign(&pair, b"catalog", "engine=2 version=7");
        let forged = signature.replace("version=7", "version=9");
        assert!(verify_bytes(b"catalog", &forged, &[public]).is_err());
    }

    #[test]
    fn any_of_several_trusted_keys_verifies_and_the_content_is_reopened_per_key() {
        let (pair, public) = keypair();
        let (_, other) = keypair();
        let signature = sign(&pair, b"payload", "t");
        let mut opened = 0;
        let verified = verify(&signature, &[other, public], || {
            opened += 1;
            Ok(Cursor::new(b"payload".to_vec()))
        });
        assert!(verified.is_ok());
        assert_eq!(
            opened, 1,
            "a key whose id does not match is skipped unopened"
        );
    }
}
