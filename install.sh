#!/bin/sh
# Install huacnlee/cargo in place of the rustup `cargo` proxy. No sudo required.
#
#   curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh
#   sh install.sh --uninstall   # put the rustup proxy back
#
# rustup puts its proxy back on `rustup self update` (which `rustup update`
# runs), and from then on every build silently uses the stock Cargo. So this
# also turns off rustup's automatic self-update, and installs a guard (a
# launchd agent on macOS, a systemd user unit on Linux) that puts this cargo
# back whenever something replaces it. `--no-guard` skips the guard.
set -eu

version="${CARGO_FORK_VERSION:-latest}"
destination="${CARGO_FORK_INSTALL_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
uninstall=
guard=1
usage() {
  printf '%s\n' 'Usage: install.sh [--version VERSION] [--dir DIRECTORY] [--no-guard] [--uninstall]' \
    'Defaults: latest release, installed as $CARGO_HOME/bin/cargo (~/.cargo/bin/cargo).' \
    'The cargo it replaces is kept as cargo.rustup; --uninstall restores it.'
}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --version|--dir)
      [ "$#" -ge 2 ] || { usage >&2; exit 2; }
      if [ "$1" = --version ]; then version="$2"; else destination="$2"; fi
      shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    --no-guard) guard=; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
die() { printf 'cargo installer: %s\n' "$*" >&2; exit 1; }
target="$destination/cargo"
backup="$destination/cargo.rustup"
# What the guard restores from, and the guard itself.
keep="$destination/.cargo-fork"
label=com.github.huacnlee.cargo-guard
plist="$HOME/Library/LaunchAgents/$label.plist"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
is_fork() { grep -aq 'huacnlee/cargo' "$1"; }
# Keeps what is at $target (usually a link to rustup) as $backup.
backup_target() {
  if [ -L "$target" ]; then ln -sfn "$(readlink "$target")" "$backup"
  else rm -f "$backup"; cp -p "$target" "$backup"; fi
}
remove_guard() {
  if [ -f "$plist" ]; then
    launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || launchctl unload "$plist" >/dev/null 2>&1 || true
    rm -f "$plist"
  fi
  if [ -f "$units/cargo-fork-guard.path" ]; then
    systemctl --user disable --now cargo-fork-guard.path >/dev/null 2>&1 || true
    rm -f "$units/cargo-fork-guard.path" "$units/cargo-fork-guard.service"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
}

if [ -n "$uninstall" ]; then
  [ -e "$backup" ] || [ -L "$backup" ] || die "Nothing to restore: $backup does not exist."
  if is_fork "$backup"; then die "$backup is huacnlee/cargo, not the original; reinstall rustup from https://rustup.rs."; fi
  # First, or the guard would put this cargo back.
  remove_guard
  if [ -f "$keep/auto-self-update" ] && command -v rustup >/dev/null; then
    rustup set auto-self-update "$(cat "$keep/auto-self-update")" >/dev/null 2>&1 || true
  fi
  rm -rf "$keep"
  mv -f "$backup" "$target"
  printf 'Restored %s\n' "$target"
  exit 0
fi

for tool in curl tar awk; do command -v "$tool" >/dev/null || die "Required command missing: $tool"; done
case "$(uname -s)" in
  Darwin) platform=macOS; suffix=apple-darwin ;;
  Linux) platform=Linux; suffix=unknown-linux-gnu ;;
  *) die 'Use install.ps1 on Windows. This script supports macOS and Linux.' ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  arm64|aarch64) arch=aarch64 ;;
  *) die 'Unsupported CPU architecture (requires x86_64 or arm64).' ;;
esac
download() { curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location "$@"; }
repo=https://github.com/huacnlee/cargo
if [ "$version" = latest ]; then
  release_url="$(download --output /dev/null --write-out '%{url_effective}' "$repo/releases/latest")"
  version="${release_url##*/}"
fi
version="${version#v}"
awk -v version="$version" 'BEGIN {exit !(version ~ /^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$/)}' || die 'Version must be a release version such as 0.102.0-1 or v0.102.0-1.'
asset="cargo-$version-$arch-$suffix.tar.gz"
base="$repo/releases/download/v$version"
work="$(mktemp -d "${TMPDIR:-/tmp}/cargo-install.XXXXXXXX")"
trap 'rm -rf "$work"' 0
printf 'Downloading cargo %s for %s %s…\n' "$version" "$platform" "$arch"
download "$base/$asset" --output "$work/$asset"
download "$base/SHA256SUMS" --output "$work/SHA256SUMS"
expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$work/SHA256SUMS")"
[ "${#expected}" -eq 64 ] || die "Missing or invalid checksum for $asset"
case "$expected" in *[!a-fA-F0-9]*) die "Missing or invalid checksum for $asset" ;; esac
if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$work/$asset")"
elif command -v shasum >/dev/null; then
  actual="$(shasum -a 256 "$work/$asset")"
else die 'Required command missing: sha256sum or shasum'; fi
[ "${actual%% *}" = "$expected" ] || die 'Checksum verification failed; nothing was installed.'
mkdir "$work/unpacked"
# Only accept relative archive paths inside the temporary extraction directory.
tar -tzf "$work/$asset" > "$work/entries"
if awk '/^\// || /(^|\/)\.\.(\/|$)/ {bad=1} END {exit !bad}' "$work/entries"; then
  die 'Release archive contains an unsafe path.'
fi
tar -xzf "$work/$asset" -C "$work/unpacked"
[ -f "$work/unpacked/cargo" ] || die 'Release archive is missing cargo.'
"$work/unpacked/cargo" --version >/dev/null 2>&1 || die 'Downloaded cargo does not run on this system.'

mkdir -p "$destination"
# Keep the rustup proxy (or whatever cargo was there), the newest one when
# rustup put it back since. The backup must never be a build of this cargo.
if [ -e "$backup" ] && is_fork "$backup"; then rm -f "$backup"; fi
if { [ -e "$target" ] || [ -L "$target" ]; } && ! is_fork "$target"; then backup_target; fi
# Rename into place so running cargo processes keep their old inode.
staged="$(mktemp "$destination/.cargo.XXXXXXXX")"
trap 'rm -rf "$work"; rm -f "${staged:-}"' 0
install -m755 "$work/unpacked/cargo" "$staged"
mv -f "$staged" "$target"
printf 'Installed %s\n' "$target"
[ -e "$backup" ] && printf 'Previous cargo kept as %s (restore with --uninstall).\n' "$backup"

mkdir -p "$keep"
install -m755 "$target" "$keep/cargo"
remove_guard
if [ -n "$guard" ]; then
  # Run by launchd or systemd whenever $destination changes: puts this cargo
  # back if something replaced it, and follows its own updates otherwise.
  cat > "$keep/guard.sh" <<GUARD
#!/bin/sh
bin='$destination'
GUARD
  cat >> "$keep/guard.sh" <<'GUARD'
target="$bin/cargo"; keep="$bin/.cargo-fork/cargo"
is_fork() { grep -aq 'huacnlee/cargo' "$1" 2>/dev/null; }
[ -f "$keep" ] && is_fork "$keep" || exit 0
if is_fork "$target"; then
  cmp -s "$target" "$keep" || { cp "$target" "$keep.tmp" && chmod 755 "$keep.tmp" && mv -f "$keep.tmp" "$keep"; }
  exit 0
fi
if [ -L "$target" ]; then ln -sfn "$(readlink "$target")" "$bin/cargo.rustup"
elif [ -e "$target" ]; then rm -f "$bin/cargo.rustup"; cp -p "$target" "$bin/cargo.rustup"; fi
staged="$(mktemp "$bin/.cargo.XXXXXXXX")" || exit 1
cp "$keep" "$staged" && chmod 755 "$staged" && mv -f "$staged" "$target" || { rm -f "$staged"; exit 1; }
printf '%s put huacnlee/cargo back after %s was replaced\n' "$(date +%s)" "$target" >> "$bin/.cargo-fork/guard.log"
GUARD
  chmod 755 "$keep/guard.sh"
  case "$platform" in
    macOS)
      mkdir -p "$(dirname "$plist")"
      cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key><array><string>/bin/sh</string><string>$keep/guard.sh</string></array>
  <key>WatchPaths</key><array><string>$destination</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
PLIST
      if launchctl bootstrap "gui/$(id -u)" "$plist" >/dev/null 2>&1 || launchctl load -w "$plist" >/dev/null 2>&1; then
        printf 'Guard installed: %s puts this cargo back if rustup replaces it.\n' "$plist"
      else
        rm -f "$plist"; guard=
      fi ;;
    Linux)
      if command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
        mkdir -p "$units"
        printf '%s\n' '[Unit]' 'Description=Put huacnlee/cargo back if something replaces it' '' \
          '[Path]' "PathChanged=$destination" '' '[Install]' 'WantedBy=default.target' > "$units/cargo-fork-guard.path"
        printf '%s\n' '[Unit]' 'Description=Put huacnlee/cargo back if something replaces it' '' \
          '[Service]' 'Type=oneshot' "ExecStart=/bin/sh $keep/guard.sh" > "$units/cargo-fork-guard.service"
        if systemctl --user daemon-reload && systemctl --user enable --now cargo-fork-guard.path >/dev/null 2>&1; then
          printf 'Guard installed: systemd user unit cargo-fork-guard.path puts this cargo back if rustup replaces it.\n'
        else
          rm -f "$units/cargo-fork-guard.path" "$units/cargo-fork-guard.service"; guard=
        fi
      else guard=; fi ;;
  esac
  [ -n "$guard" ] || printf '%s\n' 'Note: no guard could be installed; rerun this script after `rustup self update`.'
fi

if command -v rustup >/dev/null; then
  # `rustup update` updates rustup itself first, and that puts its proxy back
  # in place of this cargo.
  if [ ! -f "$keep/auto-self-update" ]; then
    previous="$(sed -n 's/^auto_self_update *= *"\(.*\)"/\1/p' "${RUSTUP_HOME:-$HOME/.rustup}/settings.toml" 2>/dev/null || true)"
    printf '%s\n' "${previous:-enable}" > "$keep/auto-self-update"
  fi
  if rustup set auto-self-update disable >/dev/null 2>&1; then
    printf '%s\n' 'Turned off rustup self-update in `rustup update` (it would put the stock cargo back); run `rustup self update` to update rustup.'
  fi
  # Nightly builds use Cranelift for local crates; it ships as a component.
  if rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
    printf '%s\n' 'Adding the Cranelift backend to the nightly toolchain…'
    rustup component add rustc-codegen-cranelift-preview --toolchain nightly >/dev/null 2>&1 ||
      printf '%s\n' 'Cranelift is not available for this nightly; nightly builds use LLVM.'
  fi
  [ -n "$guard" ] || printf '%s\n' 'Note: `rustup self update` reinstalls its proxies; rerun this script afterwards.'
else
  printf '%s\n' 'Note: rustup not found; cargo will use the rustc on PATH.'
fi
case ":$PATH:" in *":$destination:"*) ;; *) printf 'Add %s to PATH.\n' "$destination" ;; esac
