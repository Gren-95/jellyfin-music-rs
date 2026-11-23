#!/bin/bash
# Backticks in single quotes below are Markdown code spans, not substitutions.
# shellcheck disable=SC2016
# The monthly checkup (.github/workflows/checkup.yml). Also runs by hand from
# the repo root: .github/scripts/checkup.sh [report.md]
#
# Each check appends a Markdown section to the report only when something needs
# a look, so an empty report means all clear. Needs cargo, cargo-audit and jq.
# The working tree is left as it was: Cargo.lock is put back after trying the
# dependency updates.
set -uo pipefail

REPORT=${1:-checkup.md}
LOG_LINES=40

LOG_DIR=$(mktemp -d)
trap 'rm -rf "$LOG_DIR"' EXIT

# run_check <name> <command...>: runs the command with its output kept in
# $LOG_DIR/<name>.log, and succeeds or fails like it.
run_check() {
    local name=$1
    shift
    "$@" >"$LOG_DIR/$name.log" 2>&1
}

# log_block <name> <summary>: the last lines of a check's output, folded.
log_block() {
    printf '<details><summary>%s</summary>\n\n```\n' "$2"
    tail -n "$LOG_LINES" "$LOG_DIR/$1.log"
    printf '```\n\n</details>\n\n'
}

# Formatting, lints and tests with the locked dependencies, on whatever Rust
# is installed (the newest stable in the workflow): new lints and toolchain
# changes show up here first.
check_toolchain() {
    local -a names=(fmt clippy test)
    local -A commands=(
        [fmt]="cargo fmt --check -- --color never"
        [clippy]="cargo clippy --locked --all-targets -- -D warnings"
        [test]="cargo test --locked"
    )
    local name failed=() cmd
    for name in "${names[@]}"; do
        read -ra cmd <<<"${commands[$name]}"
        run_check "$name" "${cmd[@]}" || failed+=("$name")
    done
    ((${#failed[@]})) || return 0
    {
        printf '## Build on %s\n\n' "$(rustc --version)"
        printf 'These fail with the locked dependencies:\n\n'
        for name in "${failed[@]}"; do
            log_block "$name" "<code>${commands[$name]}</code>"
        done
    } >>"$REPORT"
}

# RustSec advisories against Cargo.lock, including yanked and unmaintained
# crates.
check_advisories() {
    run_check audit cargo audit --deny warnings && return 0
    {
        printf '## Security advisories\n\n'
        printf '`cargo audit` reports advisories, or yanked or unmaintained crates, in Cargo.lock:\n\n'
        log_block audit "<code>cargo audit --deny warnings</code>"
    } >>"$REPORT"
}

# Newer versions of the dependencies: the ones Cargo.toml already allows are
# tried against the tests; newer major versions are listed for the direct
# dependencies only (the others follow whatever their parents require).
check_dependencies() {
    cp Cargo.lock "$LOG_DIR/Cargo.lock"
    if ! run_check update cargo update --verbose; then
        {
            printf '## Dependencies\n\n'
            log_block update "<code>cargo update</code> failed"
        } >>"$REPORT"
        return
    fi

    local direct updated behind tests=""
    direct=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].dependencies[].name')
    updated=$(grep -E '^\s+(Updating \S+ v\S+ -> v|Downgrading \S+ v\S+ -> v|Adding \S+ v|Removing \S+ v)' "$LOG_DIR/update.log" |
        sed -E 's/^\s+/- /')
    # "Unchanged ureq v3.1.0 (available: v4.0.0)"
    behind=$(grep -E '^\s+Unchanged ' "$LOG_DIR/update.log" |
        while read -r _ name version _ latest; do
            grep -qxF "$name" <<<"$direct" && echo "- $name $version, newest ${latest%)}"
        done)

    if [[ -n "$updated" ]]; then
        if run_check updated-test cargo test; then
            tests='The tests pass with them: run `cargo update` and commit Cargo.lock.'
        else
            tests=$(printf 'The tests fail with them:\n\n%s' "$(log_block updated-test '<code>cargo test</code> after <code>cargo update</code>')")
        fi
    fi
    cp "$LOG_DIR/Cargo.lock" Cargo.lock

    [[ -n "$updated$behind" ]] || return 0
    {
        printf '## Dependencies\n\n'
        if [[ -n "$updated" ]]; then
            printf 'Updates within the versions Cargo.toml allows:\n\n%s\n\n%s\n\n' "$updated" "$tests"
        fi
        if [[ -n "$behind" ]]; then
            printf 'New major versions (raise them in Cargo.toml):\n\n%s\n\n' "$behind"
        fi
    } >>"$REPORT"
}

: >"$REPORT"
check_toolchain
check_advisories
check_dependencies
