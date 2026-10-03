#!/bin/sh
# Checksum-verified, atomic installer for Apple Silicon Macs.

set -eu

PROGRAM="jbsync"
DISPLAY_NAME="jbsync"
DEFAULT_REPOSITORY="cdenihan/jbsync"
ENV_PREFIX="JBSYNC"

usage() {
    cat <<EOF
Install $DISPLAY_NAME on Apple Silicon Macs.

Usage:
  install.sh [--version VERSION] [--install-dir DIRECTORY]

Options:
  --version VERSION        Release tag (default: latest)
  --install-dir DIRECTORY  Destination directory (default: \$HOME/.local/bin)
  -h, --help               Show this help

Environment:
  ${ENV_PREFIX}_VERSION             Alternative to --version
  ${ENV_PREFIX}_INSTALL_DIR         Alternative to --install-dir
  ${ENV_PREFIX}_REPOSITORY          GitHub owner/repository (default: $DEFAULT_REPOSITORY)
  ${ENV_PREFIX}_RELEASE_BASE_URL    Release base URL for mirrors or testing
EOF
}

log() {
    printf '%s\n' "$PROGRAM-install: $*"
}

die() {
    printf '%s\n' "$PROGRAM-install: error: $*" >&2
    exit 1
}

environment_value() {
    suffix=$1
    eval "printf '%s' \"\${${ENV_PREFIX}_${suffix}:-}\""
}

normalize_version() {
    case "$1" in
        latest) printf '%s\n' "latest" ;;
        v*) printf '%s\n' "$1" ;;
        *) printf 'v%s\n' "$1" ;;
    esac
}

normalize_arch() {
    case "$1" in
        x86_64 | amd64) printf '%s\n' "x86_64" ;;
        aarch64 | arm64) printf '%s\n' "aarch64" ;;
        *) return 1 ;;
    esac
}

artifact_for() {
    os=$1
    arch=$2
    case "$os:$arch" in
        Darwin:aarch64) printf '%s\n' "$PROGRAM-macos-aarch64" ;;
        *) return 1 ;;
    esac
}

download_file() {
    url=$1
    destination=$2
    case "$url" in
        file://*) cp "${url#file://}" "$destination"; return ;;
        https://*) ;;
        *) die "refusing non-HTTPS download URL: $url" ;;
    esac
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error --retry 3 \
            --connect-timeout 15 --max-time 180 --max-filesize 33554432 \
            --proto '=https' --proto-redir '=https' --tlsv1.2 \
            --output "$destination" "$url"
        return
    fi
    if command -v wget >/dev/null 2>&1; then
        wget -q -O "$destination" "$url"
        return
    fi
    die "curl or wget is required"
}

sha256_file() {
    path=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$path" | awk '{print $NF}'
    else
        die "sha256sum, shasum, or openssl is required for checksum verification"
    fi
}

verify_checksum() {
    artifact=$1
    checksum_file=$2
    expected=$(awk 'NR == 1 {print $1}' "$checksum_file")
    printf '%s\n' "$expected" | grep -Eq '^[0-9A-Fa-f]{64}$' ||
        die "release checksum file is malformed"
    actual=$(sha256_file "$artifact")
    [ "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" = \
        "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ] ||
        die "SHA-256 checksum verification failed"
}

verify_binary_version() {
    binary=$1
    requested_version=$2
    reported=$("$binary" --version 2>/dev/null) ||
        die "downloaded binary could not run on this machine"
    case "$reported" in
        "$PROGRAM "*) ;;
        *) die "downloaded file did not identify itself as $DISPLAY_NAME" ;;
    esac
    if [ "$requested_version" != "latest" ] &&
        [ "$reported" != "$PROGRAM ${requested_version#v}" ]; then
        die "downloaded binary version does not match requested release $requested_version"
    fi
}

main() {
    version=$(environment_value VERSION)
    install_dir=$(environment_value INSTALL_DIR)
    version=${version:-latest}
    install_dir=${install_dir:-"${HOME:?HOME is not set}/.local/bin"}

    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version) [ "$#" -ge 2 ] || die "--version requires a value"; version=$2; shift 2 ;;
            --install-dir) [ "$#" -ge 2 ] || die "--install-dir requires a value"; install_dir=$2; shift 2 ;;
            -h | --help) usage; return ;;
            *) die "unknown option: $1" ;;
        esac
    done

    version=$(normalize_version "$version")
    if [ "$version" != latest ]; then
        printf '%s\n' "$version" | grep -Eq '^v[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[1-9][0-9]*$' || die "invalid release version"
    fi
    repository=$(environment_value REPOSITORY)
    repository=${repository:-$DEFAULT_REPOSITORY}
    release_base=$(environment_value RELEASE_BASE_URL)
    release_base=${release_base:-"https://github.com/$repository/releases"}
    os=$(uname -s)
    arch=$(normalize_arch "$(uname -m)") || die "unsupported CPU architecture: $(uname -m)"
    artifact=$(artifact_for "$os" "$arch") ||
        die "unsupported platform: $os/$arch"

    os_version=$(/usr/bin/sw_vers -productVersion)
    [ "${os_version%%.*}" -ge 15 ] || die "macOS 15 or newer is required"

    if [ "$version" = latest ]; then
        download_base="$release_base/latest/download"
    else
        download_base="$release_base/download/$version"
    fi
    temporary=$(mktemp -d "${TMPDIR:-/tmp}/$PROGRAM-install.XXXXXX") ||
        die "could not create a temporary directory"
    staging=""
    trap 'rm -rf "$temporary"; if [ -n "$staging" ]; then rm -f "$staging"; fi' EXIT HUP INT TERM

    binary_path="$temporary/$artifact"
    checksum_path="$temporary/$artifact.sha256"
    log "downloading $artifact ($version)"
    download_file "$download_base/$artifact" "$binary_path"
    download_file "$download_base/$artifact.sha256" "$checksum_path"
    verify_checksum "$binary_path" "$checksum_path"
    [ "$(/usr/bin/lipo -archs "$binary_path" 2>/dev/null)" = arm64 ] || die "release binary must contain only Apple Silicon code"
    chmod 0755 "$binary_path"
    verify_binary_version "$binary_path" "$version"

    if [ -f "$install_dir/$PROGRAM" ] && cmp -s "$binary_path" "$install_dir/$PROGRAM"; then
        log "already current at $install_dir/$PROGRAM"
        return
    fi
    mkdir -p "$install_dir" || die "could not create install directory: $install_dir"
    [ ! -d "$install_dir/$PROGRAM" ] || die "destination executable is a directory"
    staging=$(mktemp "$install_dir/.$PROGRAM-install.XXXXXX") || die "could not create atomic staging file"
    cp "$binary_path" "$staging" || die "could not write to install directory: $install_dir"
    chmod 0755 "$staging"
    mv -f "$staging" "$install_dir/$PROGRAM"
    staging=""
    log "installed $("$install_dir/$PROGRAM" --version) to $install_dir/$PROGRAM"
    case ":${PATH:-}:" in
        *":$install_dir:"*) ;;
        *) log "add $install_dir to PATH to run $PROGRAM from any directory" ;;
    esac
}

source_only=$(environment_value INSTALLER_SOURCE_ONLY)
source_only=${source_only:-0}
if [ "$source_only" != "1" ]; then
    main "$@"
fi
