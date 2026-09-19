mod build_support;

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::other("CARGO_MANIFEST_DIR is unavailable"))?,
    );
    let runtimes_dir = manifest_dir.join("..").join("..").join("runtimes");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    println!("cargo:rerun-if-changed={}", runtimes_dir.display());
    let descriptors =
        build_support::discover_and_validate(&runtimes_dir).map_err(io::Error::other)?;
    for descriptor in &descriptors {
        println!("cargo:rerun-if-changed={}", descriptor.path.display());
    }
    let generated = build_support::generate_catalog(&descriptors).map_err(io::Error::other)?;
    let output = PathBuf::from(
        env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("OUT_DIR is unavailable"))?,
    )
    .join("runtime_catalog.rs");
    fs::write(output, generated)?;
    Ok(())
}
