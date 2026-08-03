#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
policy="$repo/tools/ghidra/n64loaderwv-source-policy.json"
artifact_policy="$repo/tools/ghidra/n64loaderwv-artifact-policy.json"
provenance_verifier="$repo/tools/ghidra/verify-n64loaderwv-provenance.py"
launcher_verifier="$repo/tools/ghidra/verify-ghidra-launcher.py"
install_verifier="$repo/tools/ghidra/verify-n64loaderwv-install.py"

fail() {
    echo "n64loaderwv gui: $1" >&2
    exit 2
}

usage() {
    echo "usage: tools/ghidra/run-n64loaderwv-gui.sh PROFILE_ROOT EXTENSION_ZIP CONFORMANCE_RECEIPT [GHIDRA_ARGUMENT ...]" >&2
    echo "requires: GHIDRA_INSTALL_DIR and GHIDRA_JAVA_HOME" >&2
    exit 2
}

hash_file() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -- "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- "$1" | awk '{print $1}'
    else
        fail "no SHA-256 utility is available"
    fi
}

require_absolute() {
    case "$2" in
        /*) ;;
        *) fail "$1 must be absolute" ;;
    esac
}

[ "$#" -ge 3 ] || usage
profile_root=$1
extension_zip=$2
build_receipt=$3
shift 3
prepare_only=false
if [ "${1:-}" = --prepare-only ]; then
    prepare_only=true
    shift
fi

require_absolute PROFILE_ROOT "$profile_root"
require_absolute EXTENSION_ZIP "$extension_zip"
require_absolute CONFORMANCE_RECEIPT "$build_receipt"
[ -d "$profile_root" ] || fail "PROFILE_ROOT must already exist"
[ ! -L "$profile_root" ] || fail "PROFILE_ROOT must not be a symlink"
[ -f "$extension_zip" ] && [ ! -L "$extension_zip" ] ||
    fail "EXTENSION_ZIP must be a regular non-symlink file"
[ -f "$build_receipt" ] && [ ! -L "$build_receipt" ] ||
    fail "CONFORMANCE_RECEIPT must be a regular non-symlink file"
[ -x "$install_verifier" ] || fail "N64LoaderWV install verifier is not executable"

profile_root=$(CDPATH='' cd -- "$profile_root" && pwd -P) ||
    fail "could not resolve PROFILE_ROOT"
case "$profile_root" in
    "$repo"|"$repo"/*) fail "PROFILE_ROOT must be outside the repository" ;;
esac
if profile_mode=$(stat -c '%a' "$profile_root" 2>/dev/null); then :
elif profile_mode=$(stat -f '%Lp' "$profile_root" 2>/dev/null); then :
else fail "could not inspect PROFILE_ROOT permissions"
fi
[ "$profile_mode" = 700 ] || fail "PROFILE_ROOT must have mode 0700"

verified_artifact=$(
    "$provenance_verifier" artifact "$artifact_policy" "$policy" \
        "$build_receipt" "$extension_zip"
) || fail "extension artifact does not satisfy the fn64 policy"
extension_sha=$(printf '%s\n' "$verified_artifact" | python3 -c \
    'import json,sys; print(json.load(sys.stdin)["extension_sha256"])')

ghidra_install=${GHIDRA_INSTALL_DIR:-}
jdk=${GHIDRA_JAVA_HOME:-}
[ -n "$ghidra_install" ] || fail "GHIDRA_INSTALL_DIR is required"
[ -n "$jdk" ] || fail "GHIDRA_JAVA_HOME is required"
require_absolute GHIDRA_INSTALL_DIR "$ghidra_install"
require_absolute GHIDRA_JAVA_HOME "$jdk"
[ -x "$jdk/bin/java" ] || fail "GHIDRA_JAVA_HOME must contain bin/java"
gui="$ghidra_install/ghidraRun"
[ -x "$gui" ] || fail "Ghidra GUI launcher is not executable"
"$launcher_verifier" "$ghidra_install" "$gui" ghidraRun ||
    fail "Ghidra GUI launcher does not belong to GHIDRA_INSTALL_DIR"
ghidra_version=$(awk -F= '$1 == "application.version" { print $2 }' \
    "$ghidra_install/Ghidra/application.properties")
ghidra_release=$(awk -F= '$1 == "application.release.name" { print $2 }' \
    "$ghidra_install/Ghidra/application.properties")
[ -n "$ghidra_version" ] && [ -n "$ghidra_release" ] ||
    fail "could not read the Ghidra version and release"

profile="$profile_root/n64loaderwv-$extension_sha"
home="$profile/home"
settings_user="$home/ghidra/ghidra_${ghidra_version}_${ghidra_release}"
extensions="$settings_user/Extensions"
installed_zip="$profile/n64loaderwv-extension.zip"
mkdir -p "$extensions" "$profile/cache" "$profile/tmp"

if [ ! -f "$installed_zip" ]; then
    cp -- "$extension_zip" "$installed_zip"
fi
[ "$(hash_file "$installed_zip")" = "$extension_sha" ] ||
    fail "installed profile artifact does not match the approved fork"

archive_entries="$profile/archive-entries.txt"
unzip -Z1 "$installed_zip" > "$archive_entries"
if grep -Eq '(^/|(^|/)\.\.(/|$))' "$archive_entries"; then
    fail "extension archive contains an unsafe path"
fi
extension_roots=$(awk -F/ 'NF { print $1 }' "$archive_entries" | sort -u)
[ "$(printf '%s\n' "$extension_roots" | awk 'NF { count++ } END { print count + 0 }')" -eq 1 ] ||
    fail "extension archive must have exactly one top-level directory"
extension_root=$(printf '%s\n' "$extension_roots" | awk 'NF { print; exit }')
case "$extension_root" in
    ''|.|..|*[!A-Za-z0-9._-]*) fail "extension archive has an invalid root" ;;
esac
extension_dir="$extensions/$extension_root"
if [ ! -e "$extension_dir" ]; then
    unzip -q "$installed_zip" -d "$extensions"
fi

install_receipt="$profile/install-verification.json"
"$install_verifier" "$installed_zip" "$extension_dir" \
    "$ghidra_install" "$settings_user" > "$install_receipt" ||
    fail "installed profile does not exactly match the approved fn64 fork"

path_value=${PATH:-/usr/bin:/bin}
if [ "$prepare_only" = true ]; then
    echo "n64loaderwv gui: prepared approved fn64 fork $extension_sha"
    echo "profile=$profile"
    exit 0
fi
echo "n64loaderwv gui: launching approved fn64 fork $extension_sha"
echo "profile=$profile"
exec env -i "PATH=$path_value" "HOME=$home" "TMPDIR=$profile/tmp" \
    "JAVA_HOME=$jdk" "GHIDRA_GUI_MAXMEM=1G" \
    "GHIDRA_GUI_JAVA_OPTIONS=-Dapplication.settingsdir=$home -Dapplication.cachedir=$profile/cache -Dapplication.tempdir=$profile/tmp -Djava.io.tmpdir=$profile/tmp -Duser.home=$home" \
    "$gui" "$@"
