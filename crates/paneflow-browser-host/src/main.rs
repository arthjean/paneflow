#[cfg(all(target_os = "linux", feature = "cef-runtime"))]
mod linux;

#[cfg(any(test, all(target_os = "linux", feature = "cef-runtime")))]
mod qualification;

#[cfg(all(target_os = "linux", feature = "cef-runtime"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(not(all(target_os = "linux", feature = "cef-runtime")))]
fn main() {
    eprintln!("browser unavailable: native CEF witness is not enabled on this build");
    std::process::exit(1);
}
