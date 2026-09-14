#!/usr/bin/env bash
# Canonical Linux development/CI measurement. Do not reuse a run directory:
# cargo clean --workspace can leave older toolchain/layout ELF maps behind.
set -euo pipefail

if (( $# > 1 )); then
    echo 'usage: bash xtask/coverage.sh [output.json]' >&2
    exit 2
fi

coverage_base=${MXRS_COVERAGE_TMPDIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}
coverage_run=$(mktemp -d "$coverage_base/mxrs-coverage.XXXXXX")
coverage_output=${1:-target/coverage.json}
echo "[coverage] fresh evidence directory: $coverage_run" >&2
if [[ -n ${GITHUB_OUTPUT:-} ]]; then
    printf 'evidence_dir=%s\n' "$coverage_run" >> "$GITHUB_OUTPUT"
fi

export RUSTUP_TOOLCHAIN=nightly
export CARGO_LLVM_COV_TARGET_DIR="$coverage_run/workspace"
export MXRS_TEST_CARGO_TARGET_DIR="$coverage_run/nested"
# An inherited custom build directory would defeat isolation for nested Cargo.
unset CARGO_BUILD_BUILD_DIR CARGO_LLVM_COV_BUILD_DIR LLVM_COV_FLAGS LLVM_PROFDATA_FLAGS

mkdir -p "$(dirname "$coverage_output")"
cargo +nightly -Vv > "$coverage_run/toolchain.txt"
rustc +nightly -Vv >> "$coverage_run/toolchain.txt"
source_inventory() {
    rg --files -0 crates xtask Cargo.toml Cargo.lock | sort -z | xargs -0 sha256sum
}
verify_source_inventory() {
    if ! source_inventory | cmp -s - "$coverage_run/source.sha256"; then
        echo '[coverage] source changed during measurement; freeze edits and start a new run' >&2
        exit 1
    fi
}
source_inventory > "$coverage_run/source.sha256"
cargo +nightly llvm-cov --workspace --branch --doctests --no-report
verify_source_inventory

# cargo-llvm-cov's normal collector only recognizes workspace target names.
# Generated application executables have arbitrary names but contain real
# instrumented workspace code. Retain every fresh mapped ELF, not only the
# binaries whose names happen to match the workspace package list.
coverage_objects=()
while IFS= read -r -d '' coverage_object; do
    if readelf -SW "$coverage_object" 2>/dev/null | rg '__llvm_covmap' >/dev/null; then
        if [[ "$coverage_object" =~ [[:space:]] ]]; then
            echo '[coverage] LLVM_COV_FLAGS cannot represent whitespace in object paths; choose a whitespace-free MXRS_COVERAGE_TMPDIR' >&2
            exit 1
        fi
        coverage_objects+=("$coverage_object")
    fi
done < <(find "$coverage_run" -type f -perm /111 -print0 | sort -z)
if (( ${#coverage_objects[@]} == 0 )); then
    echo '[coverage] no instrumented ELF objects found; refusing an empty measurement' >&2
    exit 1
fi

printf '%s\n' "${coverage_objects[@]}" > "$coverage_run/objects.txt"
printf '%s\0' "${coverage_objects[@]}" | xargs -0 sha256sum > "$coverage_run/objects.sha256"
printf -v LLVM_COV_FLAGS ' -object=%s' "${coverage_objects[@]}"
export LLVM_COV_FLAGS
cargo +nightly llvm-cov report --doctests --include-build-script --json --output-path "$coverage_output"
verify_source_inventory
echo "[coverage] ${#coverage_objects[@]} fresh mapped objects; profiles and object hashes retained in $coverage_run" >&2
