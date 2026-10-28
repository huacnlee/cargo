#!/bin/sh
# Install huacnlee/cargo in place of the rustup `cargo` proxy. No sudo required.
#
#   curl -fsSL https://raw.githubusercontent.com/huacnlee/cargo/HEAD/install.sh | sh
#   sh install.sh --uninstall   # put the rustup proxy back
set -eu

version="${CARGO_FORK_VERSION:-latest}"
destination="${CARGO_FORK_INSTALL_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
uninstall=
usage() {
  printf '%s\n' 'Usage: install.sh [--version VERSION] [--dir DIRECTORY] [--uninstall]' \
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
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
die() { printf 'cargo installer: %s\n' "$*" >&2; exit 1; }
target="$destination/cargo"
backup="$destination/cargo.rustup"

if [ -n "$uninstall" ]; then
  [ -e "$backup" ] || die "Nothing to restore: $backup does not exist."
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
# Keep the rustup proxy (or whatever cargo was there) the first time only.
if [ -e "$target" ] && [ ! -e "$backup" ]; then cp -p "$target" "$backup"; fi
# Rename into place so running cargo processes keep their old inode.
staged="$(mktemp "$destination/.cargo.XXXXXXXX")"
trap 'rm -rf "$work"; rm -f "${staged:-}"' 0
install -m755 "$work/unpacked/cargo" "$staged"
mv -f "$staged" "$target"
printf 'Installed %s\n' "$target"
[ -e "$backup" ] && printf 'Previous cargo kept as %s (restore with --uninstall).\n' "$backup"
if command -v rustup >/dev/null; then
  printf '%s\n' 'Note: `rustup self update` reinstalls its proxies; rerun this script afterwards.'
else
  printf '%s\n' 'Note: rustup not found; cargo will use the rustc on PATH.'
fi
case ":$PATH:" in *":$destination:"*) ;; *) printf 'Add %s to PATH.\n' "$destination" ;; esac
