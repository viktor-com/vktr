#!/bin/sh
# vktr installer.
#
#   curl -fsSL <raw>/install.sh | sh                        # install the latest GitHub release
#   sh install.sh --version v1.0.38-vktr.2                  # a specific release tag
#   sh install.sh --tarball dist/vktr-<version>-<target>.tar.gz
#   sh install.sh --url https://example.com/vktr-<version>-<target>.tar.gz
#   sh install.sh --from-source                             # cargo build in this checkout
#
# Installs the binary to $VKTR_INSTALL_DIR (default ~/.vktr/bin) and links it into
# $VKTR_BIN_DIR (default ~/.local/bin). Release downloads are verified against the published
# .sha256 file. Nothing is installed system-wide and nothing needs root.
#
# Releases come from the GitHub repo $VKTR_RELEASE_REPO (default viktor-com/vktr) and are fetched
# with plain curl; if the repo is not publicly readable the GitHub CLI (`gh auth login`) is used.
# "latest" is the newest non-prerelease. Set $VKTR_RELEASE_BASE_URL to use a plain file server.
set -eu

BASE_URL="${VKTR_RELEASE_BASE_URL:-}"
REPO="${VKTR_RELEASE_REPO:-viktor-com/vktr}"
INSTALL_DIR="${VKTR_INSTALL_DIR:-$HOME/.vktr/bin}"
BIN_DIR="${VKTR_BIN_DIR:-$HOME/.local/bin}"
MODE=release; SOURCE=""; VERSION="${VKTR_VERSION:-latest}"

say()  { printf '%s\n' "$*" >&2; }
die()  { say "vktr install: $*"; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "\`$1\` is required"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --tarball)     MODE=tarball; SOURCE="${2:-}"; shift 2 ;;
    --url)         MODE=url; SOURCE="${2:-}"; shift 2 ;;
    --from-source) MODE=source; shift ;;
    --version)     VERSION="${2:-}"; shift 2 ;;
    --base-url)    BASE_URL="${2:-}"; shift 2 ;;
    --repo)        REPO="${2:-}"; shift 2 ;;
    -h|--help)     sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

detect_target() {
  os="$(uname -s)"; arch="$(uname -m)"
  case "$arch" in x86_64|amd64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; *) die "unsupported CPU: $arch" ;; esac
  case "$os" in
    Linux)  echo "$arch-unknown-linux-gnu" ;;
    Darwin) echo "$arch-apple-darwin" ;;
    *) die "unsupported OS: $os (build from source with --from-source)" ;;
  esac
}

fetch() { # fetch <url> <dest>
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget -q "$1" -O "$2"
  else die "curl or wget is required"; fi
}

# The newest non-prerelease tag. Tried unauthenticated first so a public repo needs no GitHub
# CLI; a private repo answers 404 and the CLI takes over.
latest_tag() {
  curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1
}

# The asset file name for this platform, from the public API. Empty when the repo is private.
public_asset() { # public_asset <tag> <target>
  curl -fsSL "https://api.github.com/repos/$REPO/releases/tags/$1" 2>/dev/null \
    | sed -n 's/.*"name": *"\(vktr-[^"]*'"$2"'\.tar\.gz\)".*/\1/p' | head -1
}

# Download the release tarball and its checksum into $WORK, by whichever route works.
github_release() { # github_release <target>
  target="$1"
  tag="$VERSION"
  if [ "$tag" = latest ]; then
    tag="$(latest_tag)"
    if [ -z "$tag" ] && command -v gh >/dev/null 2>&1; then
      tag="$(gh release list --repo "$REPO" --exclude-pre-releases --limit 1 --json tagName --jq '.[0].tagName' 2>/dev/null)"
    fi
    if [ -z "$tag" ]; then
      # A public repo answers this; a private one (or no network) does not.
      if curl -fsS -o /dev/null "https://api.github.com/repos/$REPO" 2>/dev/null; then
        die "$REPO has no published release yet. Build from a checkout with \`sh install.sh --from-source\`."
      fi
      die "cannot list releases of $REPO. If it is private, run \`gh auth login\` first."
    fi
  fi
  asset="$(public_asset "$tag" "$target")"
  if [ -n "$asset" ]; then
    say "Downloading $asset from $REPO $tag..."
    url="https://github.com/$REPO/releases/download/$tag/$asset"
    fetch "$url" "$WORK/vktr.tar.gz" || die "cannot download $url"
    fetch "$url.sha256" "$WORK/vktr.tar.gz.sha256" || die "release is missing its .sha256"
    return
  fi
  # A public repo whose release has no asset for this platform: gh cannot help.
  if curl -fsSL "https://api.github.com/repos/$REPO/releases/tags/$tag" >/dev/null 2>&1; then
    die "$REPO $tag has no prebuilt vktr for $target. Build it from a checkout with \`sh install.sh --from-source\`."
  fi
  # Private repo: authenticate with the GitHub CLI.
  command -v gh >/dev/null 2>&1 \
    || die "$REPO is not publicly readable. Install the GitHub CLI and run \`gh auth login\`, or use --url / --tarball / --from-source."
  say "Downloading vktr $tag for $target from $REPO with gh..."
  gh release download "$tag" --repo "$REPO" --dir "$WORK" --clobber \
     --pattern "vktr-*-$target.tar.gz" --pattern "vktr-*-$target.tar.gz.sha256" \
    || die "no release asset for $target in $REPO $tag (\`gh auth status\` to check your login)"
  found="$(find "$WORK" -name "vktr-*-$target.tar.gz" | head -1)"
  [ -n "$found" ] || die "no release asset for $target in $REPO $tag"
  [ -f "$found.sha256" ] || die "release is missing its .sha256"
  mv "$found.sha256" "$WORK/vktr.tar.gz.sha256"
  mv "$found" "$WORK/vktr.tar.gz"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# Work next to the install dir, not in /tmp: the download is large and /tmp is often a small tmpfs.
mkdir -p "$INSTALL_DIR"
WORK="$(mktemp -d "$INSTALL_DIR/.install.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

case "$MODE" in
  source)
    need cargo
    [ -f Cargo.toml ] && [ -d crates/codegen/xai-grok-pager-bin ] || die "--from-source must run in a vktr checkout"
    command -v dotslash >/dev/null 2>&1 || die "dotslash is required to build (cargo install dotslash)"
    say "Building vktr (this takes a while the first time)..."
    cargo build -p xai-grok-pager-bin --profile vktr-dist
    NEW_BIN="target/vktr-dist/vktr"
    ;;
  tarball)
    [ -f "$SOURCE" ] || die "no such tarball: $SOURCE"
    cp "$SOURCE" "$WORK/vktr.tar.gz"
    [ -f "$SOURCE.sha256" ] && cp "$SOURCE.sha256" "$WORK/vktr.tar.gz.sha256"
    ;;
  url)
    [ -n "$SOURCE" ] || die "--url needs a value"
    fetch "$SOURCE" "$WORK/vktr.tar.gz"
    fetch "$SOURCE.sha256" "$WORK/vktr.tar.gz.sha256" 2>/dev/null || true
    ;;
  release)
    TARGET="$(detect_target)"
    if [ -n "$BASE_URL" ]; then
      # A plain file server laid out as <base>/LATEST and <base>/vktr-<version>-<target>.tar.gz.
      if [ "$VERSION" = latest ]; then
        fetch "$BASE_URL/LATEST" "$WORK/LATEST" || die "cannot read $BASE_URL/LATEST"
        VERSION="$(tr -d ' \n\r' <"$WORK/LATEST")"
      fi
      NAME="vktr-$VERSION-$TARGET.tar.gz"
      say "Downloading $NAME..."
      fetch "$BASE_URL/$NAME" "$WORK/vktr.tar.gz" || die "no release for $TARGET at $BASE_URL/$NAME"
      fetch "$BASE_URL/$NAME.sha256" "$WORK/vktr.tar.gz.sha256" || die "release is missing its .sha256"
    else
      github_release "$TARGET"
    fi
    ;;
esac

if [ "$MODE" != source ]; then
  if [ -f "$WORK/vktr.tar.gz.sha256" ]; then
    want="$(awk '{print $1}' "$WORK/vktr.tar.gz.sha256")"
    got="$(sha256_of "$WORK/vktr.tar.gz")"
    [ "$want" = "$got" ] || die "checksum mismatch (expected $want, got $got)"
    say "Checksum verified."
  elif [ "$MODE" = release ]; then
    die "refusing to install an unverified release"
  else
    say "Warning: no .sha256 next to the tarball; skipping verification."
  fi
  tar -xzf "$WORK/vktr.tar.gz" -C "$WORK"
  NEW_BIN="$(find "$WORK" -type f -name vktr -perm -u+x | head -1)"
  [ -n "$NEW_BIN" ] || die "the tarball does not contain a vktr binary"
fi

VER="$("$NEW_BIN" --version 2>/dev/null | awk '{print $2}')"
[ -n "$VER" ] || die "the vktr binary does not run on this machine"
mkdir -p "$INSTALL_DIR" "$BIN_DIR"
DEST="$INSTALL_DIR/vktr-$VER"
cp "$NEW_BIN" "$DEST.tmp" && chmod 755 "$DEST.tmp" && mv "$DEST.tmp" "$DEST"
ln -sfn "$DEST" "$INSTALL_DIR/vktr"
ln -sfn "$INSTALL_DIR/vktr" "$BIN_DIR/vktr"
# Keep the license files next to the binary: Apache-2.0 requires them to travel with it.
if [ "$MODE" = source ]; then SRC_DIR="."; else SRC_DIR="$(dirname "$NEW_BIN")"; fi
for f in LICENSE NOTICE THIRD-PARTY-NOTICES; do
  [ -f "$SRC_DIR/$f" ] && cp "$SRC_DIR/$f" "$INSTALL_DIR/$f"
done

say "Installed vktr $VER -> $BIN_DIR/vktr"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "Add $BIN_DIR to your PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;; esac
say "Next: run \`vktr login\` and paste your Viktor API key (or export VIKTOR_API_KEY), then run: vktr"
