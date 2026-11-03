#!/usr/bin/env bash
# Runs a release binary the way users have it: installed as
# `$CARGO_HOME/bin/cargo`, outside CI, so the background self-update starts.
#
#   ci/smoke-installed.sh path/to/cargo[.exe]
#
# Checks that `cargo -V` prints the version and returns at once even while the
# update check hangs (its output is read through a pipe, as terminals and IDEs
# do, so an inherited handle would hold it open), and that the update check
# itself completes.
set -euo pipefail

bin="$1"
exe=""
case "$bin" in *.exe) exe=".exe" ;; esac

home="$(mktemp -d)"
# Windows cargo needs a Windows path, not an MSYS one.
if command -v cygpath >/dev/null; then home="$(cygpath -m "$home")"; fi
python="$(command -v python3 || command -v python)"
mkdir -p "$home/bin"
cp "$bin" "$home/bin/cargo$exe"
cargo="$home/bin/cargo$exe"
log="$home/.fork-update.log"

run() {
    env -u CI -u GITHUB_ACTIONS -u CARGO_FORK_AUTO_UPDATE CARGO_HOME="$home" "$@"
}

# A proxy that accepts connections and never answers: the update check hangs
# until its 30 s timeout.
"$python" - <<'EOF' &
import socket, time
s = socket.socket()
s.bind(("127.0.0.1", 18080))
s.listen(16)
conns = []
while True:
    conns.append(s.accept()[0])
EOF
proxy=$!
trap 'kill $proxy 2>/dev/null || true' EXIT
sleep 1

start=$SECONDS
out="$(run https_proxy=http://127.0.0.1:18080 HTTPS_PROXY=http://127.0.0.1:18080 "$cargo" -V)"
elapsed=$((SECONDS - start))
echo "cargo -V: $out (${elapsed}s)"
case "$out" in cargo\ *) ;; *) echo "unexpected output: $out" >&2; exit 1 ;; esac
if [ "$elapsed" -gt 10 ]; then
    echo "cargo -V waited ${elapsed}s for the background update" >&2
    exit 1
fi
test -f "$home/.fork-update" || { echo "the update check did not start" >&2; exit 1; }

# The hung check fails on its timeout and logs it.
for _ in $(seq 90); do [ -f "$log" ] && break; sleep 1; done
echo "hung check: $(cat "$log" 2>/dev/null || echo 'no log')"
grep -q failed "$log"

# A real check against GitHub completes.
rm -f "$log" "$home/.fork-update"
run "$cargo" -V >/dev/null
for _ in $(seq 90); do [ -f "$log" ] && break; sleep 1; done
echo "check: $(cat "$log" 2>/dev/null || echo 'no log')"
grep -qE 'up to date|updated' "$log"

# Still runs afterwards.
run "$cargo" -V
