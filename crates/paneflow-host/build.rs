use std::error::Error;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS")? != "windows" {
        return Ok(());
    }
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("missing crate path")?)
        .join("../../native/conpty");
    let manifest_path = root.join("manifest.json");
    println!("cargo::rerun-if-changed={}", manifest_path.display());
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
    let target = std::env::var("TARGET")?;
    let files = manifest["targets"][&target]
        .as_object()
        .ok_or_else(|| format!("no pinned ConPTY runtime for {target}"))?;
    let version = manifest["version"]
        .as_str()
        .ok_or("missing ConPTY version")?;
    println!("cargo::rustc-env=PANEFLOW_CONPTY_VERSION={version}");
    println!("cargo::rustc-env=PANEFLOW_CONPTY_TARGET={target}");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing output path")?);
    for (name, entry) in files {
        let path = root.join("prebuilt").join(&target).join(name);
        println!("cargo::rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).map_err(|error| {
            format!("{}: {error}; run scripts/fetch-conpty.ps1", path.display())
        })?;
        let expected = entry["sha256"].as_str().ok_or("missing ConPTY hash")?;
        if format!("{:x}", Sha256::digest(&bytes)) != expected {
            return Err(format!("ConPTY hash mismatch: {}", path.display()).into());
        }
        std::fs::write(output.join(name), bytes)?;
    }
    Ok(())
}
