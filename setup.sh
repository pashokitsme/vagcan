#!/usr/bin/env bash
#
# Unpack the vendored VCDS installations for local development.
#
# The archives under vendor/ are Ross-Tech's VCDS, unmodified, tracked with Git
# LFS. VCDS is Ross-Tech's software, free to download from
# https://www.ross-tech.com/vcds/ — this repository redistributes it unchanged
# for convenience and claims no ownership of it. The label data it contains is
# what `vagcan setup` parses; nothing here decompiles or alters the program.
#
# This script is for working ON vagcan. A user of the built tool does not run
# it — they point `vagcan setup` at their own VCDS installation, or fetch one.
#
set -euo pipefail

cd "$(dirname "$0")/vendor"

if ! command -v git-lfs >/dev/null 2>&1; then
    echo "git-lfs is not installed; the archives are LFS pointers without it." >&2
    echo "Install it (https://git-lfs.com) and run 'git lfs pull', then re-run." >&2
    exit 1
fi

for lang in en ru; do
    archive="vcds-$lang.zip"
    if [ ! -f "$archive" ]; then
        echo "$archive is missing." >&2
        continue
    fi
    # An LFS pointer is a few hundred bytes; the real archive is tens of MB. If
    # the file is tiny, LFS has not fetched it yet, and unzip would fail with a
    # confusing error instead of a useful one.
    if [ "$(wc -c < "$archive")" -lt 1000000 ]; then
        echo "$archive looks like an unfetched LFS pointer. Run 'git lfs pull'." >&2
        continue
    fi
    echo "unpacking $archive …"
    rm -rf "vcds-$lang"
    unzip -q "$archive"
done

echo "done. The installations are under vendor/vcds-en and vendor/vcds-ru."
