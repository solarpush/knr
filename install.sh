#!/usr/bin/env sh
# =============================================================================
# knr Installer — Installs the prebuilt 'knr' binary from GitHub Releases
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/solarpush/knr/main/install.sh | sh
#
# Environment Variables:
#   KNR_VERSION       Specific release tag to install (e.g. "v0.1.0", default: latest)
#   KNR_INSTALL_DIR   Destination directory (default: "$HOME/.local/bin")
#   KNR_REPO          GitHub repository (default: "solarpush/knr")
# =============================================================================

set -eu

REPO="${KNR_REPO:-solarpush/knr}"
INSTALL_DIR="${KNR_INSTALL_DIR:-$HOME/.local/bin}"
REQUESTED_VERSION="${KNR_VERSION:-}"

info() {
    printf "\033[1;34m==>\033[0m %s\n" "$*"
}

success() {
    printf "\033[1;32m==>\033[0m %s\n" "$*"
}

error() {
    printf "\033[1;31merror:\033[0m %s\n" "$*" >&2
    exit 1
}

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        error "Required command '$1' not found. Please install it first."
    fi
}

need_cmd uname
need_cmd tar
need_cmd mktemp

detect_target() {
    os_name="$(uname -s)"
    arch_name="$(uname -m)"

    case "$os_name" in
        Linux)
            os_triple="unknown-linux-gnu"
            ;;
        Darwin)
            os_triple="apple-darwin"
            ;;
        *)
            error "Unsupported operating system: $os_name"
            ;;
    esac

    case "$arch_name" in
        x86_64 | amd64)
            if [ "$os_name" = "Darwin" ]; then
                error "Intel macOS (x86_64) is no longer supported; only Apple Silicon (arm64) is supported on macOS."
            fi
            arch_triple="x86_64"
            ;;
        aarch64 | arm64)
            arch_triple="aarch64"
            ;;
        *)
            error "Unsupported CPU architecture: $arch_name"
            ;;
    esac

    printf "%s-%s" "$arch_triple" "$os_triple"
}

fetch_text() {
    url="$1"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -H "Accept: application/vnd.github+json" -H "User-Agent: knr-installer" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- --header="Accept: application/vnd.github+json" --header="User-Agent: knr-installer" "$url"
    else
        error "Neither 'curl' nor 'wget' was found."
    fi
}

download_file() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -H "User-Agent: knr-installer" -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --header="User-Agent: knr-installer" -O "$dest" "$url"
    else
        error "Neither 'curl' nor 'wget' was found."
    fi
}

resolve_version() {
    if [ -n "$REQUESTED_VERSION" ]; then
        case "$REQUESTED_VERSION" in
            v*) printf "%s" "$REQUESTED_VERSION" ;;
            *)  printf "v%s" "$REQUESTED_VERSION" ;;
        esac
        return
    fi

    api_url="${KNR_RELEASE_API_URL:-https://api.github.com/repos/${REPO}/releases/latest}"
    json="$(fetch_text "$api_url")" || error "Failed to query latest release from $api_url"
    tag="$(printf "%s" "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
    if [ -z "$tag" ]; then
        error "Could not determine latest release tag from $api_url"
    fi
    printf "%s" "$tag"
}

verify_checksum() {
    archive_file="$1"
    checksum_file="$2"

    expected="$(awk '{print $1}' "$checksum_file" | head -n 1)"
    if [ -z "$expected" ]; then
        return 0
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$archive_file" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$archive_file" | awk '{print $1}')"
    else
        return 0
    fi

    if [ "$expected" != "$actual" ]; then
        error "SHA256 checksum mismatch! Expected $expected, got $actual"
    fi
    info "SHA256 checksum verified ($actual)"
}

main() {
    TARGET="$(detect_target)"
    TAG="$(resolve_version)"
    ASSET="knr-${TARGET}.tar.gz"
    BASE_URL="${KNR_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download/${TAG}}"
    DOWNLOAD_URL="${BASE_URL}/${ASSET}"
    CHECKSUM_URL="${DOWNLOAD_URL}.sha256"

    info "Detected target: ${TARGET}"
    info "Installing knr ${TAG} from ${REPO}..."

    TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t knr_install)"
    trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

    ARCHIVE_PATH="${TMP_DIR}/${ASSET}"
    CHECKSUM_PATH="${ARCHIVE_PATH}.sha256"

    info "Downloading ${DOWNLOAD_URL}..."
    download_file "$DOWNLOAD_URL" "$ARCHIVE_PATH" || error "Failed to download ${DOWNLOAD_URL}"

    if download_file "$CHECKSUM_URL" "$CHECKSUM_PATH" 2>/dev/null; then
        verify_checksum "$ARCHIVE_PATH" "$CHECKSUM_PATH"
    fi

    EXTRACT_DIR="${TMP_DIR}/extract"
    mkdir -p "$EXTRACT_DIR"
    tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"

    if [ -f "${EXTRACT_DIR}/knr" ]; then
        BIN_SRC="${EXTRACT_DIR}/knr"
    else
        BIN_SRC="$(find "$EXTRACT_DIR" -type f -name knr | head -n 1)"
    fi

    if [ -z "${BIN_SRC:-}" ] || [ ! -f "$BIN_SRC" ]; then
        error "Archive ${ASSET} did not contain a 'knr' binary."
    fi

    mkdir -p "$INSTALL_DIR"
    DEST_BIN="${INSTALL_DIR}/knr"
    cp "$BIN_SRC" "${DEST_BIN}.tmp.$$"
    chmod 755 "${DEST_BIN}.tmp.$$"
    mv -f "${DEST_BIN}.tmp.$$" "$DEST_BIN"

    INSTALLED_VER="$("$DEST_BIN" --version 2>/dev/null || echo "knr ${TAG}")"
    success "Installed ${INSTALLED_VER} to ${DEST_BIN}"

    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            ;;
        *)
            printf "\n\033[1;33mnote:\033[0m %s is not in your PATH. Add it with:\n" "$INSTALL_DIR"
            printf "  export PATH=\"%s:\$PATH\"\n" "$INSTALL_DIR"
            ;;
    esac

    printf "\nRun '\033[1mknr --help\033[0m' to get started, or '\033[1mknr update\033[0m' to update later.\n"
}

main "$@"
