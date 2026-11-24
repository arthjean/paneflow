#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

const EXPECTED_VARIANT_COUNT: usize = 4;

struct ShaderVariant {
    name: String,
    source: String,
}

enum ShaderPart {
    File(String),
    Literal(String),
}

fn renderer_source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor/gpui_wgpu/src")
}

fn shader_lists(renderer: &str) -> Vec<(String, Vec<ShaderPart>)> {
    let mut lists = Vec::new();
    let mut rest = renderer;
    while let Some(start) = rest.find("const ") {
        rest = &rest[start + "const ".len()..];
        let Some((name, after_name)) = rest.split_once(':') else {
            break;
        };
        let name = name.trim();
        if !name.ends_with("_SHADERS") || name.contains(char::is_whitespace) {
            continue;
        }
        let Some(body) = after_name
            .trim_start()
            .strip_prefix("&str")
            .and_then(|tail| tail.trim_start().strip_prefix('='))
            .and_then(|tail| tail.trim_start().strip_prefix("concat!("))
        else {
            continue;
        };
        let end = body
            .find(");")
            .unwrap_or_else(|| panic!("the concat! list of {name} never closes"));
        lists.push((name.to_string(), shader_parts(name, &body[..end])));
        rest = &body[end..];
    }
    lists
}

fn shader_parts(name: &str, list: &str) -> Vec<ShaderPart> {
    list.split(",\n")
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            if let Some(file) = item
                .strip_prefix("include_str!(\"")
                .and_then(|tail| tail.strip_suffix("\")"))
            {
                ShaderPart::File(file.to_string())
            } else if let Some(literal) = item
                .strip_prefix('"')
                .and_then(|tail| tail.strip_suffix('"'))
            {
                ShaderPart::Literal(literal.replace("\\n", "\n"))
            } else {
                panic!("{name} lists {item:?}, which is neither include_str! nor a string literal")
            }
        })
        .collect()
}

fn assemble(name: &str, parts: &[ShaderPart]) -> Result<String, String> {
    let dir = renderer_source_dir();
    let mut source = String::new();
    for part in parts {
        match part {
            ShaderPart::Literal(text) => source.push_str(text),
            ShaderPart::File(file) => {
                let path = dir.join(file);
                let text = std::fs::read_to_string(&path).map_err(|error| {
                    format!(
                        "{name} lists {file}, which cannot be read at {}: {error}",
                        path.display()
                    )
                })?;
                source.push_str(&text);
            }
        }
    }
    Ok(source)
}

fn renderer_variants(renderer: &str) -> Vec<ShaderVariant> {
    let lists = shader_lists(renderer);
    let names: Vec<&str> = lists.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        lists.len(),
        EXPECTED_VARIANT_COUNT,
        "the renderer defines {} *_SHADERS lists, expected {EXPECTED_VARIANT_COUNT}: {names:?}",
        lists.len()
    );
    let mut missing = Vec::new();
    let mut variants = Vec::new();
    for (name, parts) in &lists {
        match assemble(name, parts) {
            Ok(source) => variants.push(ShaderVariant {
                name: name.clone(),
                source,
            }),
            Err(error) => missing.push(error),
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    variants
}

fn capabilities_for(variant: &ShaderVariant) -> naga::valid::Capabilities {
    if variant.name.starts_with("SUBPIXEL_") {
        naga::valid::Capabilities::DUAL_SOURCE_BLENDING
    } else {
        naga::valid::Capabilities::empty()
    }
}

fn validate(variant: &ShaderVariant) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(&variant.source).map_err(|error| {
        format!(
            "{} does not parse: {}",
            variant.name,
            error.emit_to_string(&variant.source)
        )
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        capabilities_for(variant),
    )
    .validate(&module)
    .map_err(|error| {
        format!(
            "{} does not validate: {}",
            variant.name,
            error.emit_to_string(&variant.source)
        )
    })?;
    Ok(())
}

fn read_renderer() -> String {
    let path = renderer_source_dir().join("wgpu_renderer.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

#[test]
fn every_wgsl_variant_of_the_vendored_renderer_validates_with_naga() {
    let variants = renderer_variants(&read_renderer());
    let failures: Vec<String> = variants
        .iter()
        .filter_map(|variant| validate(variant).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} WGSL variants fail naga validation:\n{}",
        failures.len(),
        variants.len(),
        failures.join("\n\n")
    );
}

#[test]
fn the_texture_variants_of_the_vendored_renderer_declare_no_storage_buffer() {
    let variants = renderer_variants(&read_renderer());
    let texture_variants: Vec<&ShaderVariant> = variants
        .iter()
        .filter(|variant| variant.name.contains("WEBGL"))
        .collect();
    assert_eq!(
        texture_variants.len(),
        2,
        "expected a plain and a subpixel texture variant: {:?}",
        variants
            .iter()
            .map(|variant| &variant.name)
            .collect::<Vec<_>>()
    );
    for variant in texture_variants {
        assert!(
            !variant.source.contains("var<storage"),
            "{} declares a storage buffer",
            variant.name
        );
    }
}

#[test]
fn a_subpixel_variant_is_validated_with_dual_source_blending() {
    let variants = renderer_variants(&read_renderer());
    let subpixel: Vec<&str> = variants
        .iter()
        .filter(|variant| {
            capabilities_for(variant).contains(naga::valid::Capabilities::DUAL_SOURCE_BLENDING)
        })
        .map(|variant| variant.name.as_str())
        .collect();
    assert_eq!(
        subpixel,
        ["SUBPIXEL_STORAGE_BUFFER_SHADERS", "SUBPIXEL_WEBGL_SHADERS"],
        "the subpixel variants are the ones the renderer builds only with dual-source blending"
    );
}
