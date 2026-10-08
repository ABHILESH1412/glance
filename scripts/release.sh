#!/bin/bash
# SPDX-FileCopyrightText: 2026 Abhilesh Singh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Make a release: everything after you have committed your work.
#
#   scripts/release.sh            2.0.0 -> 2.0.1   (also: make release)
#   scripts/release.sh minor      2.0.0 -> 2.1.0   (also: make release-minor)
#   scripts/release.sh major      2.0.0 -> 3.0.0   (also: make release-major)
#   scripts/release.sh --dry-run  say what would happen, change nothing
#   ... --yes                     do not ask before going ahead
#
# In order:
#   1. checks: on main, everything committed, nothing on GitHub missing here,
#      and the tools it needs are there;
#   2. works out the new version, and release notes from the commit messages
#      since the last release, and asks before going on;
#   3. writes the new version into Cargo.toml, Cargo.lock, the Arch, Fedora
#      and Debian recipes and the AppStream release list; runs the tests;
#   4. commits that as "Release vX.Y.Z" and tags it — on this computer only;
#   5. builds the three packages (scripts/build-packages.sh);
#   6. only then pushes the branch and the tag, and publishes the GitHub
#      release with the packages and SHA256SUMS attached.
#
# If anything fails before step 6, nothing has left this computer, and the
# script says how to take the release commit and tag back.
#
# Needs: git, cargo, makepkg, flatpak with flatpak-builder (or org.flatpak.Builder),
# docker (running), and the GitHub CLI, signed in: `sudo pacman -S github-cli`,
# then `gh auth login` once.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
APPID=io.github.abhilesh1412.Glance

part=patch
dry_run=false
assume_yes=false
for argument in "$@"; do
    case "$argument" in
        patch | minor | major) part=$argument ;;
        --dry-run) dry_run=true ;;
        --yes | -y) assume_yes=true ;;
        -h | --help) sed -n '5,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown option: $argument (try --help)" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mStopped:\033[0m %s\n' "$*" >&2; exit 1; }

# --- 1. Checks ---------------------------------------------------------------
say "Checking"
for tool in git cargo makepkg flatpak docker gh sha256sum; do
    command -v "$tool" > /dev/null || fail "$tool is not installed."
done
command -v flatpak-builder > /dev/null || flatpak info org.flatpak.Builder > /dev/null 2>&1 \
    || fail "flatpak-builder is not installed (flatpak install --user flathub org.flatpak.Builder)."
# A dry run builds and publishes nothing, so these two can wait.
if ! $dry_run; then
    docker info > /dev/null 2>&1 || fail "Docker is not running (sudo systemctl start docker)."
    gh auth status > /dev/null 2>&1 || fail "The GitHub CLI is not signed in (gh auth login)."
fi

branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" = main ] || fail "This is the '$branch' branch; releases are made from main."
if [ -n "$(git status --porcelain)" ]; then
    $dry_run || fail "There are uncommitted changes. Commit them first (git add, git commit)."
    echo "Note: there are uncommitted changes; a real release would stop here."
fi
git fetch --quiet --tags origin
[ "$(git rev-list --count HEAD..origin/main)" = 0 ] || fail "GitHub has commits this copy does not. Pull them first (git pull)."

# --- 2. The new version, and its notes -----------------------------------------
current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IFS=. read -r major minor patch <<< "$current"
case "$part" in
    major) next="$((major + 1)).0.0" ;;
    minor) next="$major.$((minor + 1)).0" ;;
    patch) next="$major.$minor.$((patch + 1))" ;;
esac
tag="v$next"
git rev-parse -q --verify "refs/tags/$tag" > /dev/null && fail "The tag $tag already exists."

last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2> /dev/null || true)
range=${last:+$last..}HEAD
# Every line of every commit message, so a commit with a [feat] line and a
# [fix] line gives both; not the release commits, sign-offs or repeats.
mapfile -t changes < <(git log --no-merges --format=%B "$range" | sed 's/^\s*//; s/\s*$//' \
    | grep -v -e '^$' -e '^Release v' -e '^[A-Za-z-]*-by: ' | awk '!seen[$0]++' || true)
[ ${#changes[@]} -gt 0 ] || fail "Nothing has been committed since ${last:-the start}."

notes="$ROOT/target/release-notes.md"
mkdir -p "$ROOT/target"
{
    echo "## What's new"
    echo
    for change in "${changes[@]}"; do echo "- $change"; done
    echo
    echo "## Install"
    echo
    echo "See the [install guide](https://github.com/ABHILESH1412/glance#installing) for your distribution."
    echo "Glance updates itself once installed; turn that off in Preferences → General → Updates."
} > "$notes"

say "Glance $current → $next  ($part release${last:+, since $last})"
cat "$notes"
if $dry_run; then
    echo
    echo "Dry run: nothing changed."
    exit 0
fi
if ! $assume_yes; then
    echo
    read -r -p "Make release $tag and publish it on GitHub? [y/N] " answer
    [[ "$answer" =~ ^[Yy] ]] || { echo "Nothing changed."; exit 0; }
fi

undo() {
    cat >&2 << EOF

Nothing has been pushed or published. To take the release back:
    git tag -d $tag
    git reset --hard HEAD~1
EOF
}

# --- 3. Write the new version --------------------------------------------------
say "Writing version $next"
today=$(date +%F)
sed -i "0,/^version = \"$current\"/s//version = \"$next\"/" Cargo.toml
# Cargo.lock's entry for Glance itself, the line after its name.
sed -i "/^name = \"glance\"$/{n;s/^version = \".*\"/version = \"$next\"/}" Cargo.lock
sed -i "s/^pkgver=.*/pkgver=$next/; s/^pkgrel=.*/pkgrel=1/; s/tag v[0-9.]* unpacks to glance-[0-9.]*\//tag $tag unpacks to glance-$next\//" packaging/aur/PKGBUILD
sed -i "s/^\(Version:\s*\).*/\1$next/" packaging/fedora/glance.spec
sed -i "s/^\(\s*#\s*tag: \)v[0-9.]*$/\1$tag/" build-aux/$APPID.yaml
{
    echo "glance-image-viewer ($next-1) unstable; urgency=medium"
    echo
    for change in "${changes[@]}"; do echo "  * $change"; done
    echo
    echo " -- Abhilesh Singh <sabhilesh260@gmail.com>  $(date -R)"
    echo
    cat packaging/debian/changelog
} > target/changelog.new
mv target/changelog.new packaging/debian/changelog
# AppStream: the release, newest first, with what changed.
python3 - "$next" "$today" "${changes[@]}" << 'PY'
import sys
from xml.sax.saxutils import escape
version, date, changes = sys.argv[1], sys.argv[2], sys.argv[3:]
path = "data/io.github.abhilesh1412.Glance.metainfo.xml"
text = open(path).read()
items = "\n".join(f"          <li>{escape(c)}</li>" for c in changes[:15])
entry = (f'    <release version="{version}" date="{date}">\n      <description>\n        <ul>\n'
         f"{items}\n        </ul>\n      </description>\n    </release>\n")
marker = "  <releases>\n"
if marker not in text:
    sys.exit("no <releases> in the metainfo")
open(path, "w").write(text.replace(marker, marker + entry, 1))
PY

# Flatpak builds with no network, so every crate has to be listed in
# build-aux/cargo-sources.json. If Cargo.lock has changed since the last
# release, beyond Glance's own version, the list is made again.
if [ -n "$last" ] && ! git diff --quiet "$last" -- Cargo.lock \
    && git diff "$last" -- Cargo.lock | grep '^[-+]' | grep -v '^[-+][-+]' | grep -vq '^[-+]version = '; then
    say "Cargo.lock changed: making build-aux/cargo-sources.json again"
    fcg="$ROOT/target/flatpak-cargo-generator"
    mkdir -p "$fcg"
    [ -s "$fcg/flatpak-cargo-generator.py" ] || curl -fsSL -o "$fcg/flatpak-cargo-generator.py" \
        https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
    [ -x "$fcg/venv/bin/python" ] || { python3 -m venv "$fcg/venv" && "$fcg/venv/bin/pip" install --quiet aiohttp PyYAML tomlkit; }
    "$fcg/venv/bin/python" "$fcg/flatpak-cargo-generator.py" Cargo.lock -o build-aux/cargo-sources.json
fi

say "Testing"
make check || { git checkout -- .; fail "The tests failed, so nothing was released. The version change was undone."; }

# --- 4. Commit and tag, here only ---------------------------------------------
say "Committing and tagging $tag (not pushed yet)"
git commit --quiet -am "Release $tag"
git tag -a "$tag" -m "Glance $next" -m "$(printf -- '- %s\n' "${changes[@]}")"

# --- 5. Build ---------------------------------------------------------------
out="$ROOT/target/packaging/release/$tag"
rm -rf "$out"
if ! scripts/build-packages.sh "$out"; then
    undo
    fail "A package did not build."
fi

# --- 6. Publish ---------------------------------------------------------------
say "Pushing to GitHub"
git push origin main
git push origin "$tag"

say "Publishing the release"
gh release create "$tag" --title "Glance $next" --notes-file "$notes" --latest --verify-tag \
    "$out/Glance-x86_64.AppImage" \
    "$out/Glance-x86_64.flatpak" \
    "$out/glance-image-viewer-x86_64.pkg.tar.zst" \
    "$out/SHA256SUMS"

say "Released Glance $next"
gh release view "$tag" --json url --jq .url
