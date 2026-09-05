#!/usr/bin/env bash
set -euo pipefail
mode=sync
commit=
while (($#)); do
    case "$1" in
        --check) mode=check; shift ;;
        --commit) commit="${2:?--commit requires a revision}"; shift 2 ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done
: "${ZED_DIR:?Set ZED_DIR to the local Zed checkout}"
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../src-app/src/diff/queries" && pwd)"
if [[ -z "$commit" ]]; then
    commit="$(sed -n 's/^commit = "\([a-f0-9]*\)"$/\1/p' "$root/MANIFEST.toml")"
fi
commit="$(git -C "$ZED_DIR" rev-parse --verify "${commit}^{commit}")"
[[ "$commit" =~ ^[a-f0-9]{40}$ ]] || { printf 'Invalid Zed commit\n' >&2; exit 1; }
stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
languages=(rust json jsonc bash python typescript tsx javascript markdown markdown-inline go yaml css c cpp)
printf 'repository = "https://github.com/zed-industries/zed"\ncommit = "%s"\nlicense = "GPL-3.0-or-later"\nlicense_evidence = "crates/languages/Cargo.toml; LICENSE-GPL; crates/grammars/Cargo.toml has no license field"\n' "$commit" > "$stage/MANIFEST.toml"
for language in "${languages[@]}"; do
    source="crates/grammars/src/$language/highlights.scm"
    mkdir -p -- "$stage/$language"
    git -C "$ZED_DIR" show "$commit:$source" > "$stage/$language/highlights.scm"
    if command -v sha256sum >/dev/null 2>&1; then
        hash="$(sha256sum "$stage/$language/highlights.scm")"
    else
        hash="$(shasum -a 256 "$stage/$language/highlights.scm")"
    fi
    hash="${hash%% *}"
    deviations='[]'
    if [[ "$language" == javascript ]]; then
        deviations='["Uses the TSX grammar instead of tree-sitter-javascript"]'
    fi
    printf '\n[[queries]]\nname = "%s"\nsource = "%s"\nsha256 = "%s"\ndeviations = %s\n' "$language" "$source" "$hash" "$deviations" >> "$stage/MANIFEST.toml"
done
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
EOF
failed=0
files=(MANIFEST.toml NOTICE)
for language in "${languages[@]}"; do files+=("$language/highlights.scm"); done
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
printf '15 Zed queries verified at %s\n' "$commit"
