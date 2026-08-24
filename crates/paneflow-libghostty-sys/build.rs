#[path = "src/build_support/mod.rs"]
mod build_support;

fn main() -> build_support::BuildResult<()> {
    build_support::run()
}
