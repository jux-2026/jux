#!/bin/sh
set -eu

version="$1"
archive="${2:-target/distrib/jux-npm-package.tar.gz}"
if [ ! -f "$archive" ]; then
    echo "npm installer package not found: $archive" >&2
    exit 1
fi

work_dir=$(mktemp -d)
cleanup() {
    rm -rf "$work_dir"
}
trap cleanup EXIT

tar -xzf "$archive" -C "$work_dir"
package_json=$(find "$work_dir" -type f -name package.json -print -quit)
if [ -z "$package_json" ]; then
    echo "package.json not found in $archive" >&2
    exit 1
fi

# cargo-dist generates a portable npm launcher that downloads release archives at install time.
# Point it at the Npm-branded variants rather than the Bash/PowerShell variants of the same build.
temporary_json="${package_json}.tmp"
jq '
    .supportedPlatforms |= (
        with_entries(
            select(
                .key == "aarch64-apple-darwin"
                or .key == "x86_64-unknown-linux-gnu"
                or .key == "x86_64-pc-windows-msvc"
            )
        )
        | with_entries(
            .value.artifactName |=
                if startswith("jux-npm-") then . else sub("^jux-"; "jux-npm-") end
        )
    )
' "$package_json" > "$temporary_json"
mv "$temporary_json" "$package_json"

if ! jq -e \
    --arg version "$version" \
    '.name == "@jux-2026/jux"
     and .version == $version
     and (.artifactDownloadUrls | length > 0)
     and ([.artifactDownloadUrls[] | startswith("https://github.com/jux-2026/jux/releases/download/")] | all)
     and (.supportedPlatforms | length == 3)
     and ([.supportedPlatforms[].artifactName | startswith("jux-npm-")] | all)' \
    "$package_json" >/dev/null; then
    echo "npm installer package verification failed" >&2
    exit 1
fi

root=$(dirname "$package_json")
temporary_archive="${archive}.tmp"
COPYFILE_DISABLE=1 tar -czf "$temporary_archive" -C "$(dirname "$root")" "$(basename "$root")"
mv "$temporary_archive" "$archive"
