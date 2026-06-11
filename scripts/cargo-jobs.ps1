# scripts/cargo-jobs.ps1
#
# Shared Windows-local build-parallelism cap for the heavy Rust gates.
#
# On this Windows dev box a from-scratch, fully parallel `cargo build`/`test`/
# `clippy` of the heavy crates (relux-kernel and the relix mesh: reqwest +
# rustls + axum + libp2p + relix-web-bridge) launches many codegen+link units at
# once and exhausts the system commit limit. The linker then dies with LNK1102
# ("link.exe exit code 1102" / 0xc000012d STATUS_COMMIT_LIMIT) and rustc throws
# an internal compiler error. The crashed units leave PARTIAL artifacts in
# target/, which cascade into BOGUS follow-on errors on an otherwise-green tree:
# "crate X required to be available in rlib format", "found invalid metadata
# files for crate core/test", "can't find crate for relix_runtime". None of
# those are source defects -- the same command succeeds when target/ is warm or
# when peak parallelism is throttled.
#
# Get-CargoJobsArgs returns the `-j <N>` argument fragment to splice into a heavy
# cargo invocation so its PEAK parallelism is capped. A warm/incremental build
# has too few units for the cap to matter, so this does not meaningfully slow the
# common case; it only bounds the cold-build link storm that triggers the OOM.
#
# The cap defaults to 2. Override with $env:RELUX_CARGO_JOBS:
#   - a positive integer  -> use that many jobs (e.g. 4 on a bigger box)
#   - 0 (or negative)     -> no cap; fall back to cargo's default (all cores)
# Setting it to 0 restores the previous unthrottled behaviour for anyone whose
# machine has the headroom.
function Get-CargoJobsArgs {
    $cap = 2
    if ($env:RELUX_CARGO_JOBS) {
        $parsed = 0
        if ([int]::TryParse($env:RELUX_CARGO_JOBS, [ref]$parsed)) { $cap = $parsed }
    }
    if ($cap -le 0) { return @() }
    return @('-j', "$cap")
}
