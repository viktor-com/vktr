#!/usr/bin/env bash
# Build a distributable vktr tarball.
#
#   scripts/dist.sh                                   -> dist/vktr-<version>-<host>.tar.gz (+ .sha256)
#   VKTR_DIST_TARGET=aarch64-unknown-linux-gnu scripts/dist.sh
#
# The working tree must be clean (tracked files), so the commit in `vktr --version` is the one
# the binary was built from.
#
# Linux gnu targets are built with cargo-zigbuild against an old glibc (VKTR_DIST_GLIBC, default
# 2.28), so the binary runs on RHEL 8, Debian 10, Ubuntu 20.04 and newer instead of needing the
# build host's glibc. VKTR_DIST_GLIBC=host builds with plain cargo for the host only. A Linux binary
# may link glibc's own libraries and nothing else.
#
# macOS targets are built natively (the release workflow builds Apple Silicon only; Intel Macs get
# no prebuilt binary) for macOS VKTR_DIST_MACOS (default 11.0) and newer, may link only system
# libraries, and carry an ad-hoc code signature (Apple Silicon refuses to run unsigned code).
#
# The tarball holds the stripped binary plus the files the Apache-2.0 license requires anyone
# redistributing vktr to ship: LICENSE, NOTICE and THIRD-PARTY-NOTICES.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# The binary stamps `git rev-parse HEAD` into `vktr --version`; a build from uncommitted changes
# would name a commit that does not contain the shipped code.
if [ "${VKTR_DIST_ALLOW_DIRTY:-0}" != 1 ] && [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "dist: the working tree has uncommitted changes; commit them first (VKTR_DIST_ALLOW_DIRTY=1 to override)" >&2
  exit 1
fi

VERSION="$(grep -m1 '^version' crates/codegen/xai-grok-pager-bin/Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
TARGET="${VKTR_DIST_TARGET:-$HOST}"
GLIBC="${VKTR_DIST_GLIBC:-2.28}"
MACOS="${VKTR_DIST_MACOS:-11.0}"
NAME="vktr-$VERSION-$TARGET"
OUT="$ROOT/dist"
# Panic locations embed source paths; map the build user's home, cargo home and checkout to
# neutral prefixes so a release names no build machine. rustc tries the last match first.
CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
REMAP="--remap-path-prefix=$HOME=/home --remap-path-prefix=$CARGO_DIR=/cargo --remap-path-prefix=$ROOT=/vktr"

if [[ "$TARGET" == *-linux-gnu && "$GLIBC" != host ]]; then
  BIN="$ROOT/target/$TARGET/vktr-dist/vktr"
  # The target's rustflags are replaced (not appended) so the path remapping always applies and a
  # CPU-specific flag in a local .cargo/config.toml cannot leak into a download.
  flags_var="CARGO_TARGET_$(echo "$TARGET" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"
  if [ "${SKIP_BUILD:-0}" != 1 ]; then
    env "$flags_var=-C force-unwind-tables=yes $REMAP" \
      cargo zigbuild -p xai-grok-pager-bin --profile vktr-dist --target "$TARGET.$GLIBC"
  fi
else
  [ "$TARGET" = "$HOST" ] || { echo "dist: $TARGET needs VKTR_DIST_GLIBC (cargo-zigbuild)" >&2; exit 1; }
  BIN="$ROOT/target/vktr-dist/vktr"
  # rustc and the cc crate (C dependencies) both read this; without it the floor is the build host.
  [[ "$TARGET" == *-apple-darwin ]] && export MACOSX_DEPLOYMENT_TARGET="$MACOS"
  if [ "${SKIP_BUILD:-0}" != 1 ]; then
    # --config arrays are appended to the target's rustflags from .cargo/config.toml.
    remap_toml="$(printf '"%s",' $REMAP)"
    cargo build -p xai-grok-pager-bin --profile vktr-dist --config "target.$TARGET.rustflags=[${remap_toml%,}]"
  fi
fi
[ -x "$BIN" ] || { echo "dist: $BIN not found; build failed?" >&2; exit 1; }
if strings "$BIN" | grep -F "$HOME/" >/dev/null; then
  echo "dist: $BIN still contains build paths under $HOME" >&2; exit 1
fi
COMMIT="$(git rev-parse --short=12 HEAD)"
if ! strings "$BIN" | grep -F "$VERSION ($COMMIT)" >/dev/null; then
  echo "dist: $BIN is not stamped \"$VERSION ($COMMIT)\"; it was built from another commit (rebuild without SKIP_BUILD)" >&2; exit 1
fi
if [[ "$TARGET" == *-linux-gnu && "$GLIBC" != host ]]; then
  newest="$(readelf -W --dyn-syms "$BIN" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"
  [ "$(printf '%s\n%s\n' "$newest" "$GLIBC" | sort -V | tail -1)" = "$GLIBC" ] \
    || { echo "dist: $BIN needs glibc $newest, above the $GLIBC floor" >&2; exit 1; }
  echo "dist: $TARGET binary needs glibc <= $GLIBC (newest symbol: $newest)"
  # Anything beyond glibc (libssl, libstdc++, ...) would be a runtime dependency users may lack.
  extra="$(readelf -d "$BIN" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' \
    | grep -vE '^(libc|libm|libdl|libpthread|librt|libutil|ld-linux[-a-z0-9_]*)\.so\.[0-9]+$' || true)"
  [ -z "$extra" ] || { echo "dist: $BIN links libraries outside glibc: $(tr '\n' ' ' <<<"$extra")" >&2; exit 1; }
fi
if [[ "$TARGET" == *-apple-darwin ]]; then
  minos="$(otool -l "$BIN" | awk '/LC_BUILD_VERSION/{f=1} f && $1=="minos"{print $2; exit}')"
  # awk, not sort -V: the build runs on the stock macOS userland.
  if [ -z "$minos" ] || ! awk -v a="$minos" -v b="$MACOS" 'BEGIN { split(a, x, "."); split(b, y, ".");
      for (i = 1; i <= 3; i++) if (x[i] + 0 != y[i] + 0) exit !(x[i] + 0 < y[i] + 0); exit 0 }'; then
    echo "dist: $BIN needs macOS ${minos:-?}, above the $MACOS floor" >&2; exit 1
  fi
  # Homebrew or other non-system dylibs would not exist on a user's Mac.
  extra="$(otool -L "$BIN" | tail -n +2 | awk '{print $1}' | grep -vE '^(/usr/lib/|/System/Library/)' || true)"
  [ -z "$extra" ] || { echo "dist: $BIN links non-system libraries: $(tr '\n' ' ' <<<"$extra")" >&2; exit 1; }
  echo "dist: $TARGET binary needs macOS >= $minos"
fi

# Stage next to the output, not in /tmp: the binary is ~200 MB and /tmp is often a small tmpfs.
mkdir -p "$OUT"
STAGE="$(mktemp -d "$OUT/.stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/$NAME"
cp "$BIN" "$STAGE/$NAME/vktr"
if [[ "$TARGET" == *-apple-darwin ]]; then
  codesign --verify "$STAGE/$NAME/vktr" 2>/dev/null || codesign --force --sign - "$STAGE/$NAME/vktr"
  codesign --verify --verbose "$STAGE/$NAME/vktr"
fi
cp LICENSE NOTICE THIRD-PARTY-NOTICES README.md CHANGELOG.md "$STAGE/$NAME/"
if [ "$TARGET" = "$HOST" ] || [[ "$HOST" == x86_64-* && "$TARGET" == x86_64-* ]]; then
  "$STAGE/$NAME/vktr" --version >"$STAGE/$NAME/VERSION"
else
  echo "vktr $VERSION ($COMMIT)" >"$STAGE/$NAME/VERSION"
fi

# COPYFILE_DISABLE keeps macOS tar from adding ._ AppleDouble entries.
COPYFILE_DISABLE=1 tar -C "$STAGE" -czf "$OUT/$NAME.tar.gz" "$NAME"
if command -v sha256sum >/dev/null 2>&1; then sum=(sha256sum); else sum=(shasum -a 256); fi
(cd "$OUT" && "${sum[@]}" "$NAME.tar.gz" >"$NAME.tar.gz.sha256")
ls -la "$OUT/$NAME.tar.gz"
cat "$OUT/$NAME.tar.gz.sha256"
