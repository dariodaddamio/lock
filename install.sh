#!/bin/sh
# Installs the lock binary from the latest GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/dariodaddamio/lock/main/install.sh | sh
#
# LOCK_VERSION picks a release tag, such as v0.1.0. LOCK_INSTALL_DIR picks the
# directory, which is ~/.local/bin by default.
set -eu

repo="dariodaddamio/lock"
dir="${LOCK_INSTALL_DIR:-$HOME/.local/bin}"
binary="lock"
archive="tar.gz"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64 | Linux-amd64) target="x86_64-unknown-linux-musl" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-musl" ;;
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64)
    target="x86_64-pc-windows-msvc"; binary="lock.exe"; archive="zip" ;;
  *)
    echo "lock: there is no prebuilt binary for $(uname -s) $(uname -m)." >&2
    echo "Build it with Rust instead: cargo install lock-hook --locked" >&2
    exit 1 ;;
esac

if [ -n "${LOCK_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/$LOCK_VERSION"
else
  base="https://github.com/$repo/releases/latest/download"
fi
name="lock-$target.$archive"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $base/$name"
curl -fsSL "$base/$name" -o "$tmp/$name"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
expected="$(grep " $name\$" "$tmp/SHA256SUMS" | cut -d ' ' -f 1)"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$name" | cut -d ' ' -f 1)"
else
  actual="$(shasum -a 256 "$tmp/$name" | cut -d ' ' -f 1)"
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "lock: the download does not match its checksum in SHA256SUMS, so it was not installed." >&2
  exit 1
fi
if [ "$archive" = zip ]; then
  unzip -q "$tmp/$name" -d "$tmp"
else
  tar -xzf "$tmp/$name" -C "$tmp"
fi
mkdir -p "$dir"
mv "$tmp/$binary" "$dir/$binary"
chmod +x "$dir/$binary"
echo "Installed $("$dir/$binary" --version) to $dir/$binary"

case ":$PATH:" in
  *":$dir:"*) echo "Next, run 'lock install' in the root of each repository that you want to protect." ;;
  *)
    echo
    echo "$dir is not on your PATH. Add this line to your shell profile, then open a new terminal:"
    echo "  export PATH=\"$dir:\$PATH\""
    ;;
esac
