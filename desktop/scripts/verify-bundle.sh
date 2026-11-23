#!/usr/bin/env bash
# Structural checks for the bundles produced by `tauri build`.
#
# Set REQUIRE_UPDATER_ARTIFACTS=1 for release builds so a missing updater
# package/signature fails the workflow before artifacts are published.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${DESKTOP_DIR}"

TARGET_TRIPLE="${SENESCHAL_BUNDLE_TARGET:-${TARGET_TRIPLE:-$(rustc -vV | awk '/host:/ {print $2}')}}"
# `tauri build` runs from src-tauri; without --target it emits under
# src-tauri/target/release, and with --target under src-tauri/target/<triple>.
if [[ -n "${SENESCHAL_BUNDLE_TARGET:-}" ]]; then
    BUNDLE_ROOT="src-tauri/target/${TARGET_TRIPLE}/release/bundle"
else
    BUNDLE_ROOT="src-tauri/target/release/bundle"
fi
REQUIRE_UPDATER_ARTIFACTS="${REQUIRE_UPDATER_ARTIFACTS:-0}"

fail() {
    echo "bundle verification failed: $*" >&2
    exit 1
}

require_file() {
    local pattern="$1" directory="$2" found
    found="$(find "${directory}" -maxdepth 2 -type f -name "${pattern}" -print -quit 2>/dev/null || true)"
    [[ -n "${found}" ]] || fail "no ${pattern} under ${directory}"
    echo "verified ${found}"
}

verify_linux() {
    local appimage deb sidecar
    appimage="$(find "${BUNDLE_ROOT}/appimage" -maxdepth 1 -type f -name '*.AppImage' -print -quit 2>/dev/null || true)"
    deb="$(find "${BUNDLE_ROOT}/deb" -maxdepth 1 -type f -name '*.deb' -print -quit 2>/dev/null || true)"
    [[ -n "${appimage}" ]] || fail "no AppImage under ${BUNDLE_ROOT}/appimage"
    [[ -n "${deb}" ]] || fail "no .deb under ${BUNDLE_ROOT}/deb"

    file "${appimage}" | grep -q 'ELF' || fail "AppImage is not an ELF image: ${appimage}"
    appimage="$(realpath "${appimage}")"

    EXTRACT_DIR="$(mktemp -d)"
    trap 'rm -rf "${EXTRACT_DIR}"' EXIT
    if ! (cd "${EXTRACT_DIR}" && "${appimage}" --appimage-extract >/dev/null 2>&1); then
        fail "could not extract AppImage: ${appimage}"
    fi
    find "${EXTRACT_DIR}" -type d -name 'espeak-ng-data' -print -quit | grep -q . ||
        fail "AppImage does not contain espeak-ng-data: ${appimage}"

    sidecar="$(find "${EXTRACT_DIR}" -type f -name 'seneschal' -print -quit 2>/dev/null || true)"
    [[ -x "${sidecar}" ]] || fail "sidecar is missing from the AppImage: ${appimage}"
    if command -v ldd >/dev/null 2>&1; then
        if ldd "${sidecar}" 2>/dev/null | grep -q 'not found'; then
            ldd "${sidecar}" 2>/dev/null | grep 'not found' >&2 || true
            fail "sidecar has unresolved dynamic libraries"
        fi
    fi
    rm -rf "${EXTRACT_DIR}"
    trap - EXIT

    dpkg-deb -I "${deb}" >/dev/null || fail "invalid Debian package: ${deb}"
    dpkg-deb -c "${deb}" | grep -q 'seneschal' || fail "Debian package does not contain the sidecar: ${deb}"
    dpkg-deb -c "${deb}" | grep -q 'espeak-ng-data' || fail "Debian package does not contain espeak-ng-data: ${deb}"
    echo "verified ${appimage}"
    echo "verified ${deb}"

    if [[ "${REQUIRE_UPDATER_ARTIFACTS}" = "1" ]]; then
        require_file '*.AppImage.sig' "${BUNDLE_ROOT}/appimage"
    fi
}

verify_macos() {
    local app dmg sidecar
    app="$(find "${BUNDLE_ROOT}/macos" -maxdepth 1 -type d -name '*.app' -print -quit 2>/dev/null || true)"
    dmg="$(find "${BUNDLE_ROOT}/dmg" -maxdepth 1 -type f -name '*.dmg' -print -quit 2>/dev/null || true)"
    [[ -n "${app}" ]] || fail "no .app under ${BUNDLE_ROOT}/macos"
    [[ -n "${dmg}" ]] || fail "no .dmg under ${BUNDLE_ROOT}/dmg"

    /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
        "${app}/Contents/Info.plist" | grep -qx 'org.veladan.seneschal' ||
        fail "unexpected bundle identifier in ${app}"

    sidecar="$(find "${app}/Contents/MacOS" -maxdepth 1 -type f -name 'seneschal-*' -print -quit 2>/dev/null || true)"
    [[ -n "${sidecar}" ]] || fail "sidecar is missing from ${app}/Contents/MacOS"

    if command -v codesign >/dev/null 2>&1; then
        codesign --verify --deep --strict "${app}" ||
            fail "invalid or missing code signature for ${app}"
    fi
    echo "verified ${app}"
    echo "verified ${dmg}"

    if [[ "${REQUIRE_UPDATER_ARTIFACTS}" = "1" ]]; then
        require_file '*.app.tar.gz.sig' "${BUNDLE_ROOT}/macos"
    fi
}

case "$(uname -s)" in
    Darwin) verify_macos ;;
    Linux) verify_linux ;;
    *) fail "unsupported bundle host: $(uname -s)" ;;
esac

echo "Bundle verification passed for ${TARGET_TRIPLE}."
