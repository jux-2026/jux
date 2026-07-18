#!/bin/sh
set -eu

target="$1"
version="$2"
source_commit="$3"
archive="target/distrib/jux-${target}.tar.xz"

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
root=$(find "$work_dir" -mindepth 1 -maxdepth 1 -type d -print -quit)
if [ -z "$binary" ] || [ -z "$root" ]; then
    echo "jux archive layout is invalid: $archive" >&2
    exit 1
fi

# Keep one untouched build outside the archive root while injecting release metadata.
base_binary="$work_dir/jux.base"
cp "$binary" "$base_binary"
chmod +x "$base_binary"

write_checksum() {
    archive="$1"
    name=$(basename "$archive")
    if command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$archive" | awk '{print $1}')
    else
        digest=$(shasum -a 256 "$archive" | awk '{print $1}')
    fi
    printf '%s *%s\n' "$digest" "$name" > "${archive}.sha256"
}

brand_archive() {
    archive="$1"
    channel="$2"
    installer="$3"
    expected_channel="$4"
    expected_installer="$5"
    branded="${binary}.branded"

    "$base_binary" distribution inject \
        --input "$base_binary" \
        --output-path "$branded" \
        --channel "$channel" \
        --installer "$installer" \
        --version "$version" \
        --source-commit "$source_commit"
    mv "$branded" "$binary"
    if [ "$(uname -s)" = "Darwin" ]; then
        # Patching a Mach-O invalidates its signature. A future Developer ID signing step must
        # run after this same channel-branding boundary.
        codesign --force --sign - "$binary"
    fi

    metadata=$("$binary" distribution show)
    case "$metadata" in
        *"channel: ${expected_channel}"*"installer: ${expected_installer}"*) ;;
        *)
            echo "distribution metadata verification failed for $archive" >&2
            exit 1
            ;;
    esac

    temporary_archive="${archive}.tmp"
    COPYFILE_DISABLE=1 tar -cJf "$temporary_archive" -C "$work_dir" "$(basename "$root")"
    mv "$temporary_archive" "$archive"
    write_checksum "$archive"
}

brand_archive "$archive" github-release bash GithubRelease Bash
