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
    echo "Build it with Rust instead: cargo install --git https://github.com/$repo --locked" >&2
    exit 1 ;;
esac

if [ -n "${LOCK_VERSION:-}" ]; then
  url="https://github.com/$repo/releases/download/$LOCK_VERSION/lock-$target.$archive"
else
  url="https://github.com/$repo/releases/latest/download/lock-$target.$archive"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/lock.$archive"
if [ "$archive" = zip ]; then
  unzip -q "$tmp/lock.$archive" -d "$tmp"
else
  tar -xzf "$tmp/lock.$archive" -C "$tmp"
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
