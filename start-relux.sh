#!/usr/bin/env bash
# start-relux.sh - launch Relux locally from a SOURCE CHECKOUT (repo root) on
# macOS / Linux (and any Unix-like shell with bash).
#
# This is the macOS/Linux source-checkout counterpart of the Windows
# Start-Relux.ps1 launcher. It builds (or reuses) the relux-kernel binary from
# this checkout, points it at the committed dashboard bundle + the gitignored
# dev-data store, preflights the port, prints the dashboard URL, and runs the
# server in the foreground (Ctrl+C to stop).
#
# There is NO prebuilt macOS/Linux release zip - the packaged release is
# Windows-x64 only. On macOS/Linux you run from a source checkout, and this
# script is the sane local launcher for that path. It mirrors the PowerShell
# launcher's semantics (same env vars, same default port, same port preflight,
# same dry-run / doctor / help affordances).
#
# Usage (from the repo root):
#   ./start-relux.sh
#   ./start-relux.sh --port 20000
#   ./start-relux.sh --release
#   ./start-relux.sh --dry-run
#   ./start-relux.sh --doctor
#   ./start-relux.sh --help
#
# Env:
#   RELUX_CARGO_JOBS   cap cargo build parallelism (-j N) when set to a positive
#                      integer; 0 / unset = cargo default (all cores). Mirrors the
#                      Windows scripts/cargo-jobs.ps1 knob, but is OPT-IN here: the
#                      build-parallelism OOM it guards against is specific to the
#                      Windows dev box, so Unix defaults to no cap.

set -uo pipefail

# -- color helpers (no-op when stdout is not a terminal) -------------------
if [[ -t 1 ]]; then
    C_RED=$'\033[31m'; C_GRN=$'\033[32m'; C_YEL=$'\033[33m'
    C_CYN=$'\033[36m'; C_DIM=$'\033[2m'; C_RST=$'\033[0m'
else
    C_RED=""; C_GRN=""; C_YEL=""; C_CYN=""; C_DIM=""; C_RST=""
fi

# -- defaults / args -------------------------------------------------------
PORT=19891
RELEASE=0
DRYRUN=0
DOCTOR=0

usage() {
    cat <<EOF

${C_CYN}start-relux.sh${C_RST} - run Relux from this source checkout (macOS/Linux).

  --port <n>   Loopback port for the dashboard/API (default 19891).
  --release    Build/use target/release (optimized) instead of target/debug.
  --dry-run    Check prereqs/port/paths and print the plan; build/start nothing.
  --doctor     Run the kernel health check and exit (no server).
  --help       Show this help.

Examples:
  ./start-relux.sh
  ./start-relux.sh --port 20000

Stop a running server with Ctrl+C (it runs in the foreground).
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port)      PORT="${2:-}"; shift 2 ;;
        --port=*)    PORT="${1#*=}"; shift ;;
        --release)   RELEASE=1; shift ;;
        --dry-run)   DRYRUN=1; shift ;;
        --doctor)    DOCTOR=1; shift ;;
        -h|--help)   usage; exit 0 ;;
        *)           echo "${C_RED}unknown arg: $1${C_RST}" >&2; usage >&2; exit 2 ;;
    esac
done

if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [[ "$PORT" -lt 1 || "$PORT" -gt 65535 ]]; then
    echo "${C_RED}ERROR: --port must be a number in 1..65535 (got: ${PORT}).${C_RST}" >&2
    exit 2
fi

# -- repo-root guard -------------------------------------------------------
# The script lives at the repo root; resolve it from the script path so it is
# runnable from any working directory. Confirm we are actually in a Relux
# checkout before doing anything so a misplaced copy fails loudly instead of
# half-building.
ROOT="$(cd "$(dirname "$0")" && pwd)"
KERNEL_TOML="$ROOT/crates/relux-kernel/Cargo.toml"
if [[ ! -f "$ROOT/Cargo.toml" || ! -f "$KERNEL_TOML" ]]; then
    echo "${C_RED}ERROR: this does not look like a Relux source checkout.${C_RST}" >&2
    echo "${C_RED}Expected workspace Cargo.toml and ${KERNEL_TOML} next to this script.${C_RST}" >&2
    echo "${C_YEL}Run start-relux.sh from the root of the cloned repository.${C_RST}" >&2
    exit 1
fi

# -- prerequisites ---------------------------------------------------------
HAVE_CARGO=0
CARGO_PATH=""
if command -v cargo >/dev/null 2>&1; then
    HAVE_CARGO=1
    CARGO_PATH="$(command -v cargo)"
fi

# -- binary resolution -----------------------------------------------------
if [[ "$RELEASE" -eq 1 ]]; then
    EXE="$ROOT/target/release/relux-kernel"
    PROFILE="release"
    BUILD_ARGS=(build -p relux-kernel --release)
else
    EXE="$ROOT/target/debug/relux-kernel"
    PROFILE="debug"
    BUILD_ARGS=(build -p relux-kernel)
fi
HAVE_EXE=0
[[ -x "$EXE" ]] && HAVE_EXE=1

# Optional build-parallelism cap (opt-in; see header). Mirrors the Windows
# RELUX_CARGO_JOBS knob but defaults to no cap on Unix.
JOBS_ARGS=()
if [[ -n "${RELUX_CARGO_JOBS:-}" ]] && [[ "$RELUX_CARGO_JOBS" =~ ^-?[0-9]+$ ]] && [[ "$RELUX_CARGO_JOBS" -gt 0 ]]; then
    JOBS_ARGS=(-j "$RELUX_CARGO_JOBS")
fi

# -- dashboard bundle ------------------------------------------------------
DASHBOARD_DIST="$ROOT/crates/relix-web-bridge/dashboard-dist"
HAVE_DASHBOARD=0
[[ -f "$DASHBOARD_DIST/index.html" ]] && HAVE_DASHBOARD=1

# -- data store (gitignored dev-data, resolved against the repo root) -------
DATA_DB="$ROOT/dev-data/relux/local.db"

# -- port preflight --------------------------------------------------------
# A second Relux (or any other process) already holding the loopback port is the
# most common first-run failure; the kernel would bind-fail AFTER a URL is
# printed. Probe 127.0.0.1:$PORT up front and stop with an actionable message.
# Prefer bash's /dev/tcp; fall back to nc; if neither works, skip the probe
# honestly (the kernel still reports a clear bind error on a real conflict).
PORT_PROBE="unsupported"   # unsupported | free | busy
probe_port() {
    local p="$1"
    # Prefer nc when present: a definitive loopback connect probe.
    if command -v nc >/dev/null 2>&1; then
        if nc -z -w1 127.0.0.1 "$p" >/dev/null 2>&1; then PORT_PROBE="busy"; else PORT_PROBE="free"; fi
        return
    fi
    # bash /dev/tcp fallback. Inspect the failure reason so a bash built WITHOUT
    # /dev/tcp is reported as "unsupported" rather than a false "free".
    local err rc
    # The connect happens in a subshell, so the socket fd is closed when it
    # exits - nothing to clean up in this shell.
    err="$( (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>&1 )"; rc=$?
    if [[ "$rc" -eq 0 ]]; then
        PORT_PROBE="busy"
    elif [[ "$err" == *"refused"* || "$err" == *"timed out"* ]]; then
        PORT_PROBE="free"
    else
        PORT_PROBE="unsupported"
    fi
}
probe_port "$PORT"

alt_port() { [[ "$1" -eq 20000 ]] && echo 20001 || echo 20000; }
ALT="$(alt_port "$PORT")"

# -- dry run: report the plan and exit, building/starting nothing -----------
if [[ "$DRYRUN" -eq 1 ]]; then
    echo ""
    echo "${C_CYN}== start-relux dry run ==${C_RST}"
    echo "  repo root        : $ROOT"
    if [[ "$HAVE_CARGO" -eq 1 ]]; then
        echo "  cargo present    : ${C_GRN}yes ($CARGO_PATH)${C_RST}"
    else
        echo "  cargo present    : ${C_YEL}NO - install Rust (https://rustup.rs)${C_RST}"
    fi
    echo "  build profile    : $PROFILE"
    if [[ "$HAVE_EXE" -eq 1 ]]; then
        echo "  binary           : ${C_GRN}$EXE (reuse)${C_RST}"
    else
        echo "  binary           : ${C_YEL}$EXE (would build)${C_RST}"
    fi
    if [[ "$HAVE_DASHBOARD" -eq 1 ]]; then
        echo "  dashboard bundle : ${C_GRN}$DASHBOARD_DIST${C_RST}"
    else
        echo "  dashboard bundle : ${C_YEL}MISSING - run 'npm run build' in apps/dashboard${C_RST}"
    fi
    echo "  data store       : ${C_DIM}$DATA_DB${C_RST}"
    case "$PORT_PROBE" in
        busy) echo "  port $PORT        : ${C_YEL}IN USE - try --port $ALT${C_RST}" ;;
        free) echo "  port $PORT        : ${C_GRN}free${C_RST}" ;;
        *)    echo "  port $PORT        : ${C_DIM}not probed (no /dev/tcp or nc; the kernel checks on bind)${C_RST}" ;;
    esac
    echo "  dashboard URL    : ${C_GRN}http://127.0.0.1:$PORT/dashboard${C_RST}"
    echo "  API URL          : http://127.0.0.1:$PORT/v1/relux/state"
    echo ""
    echo "${C_DIM}Dry run only - nothing was built or started.${C_RST}"
    echo ""
    exit 0
fi

# -- port guard (real run) -------------------------------------------------
if [[ "$PORT_PROBE" == "busy" ]]; then
    echo ""
    echo "${C_RED}ERROR: port $PORT on 127.0.0.1 is already in use; Relux did not start.${C_RST}" >&2
    echo "${C_YEL}The most likely cause is that Relux is already running on this port.${C_RST}" >&2
    echo "${C_CYN}  If so, open the running instance: http://127.0.0.1:$PORT/dashboard${C_RST}" >&2
    echo "${C_YEL}Otherwise another program holds that port. Start Relux on a free port, e.g.:${C_RST}" >&2
    echo "${C_GRN}  ./start-relux.sh --port $ALT${C_RST}" >&2
    echo ""
    exit 1
fi

# -- build the binary if needed --------------------------------------------
if [[ "$HAVE_EXE" -eq 0 ]]; then
    if [[ "$HAVE_CARGO" -eq 0 ]]; then
        echo "${C_RED}ERROR: relux-kernel is not built yet and 'cargo' is not on PATH.${C_RST}" >&2
        echo "${C_YEL}Install the Rust toolchain (https://rustup.rs):${C_RST}" >&2
        echo "${C_YEL}  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh${C_RST}" >&2
        echo "${C_YEL}then reopen the shell (so cargo is on PATH) and re-run ./start-relux.sh.${C_RST}" >&2
        exit 1
    fi
    echo ""
    echo "${C_CYN}Building relux-kernel ($PROFILE) from source - first build can take a few minutes ...${C_RST}"
    echo "${C_DIM}  cargo ${BUILD_ARGS[*]} ${JOBS_ARGS[*]}${C_RST}"
    ( cd "$ROOT" && cargo "${BUILD_ARGS[@]}" "${JOBS_ARGS[@]}" )
    if [[ $? -ne 0 || ! -x "$EXE" ]]; then
        echo "${C_RED}ERROR: build failed; see the cargo output above.${C_RST}" >&2
        exit 1
    fi
fi

# -- environment (resolved against the repo root) --------------------------
export RELUX_HTTP_ADDR="127.0.0.1:$PORT"
export RELUX_DB="$DATA_DB"
mkdir -p "$(dirname "$DATA_DB")"
if [[ "$HAVE_DASHBOARD" -eq 1 ]]; then
    export RELUX_DASHBOARD_DIST="$DASHBOARD_DIST"
else
    echo "${C_YEL}WARNING: dashboard bundle not found at crates/relix-web-bridge/dashboard-dist;${C_RST}"
    echo "${C_YEL}         the /dashboard route returns an honest 'not built' notice.${C_RST}"
    echo "${C_YEL}         Build it with: (cd apps/dashboard && npm install && npm run build)${C_RST}"
fi

# -- doctor mode: health check and exit ------------------------------------
if [[ "$DOCTOR" -eq 1 ]]; then
    ( cd "$ROOT" && "$EXE" doctor )
    exit $?
fi

# -- serve (foreground; Ctrl+C to stop) ------------------------------------
echo ""
echo "${C_CYN}Starting Relux ($PROFILE build) ...${C_RST}"
echo "${C_GRN}  Dashboard: http://127.0.0.1:$PORT/dashboard${C_RST}"
echo "  API:       http://127.0.0.1:$PORT/v1/relux/state"
echo "${C_DIM}  Data:      $DATA_DB${C_RST}"
echo "${C_DIM}  Press Ctrl+C to stop.${C_RST}"
echo ""

cd "$ROOT"
exec "$EXE" serve
