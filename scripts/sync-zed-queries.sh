#!/usr/bin/env bash
set -euo pipefail
mode=sync
commit=
zig_commit=
while (($#)); do
    case "$1" in
        --check) mode=check; shift ;;
        --commit) commit="${2:?--commit requires a revision}"; shift 2 ;;
        --zig-commit) zig_commit="${2:?--zig-commit requires a revision}"; shift 2 ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done
: "${ZED_DIR:?Set ZED_DIR to the local Zed checkout}"
: "${ZED_ZIG_DIR:?Set ZED_ZIG_DIR to the local zed-extensions/zig checkout}"
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../src-app/src/diff/queries" && pwd)"
if [[ -z "$commit" ]]; then
    commit="$(sed -n '1,/^\[\[queries\]\]$/s/^commit = "\([a-f0-9]*\)"$/\1/p' "$root/MANIFEST.toml")"
fi
if [[ -z "$zig_commit" ]]; then
    zig_commit="$(sed -n '/^name = "zig"$/,/^\[\[queries\]\]$/s/^commit = "\([a-f0-9]*\)"$/\1/p' "$root/MANIFEST.toml")"
fi
commit="$(git -C "$ZED_DIR" rev-parse --verify "${commit}^{commit}")"
[[ "$commit" =~ ^[a-f0-9]{40}$ ]] || { printf 'Invalid Zed commit\n' >&2; exit 1; }
[[ -n "$zig_commit" ]] || { printf 'MANIFEST.toml has no Zig commit: pass --zig-commit\n' >&2; exit 1; }
zig_commit="$(git -C "$ZED_ZIG_DIR" rev-parse --verify "${zig_commit}^{commit}")"
[[ "$zig_commit" =~ ^[a-f0-9]{40}$ ]] || { printf 'Invalid zed-extensions/zig commit\n' >&2; exit 1; }
stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
sha256_of() {
    local hash
    if command -v sha256sum >/dev/null 2>&1; then
        hash="$(sha256sum "$1")"
    else
        hash="$(shasum -a 256 "$1")"
    fi
    printf '%s' "${hash%% *}"
}
languages=(rust json jsonc bash python typescript tsx javascript markdown markdown-inline go yaml css c cpp)
printf 'repository = "https://github.com/zed-industries/zed"\ncommit = "%s"\nlicense = "GPL-3.0-or-later"\nlicense_evidence = "crates/languages/Cargo.toml; LICENSE-GPL; crates/grammars/Cargo.toml has no license field"\n' "$commit" > "$stage/MANIFEST.toml"
for language in "${languages[@]}"; do
    source="crates/grammars/src/$language/highlights.scm"
    mkdir -p -- "$stage/$language"
    git -C "$ZED_DIR" show "$commit:$source" > "$stage/$language/highlights.scm"
    hash="$(sha256_of "$stage/$language/highlights.scm")"
    deviations='[]'
    if [[ "$language" == javascript ]]; then
        deviations='["Uses the TSX grammar instead of tree-sitter-javascript"]'
    fi
    printf '\n[[queries]]\nname = "%s"\nsource = "%s"\nsha256 = "%s"\ndeviations = %s\n' "$language" "$source" "$hash" "$deviations" >> "$stage/MANIFEST.toml"
done
zig_source="languages/zig/highlights.scm"
mkdir -p -- "$stage/zig"
git -C "$ZED_ZIG_DIR" show "$zig_commit:$zig_source" > "$stage/zig/highlights.scm"
git -C "$ZED_ZIG_DIR" show "$zig_commit:LICENSE" > "$stage/zig/LICENSE"
printf '\n[[queries]]\nname = "zig"\nrepository = "https://github.com/zed-extensions/zig"\ncommit = "%s"\nsource = "%s"\nsha256 = "%s"\nlicense = "Apache-2.0"\nlicense_evidence = "LICENSE; Cargo.toml declares Apache-2.0"\ndeviations = ["Uses the tree-sitter-zig 1.1.2 crate parser instead of the grammar commit the extension pins"]\n' "$zig_commit" "$zig_source" "$(sha256_of "$stage/zig/highlights.scm")" >> "$stage/MANIFEST.toml"
cat > "$stage/NOTICE" <<EOF
Syntax highlighting queries from Zed, Copyright Zed Industries and contributors.
Source: https://github.com/zed-industries/zed
Revision: $commit

Imported byte for byte from crates/grammars/src/<language>/highlights.scm.
The originating crates/languages package declares GPL-3.0-or-later at this
revision. crates/grammars/Cargo.toml has no license field; the repository
contains LICENSE-GPL and LICENSE-APACHE. These queries are distributed under
GPL-3.0-or-later with Paneflow; see the repository LICENSE.

Only highlighting queries are imported. Injections and font styles are outside
this integration. JavaScript uses Paneflow's existing TSX grammar.

The Zig highlighting query comes from the Zed Zig extension, Copyright Zed
Industries and contributors, licensed under Apache-2.0.
Source: https://github.com/zed-extensions/zig
Revision: $zig_commit

Imported byte for byte from languages/zig/highlights.scm; zig/LICENSE holds
the extension's Apache-2.0 text. It applies to .zig and .zon files.
EOF
failed=0
files=(MANIFEST.toml NOTICE)
for language in "${languages[@]}"; do files+=("$language/highlights.scm"); done
files+=(zig/highlights.scm zig/LICENSE)
for file in "${files[@]}"; do
    if [[ "$mode" == check ]]; then
        if ! cmp -s "$stage/$file" "$root/$file"; then
            printf 'Divergent Zed query: %s\n' "$file" >&2
            failed=1
        fi
    else
        mkdir -p -- "$(dirname -- "$root/$file")"
        cp -- "$stage/$file" "$root/$file"
    fi
done
((failed == 0)) || exit 1
printf '15 Zed queries and 1 Zed extension query verified at %s and %s\n' "$commit" "$zig_commit"
