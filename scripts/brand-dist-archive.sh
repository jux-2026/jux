#!/bin/sh
set -eu

target="$1"
version="$2"
source_commit="$3"
archive="target/distrib/jux-${target}.tar.xz"
checksum="${archive}.sha256"

if [ ! -f "$archive" ]; then
    echo "release archive not found: $archive" >&2
    exit 1
fi

work_dir=$(mktemp -d)
cleanup() {
    rm -rf "$work_dir"
}
trap cleanup EXIT

tar -xJf "$archive" -C "$work_dir"
binary=$(find "$work_dir" -type f -name jux -print -quit)
if [ -z "$binary" ]; then
    echo "jux binary not found in $archive" >&2
    exit 1
fi

branded="${binary}.branded"
"$binary" distribution inject \
    --input "$binary" \
    --output-path "$branded" \
    --channel github-release \
    --installer bash \
    --version "$version" \
    --source-commit "$source_commit"
mv "$branded" "$binary"
if [ "$(uname -s)" = "Darwin" ]; then
    # Patching a Mach-O invalidates the linker's ad-hoc signature. Re-sign before executing the
    # branded binary; a future Developer ID signing step must run at this same pipeline boundary.
    codesign --force --sign - "$binary"
fi
metadata=$("$binary" distribution show)
case "$metadata" in
    *"channel: GithubRelease"*"installer: Bash"*) ;;
    *)
        echo "injected distribution metadata verification failed" >&2
        exit 1
        ;;
esac

root=$(find "$work_dir" -mindepth 1 -maxdepth 1 -type d -print -quit)
temporary_archive="${archive}.tmp"
tar -cJf "$temporary_archive" -C "$work_dir" "$(basename "$root")"
mv "$temporary_archive" "$archive"

name=$(basename "$archive")
if command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum "$archive" | awk '{print $1}')
else
    digest=$(shasum -a 256 "$archive" | awk '{print $1}')
fi
printf '%s *%s\n' "$digest" "$name" > "$checksum"
