#!/usr/bin/env bash
# Cut a TrontSnap release LOCALLY: bump versions, commit, annotated-tag. Never pushes.
#
# usage: scripts/release.sh X.Y.Z [--dry-run]
#
# Refuses (listing every reason it found) unless:
#   - the working tree is clean (untracked files count: a forgotten new source
#     file is exactly how "builds here, fails in CI" happens)
#   - HEAD is on a branch, and a git identity is configured
#   - X.Y.Z is greater than the current version, and tag vX.Y.Z does not exist yet
#   - Cargo.toml, hotkeyd/Cargo.toml and the two Cargo.lock entries agree today
#   - CHANGELOG.md already has a non-empty '## vX.Y.Z' section (committed; this
#     script never invents release notes)
#
# Then edits, by text (no cargo call: offline, ~100 ms, can't touch the target dir):
#   Cargo.toml, hotkeyd/Cargo.toml  [package] version
#   Cargo.lock                      the 'trontsnap' and 'trontsnap-hotkeys' entries
# commits "Release TrontSnap X.Y.Z" (those 3 files only) and tags vX.Y.Z.
#
# --dry-run: same checks, same edits (on temp copies), shows the diff, writes nothing.
# Windows box: scripts\release.ps1, same behaviour.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; }

NEW="" DRY=0
for a in "$@"; do
    case $a in
        --dry-run) DRY=1 ;;
        -h|--help) usage; exit 0 ;;
        -*)        echo "unknown option: $a" >&2; usage >&2; exit 2 ;;
        *)         [[ -z $NEW ]] || { echo "one version only (got '$NEW' and '$a')" >&2; exit 2; }
                   NEW="${a#v}" ;;
    esac
done
[[ -n $NEW ]] || { usage >&2; exit 2; }
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
[[ $NEW =~ $SEMVER ]] || { echo "version must be plain X.Y.Z (got '$NEW')" >&2; exit 2; }
TAG="v$NEW"

# === Readers ===
# HARD-WON: both Cargo.toml files start with a UTF-8 BOM, and hotkeyd's '[package]'
# header is line 1, so it never matches until the BOM is stripped. The version then
# reads back empty, which looks like "no version", not "BOM". LC_ALL=C keeps GNU sed
# treating \xEF as a byte instead of an invalid UTF-8 sequence.
toml_version() {
    LC_ALL=C sed '1s/^\xEF\xBB\xBF//' "$1" | tr -d '\r' | awk '
        /^\[/ { inpkg = ($0 ~ /^\[package\][ \t]*(#.*)?$/); next }
        inpkg && /^version[ \t]*=/ {
            v = $0; sub(/^version[ \t]*=[ \t]*"/, "", v); sub(/".*$/, "", v); print v; exit
        }'
}

# Cargo.lock: name = "x" then version = "y", one [[package]] block each.
lock_version() {
    tr -d '\r' < "$1" | awk -v want="$2" '
        /^\[\[package\]\]/ { name = ""; next }
        /^name = / { n = $0; sub(/^name = "/, "", n); sub(/".*$/, "", n); name = n; next }
        /^version = / && name == want {
            v = $0; sub(/^version = "/, "", v); sub(/".*$/, "", v); print v; exit
        }'
}

# Body of '## vX.Y.Z' up to the next '## ', blank edges trimmed. Same cut CI uses for
# the release notes (verify-release.sh). Exit 3 = no such heading.
changelog_section() {
    tr -d '\r' < "$1" | awk -v tag="$2" '
        BEGIN { h = "## " tag }
        /^## / {
            if (found) exit
            if ($0 == h || index($0, h " ") == 1) found = 1
            next
        }
        found { lines[++n] = $0 }
        END {
            if (!found) exit 3
            s = 1; while (s <= n && lines[s] ~ /^[ \t]*$/) s++
            e = n; while (e >= s && lines[e] ~ /^[ \t]*$/) e--
            for (i = s; i <= e; i++) print lines[i]
        }'
}

# newer A B: true when A > B (both plain X.Y.Z).
newer() {
    [[ $1 != "$2" ]] && [[ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" == "$1" ]]
}

# === Preflight: collect every problem, then refuse once ===
PROBLEMS=()
problem() { PROBLEMS+=("$*"); }

git rev-parse --git-dir >/dev/null 2>&1 || { echo "not inside a git repository" >&2; exit 1; }
for f in Cargo.toml hotkeyd/Cargo.toml Cargo.lock CHANGELOG.md; do
    [[ -f $f ]] || { echo "$f not found (run from a TrontSnap checkout)" >&2; exit 1; }
done

BRANCH="$(git symbolic-ref --short -q HEAD || true)"
[[ -n $BRANCH ]] || problem "HEAD is detached; check out the branch you are releasing from"

DIRTY="$(git status --porcelain)"
if [[ -n $DIRTY ]]; then
    problem "working tree is not clean ($(printf '%s\n' "$DIRTY" | wc -l) path(s)); commit or stash first:
$(printf '%s\n' "$DIRTY" | head -n 8 | sed 's/^/        /')"
fi

git var GIT_COMMITTER_IDENT >/dev/null 2>&1 \
    || problem "no git identity (git config user.name / user.email); the commit and tag would fail half-way"

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    problem "tag $TAG already exists locally"
fi

CUR="$(toml_version Cargo.toml)"
V_HK="$(toml_version hotkeyd/Cargo.toml)"
V_LT="$(lock_version Cargo.lock trontsnap)"
V_LH="$(lock_version Cargo.lock trontsnap-hotkeys)"
if [[ $CUR != "$V_HK" || $CUR != "$V_LT" || $CUR != "$V_LH" ]]; then
    problem "versions disagree today (a half-finished bump?): Cargo.toml='${CUR:-?}' hotkeyd/Cargo.toml='${V_HK:-?}' Cargo.lock trontsnap='${V_LT:-?}' trontsnap-hotkeys='${V_LH:-?}'"
fi
if [[ ! $CUR =~ $SEMVER ]]; then
    problem "current version '${CUR:-?}' is not plain X.Y.Z, so 'greater than' is undefined"
elif ! newer "$NEW" "$CUR"; then
    problem "$NEW is not greater than the current version $CUR"
fi

NOTES=""
if NOTES="$(changelog_section CHANGELOG.md "$TAG")"; then
    [[ -n $NOTES ]] || problem "CHANGELOG.md has a '## $TAG' heading but the section is empty"
else
    problem "CHANGELOG.md has no '## $TAG' section; write it (and commit it) first"
fi

if ((${#PROBLEMS[@]})); then
    echo "release.sh: refusing to release $NEW ($((${#PROBLEMS[@]})) problem(s)):" >&2
    for p in "${PROBLEMS[@]}"; do printf '  - %s\n' "$p" >&2; done
    echo "Nothing was changed." >&2
    exit 1
fi

# === Edit temp copies, prove them, only then touch the tree ===
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/hotkeyd"
cp Cargo.toml Cargo.lock CHANGELOG.md "$TMP/"
cp hotkeyd/Cargo.toml "$TMP/hotkeyd/"

# [package] table only: dependency lines like `windows = { version = "0.56" ... }` never
# start with `version`, but `[dependencies.foo]` tables could. The range ends at the
# next table header.
for f in Cargo.toml hotkeyd/Cargo.toml; do
    LC_ALL=C sed -E -i '/^(\xEF\xBB\xBF)?\[package\]/,/^\[/{s/^(version[[:space:]]*=[[:space:]]*")[^"]*"/\1'"$NEW"'"/}' "$TMP/$f"
done
# The lock always writes `name` then `version` on adjacent lines; `n` hops to the version.
LC_ALL=C sed -E -i '/^name = "(trontsnap|trontsnap-hotkeys)"/{n;s/^(version = ")[^"]*"/\1'"$NEW"'"/}' "$TMP/Cargo.lock"

# Read it all back and count changed lines: a silent no-op (pattern drift) or an
# over-eager edit must stop here, not in CI.
BAD=()
[[ "$(toml_version "$TMP/Cargo.toml")" == "$NEW" ]]          || BAD+=("Cargo.toml did not take $NEW")
[[ "$(toml_version "$TMP/hotkeyd/Cargo.toml")" == "$NEW" ]]  || BAD+=("hotkeyd/Cargo.toml did not take $NEW")
[[ "$(lock_version "$TMP/Cargo.lock" trontsnap)" == "$NEW" ]]         || BAD+=("Cargo.lock trontsnap did not take $NEW")
[[ "$(lock_version "$TMP/Cargo.lock" trontsnap-hotkeys)" == "$NEW" ]] || BAD+=("Cargo.lock trontsnap-hotkeys did not take $NEW")
changed() { diff "$1" "$2" | grep -c '^>' || true; }
[[ "$(changed Cargo.toml "$TMP/Cargo.toml")" == 1 ]]                  || BAD+=("Cargo.toml: expected exactly 1 changed line")
[[ "$(changed hotkeyd/Cargo.toml "$TMP/hotkeyd/Cargo.toml")" == 1 ]]  || BAD+=("hotkeyd/Cargo.toml: expected exactly 1 changed line")
[[ "$(changed Cargo.lock "$TMP/Cargo.lock")" == 2 ]]                  || BAD+=("Cargo.lock: expected exactly 2 changed lines")

# The exact gate CI's verify job runs, against the edited copies.
VERIFY=".github/scripts/verify-release.sh"
if [[ -f $VERIFY ]]; then
    (cd "$TMP" && bash "$ROOT/$VERIFY" "$TAG" "$TMP/notes.md" >"$TMP/verify.log" 2>&1) \
        || BAD+=("CI's verify-release.sh rejects the edited files: $(tr '\n' ' ' < "$TMP/verify.log")")
else
    echo "note: $VERIFY not found, skipping the CI-gate rehearsal" >&2
fi

if ((${#BAD[@]})); then
    echo "release.sh: the version edit did not come out right; nothing was changed:" >&2
    for p in "${BAD[@]}"; do printf '  - %s\n' "$p" >&2; done
    exit 1
fi

FILES=(Cargo.toml hotkeyd/Cargo.toml Cargo.lock)
REMOTE="$(git config --get "branch.$BRANCH.remote" 2>/dev/null || echo origin)"
PUSH="git push --atomic $REMOTE $BRANCH $TAG"

echo "== TrontSnap release $CUR -> $NEW (tag $TAG on $BRANCH) =="
echo
for f in "${FILES[@]}"; do
    diff -u --label "a/$f" --label "b/$f" "$f" "$TMP/$f" || true
done
echo
echo "CHANGELOG '## $TAG' section ($(printf '%s\n' "$NOTES" | wc -l) lines), becomes the draft release notes:"
printf '%s\n' "$NOTES" | head -n 4 | sed 's/^/    /'
echo

if ((DRY)); then
    echo "DRY RUN: nothing written. A real run would commit \"Release TrontSnap $NEW\" and tag $TAG."
else
    for f in "${FILES[@]}"; do cp "$TMP/$f" "$f"; done
    # Pathspec form: commits only these files even if something else is staged.
    if ! git commit -q -m "Release TrontSnap $NEW" -- "${FILES[@]}"; then
        # Tree was clean at the start, so checkout puts back exactly what we edited.
        git checkout -- "${FILES[@]}"
        echo "release.sh: git commit failed (a hook?); version files restored, nothing committed" >&2
        exit 1
    fi
    git tag -a "$TAG" -m "TrontSnap $NEW"
    git --no-pager show --stat --format='commit %h  %s' HEAD
    echo
    git --no-pager tag -n1 -l "$TAG"
    echo
    echo "Committed and tagged LOCALLY. Nothing has been pushed."
fi

echo
echo "Next, push the branch and the tag together (this is what starts CI):"
echo
echo "    $PUSH"
echo
echo "Then CI (.github/workflows/release.yml) will:"
echo "  1. verify   tag == Cargo.toml == hotkeyd == Cargo.lock; CHANGELOG '## $TAG' -> notes"
echo "  2. windows  build --locked; TrontSnap.exe, TrontSnap-v$NEW-windows.zip (unsigned bundle), Rajdhani-OFL.txt"
echo "  3. linux    ubuntu-22.04 build; AppImage + .zsync, .deb, tarball"
echo "  4. publish  SHA256SUMS + a DRAFT release titled 'TrontSnap $NEW'"
echo "You then set the title ('TrontSnap $NEW: <headline>') in the GitHub UI and publish."
echo "To rehearse the builds first: push the branch only, then Actions > Release > Run workflow"
echo "(branch, tag input $TAG, dry_run ticked). See RELEASING.md."
