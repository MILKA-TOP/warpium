#!/usr/bin/env bash
#
# Prints the GitHub Release notes (Markdown) for a Warpium release tag.
#
# Usage: script/warpium/release_notes.sh [options] > notes.md
#   --tag TAG         Warpium release tag (default: script/warpium/version --tag)
#   --ref REF         commit, branch or tag the release is built from, used to decide which
#                     PRs are part of it (default: TAG). Use e.g. `--ref master` to preview
#                     the notes for a tag that doesn't exist yet.
#   --previous TAG    previous Warpium release tag (default: the highest published Warpium
#                     release below TAG in version order; none for the first release)
#   --dmg PATH        the release .dmg; its SHA-256 checksum is added to the notes
#   --repo OWNER/REPO GitHub repository (default: $GH_REPO, else MILKA-TOP/warpium)
#
# "What's new" lists the pull requests merged into master whose merge commit is part of REF
# but not of the previous release tag. Membership is checked with GitHub's compare API, so
# no local Git history is needed; only an authenticated `gh` (GH_TOKEN in CI).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

TAG=""
REF=""
PREVIOUS=""
PREVIOUS_SET=0
DMG=""
REPO="${GH_REPO:-MILKA-TOP/warpium}"

usage() {
  sed -n '3,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
    --ref) REF="${2:?--ref needs a value}"; shift 2 ;;
    --previous) PREVIOUS="${2-}"; PREVIOUS_SET=1; shift 2 ;;
    --dmg) DMG="${2:?--dmg needs a value}"; shift 2 ;;
    --repo) REPO="${2:?--repo needs a value}"; shift 2 ;;
    -h | --help) usage ;;
    *) echo "error: unknown argument '$1'" >&2; usage ;;
  esac
done

[[ -n "$TAG" ]] || TAG="$("$SCRIPT_DIR/version" --tag)"
[[ -n "$REF" ]] || REF="$TAG"

# A Warpium tag is an upstream Warp release tag plus a `.N` fork counter.
WARPIUM_TAG_RE='^v[0-9]+\.[0-9]{4}(\.[0-9]{2}){4}\.[a-z]+_[0-9]{2}\.[1-9][0-9]*$'
if [[ ! "$TAG" =~ $WARPIUM_TAG_RE ]]; then
  echo "error: '$TAG' is not a Warpium release tag (vN.YYYY.MM.DD.HH.MM.channel_NN.N)" >&2
  exit 1
fi
UPSTREAM_TAG="${TAG%.*}"
DMG_NAME="Warpium-$TAG-arm64.dmg"

fail() {
  echo "error: $*" >&2
  exit 1
}

# Seconds since the epoch for an ISO 8601 UTC timestamp (GNU or BSD date).
epoch() {
  date -u -d "$1" +%s 2>/dev/null || date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$1" +%s
}

# ancestor A B: succeeds when commit A is reachable from B, fails when it isn't, and aborts
# the script when GitHub can't answer, so an API error never silently drops or adds a PR.
ancestor() {
  local status
  if ! status="$(gh api "repos/$REPO/compare/$1...$2?per_page=1" --jq .status)"; then
    fail "could not compare $1 with $2"
  fi
  case "$status" in
    ahead | identical) return 0 ;;
    behind | diverged) return 1 ;;
    *) fail "unexpected compare status '$status' for $1...$2" ;;
  esac
}

ref_sha="$(gh api "repos/$REPO/commits/$REF" --jq .sha 2>/dev/null)" ||
  fail "could not resolve $REF in $REPO; check gh auth, or pass --ref master to preview an untagged release"
ref_epoch="$(epoch "$(gh api "repos/$REPO/commits/$ref_sha" --jq .commit.committer.date)")"

if [[ "$PREVIOUS_SET" == 0 ]]; then
  # The highest published Warpium release below TAG in version order, so re-publishing an
  # older release still compares against the one before it.
  release_tags="$(gh api --paginate "repos/$REPO/releases?per_page=100" \
    --jq '.[] | select(.draft | not) | .tag_name')"
  PREVIOUS="$({
    grep -E "$WARPIUM_TAG_RE" <<<"$release_tags" || true
    echo "$TAG"
  } | sort -V -u | awk -v tag="$TAG" '$0 == tag { print prev; exit } { prev = $0 }')"
fi

# Candidate PRs: everything merged into master since the day before the previous release's
# commit, or all of them for the first release. Timestamps only narrow the candidates (with a
# day of margin for clock skew between commit and merge times); the compare API decides
# exactly which PRs the release contains.
DAY=86400
search="base:master"
if [[ -n "$PREVIOUS" ]]; then
  prev_sha="$(gh api "repos/$REPO/commits/$PREVIOUS" --jq .sha)"
  prev_epoch="$(epoch "$(gh api "repos/$REPO/commits/$prev_sha" --jq .commit.committer.date)")"
  since_epoch=$((prev_epoch - DAY))
  since="$(date -u -d "@$since_epoch" +%Y-%m-%d 2>/dev/null || date -u -r "$since_epoch" +%Y-%m-%d)"
  search+=" merged:>=$since"
fi

PR_LIMIT=1000
# Captured first (not read from a process substitution) so a failed query aborts the script.
pr_rows="$(gh pr list --repo "$REPO" --state merged --base master --search "$search" \
  --limit "$PR_LIMIT" --json number,title,mergedAt,mergeCommit \
  --jq 'sort_by(.mergedAt) | .[] | select(.mergeCommit != null) | [.number, .mergeCommit.oid, .mergedAt, .title] | @tsv')"
if [[ "$(grep -c . <<<"$pr_rows")" -ge "$PR_LIMIT" ]]; then
  fail "more than $PR_LIMIT candidate PRs; pass --previous to narrow the range"
fi

prs=()
while IFS=$'\t' read -r number merge_sha merged_at title; do
  [[ -n "$number" ]] || continue
  merged_epoch="$(epoch "$merged_at")"
  # Merged well after REF's commit: can't be part of it.
  ((merged_epoch <= ref_epoch + DAY)) || continue
  ancestor "$merge_sha" "$ref_sha" || continue
  # Merged well after the previous release's commit: can't be part of that release.
  if [[ -n "$PREVIOUS" ]] && ((merged_epoch <= prev_epoch + DAY)) && ancestor "$merge_sha" "$prev_sha"; then
    continue
  fi
  prs+=("- $title (#$number)")
done <<<"$pr_rows"

checksum=""
if [[ -n "$DMG" ]]; then
  DMG_NAME="$(basename "$DMG")"
  if [[ -f "$DMG.sha256" ]]; then
    # Reuse the checksum file uploaded next to the .dmg, so the notes and the asset agree.
    checksum="$(cut -d' ' -f1 "$DMG.sha256")"
  elif command -v sha256sum >/dev/null; then
    checksum="$(sha256sum "$DMG" | cut -d' ' -f1)"
  else
    checksum="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"
  fi
  [[ "$checksum" =~ ^[0-9a-f]{64}$ ]] || fail "could not read the SHA-256 of $DMG"
fi

# --- Notes -------------------------------------------------------------------------------

echo "## What's new"
echo
if [[ -z "$PREVIOUS" ]]; then
  echo "The first Warpium release. Changes on top of upstream Warp:"
  echo
fi
if [[ ${#prs[@]} -gt 0 ]]; then
  printf '%s\n' "${prs[@]}"
else
  echo "- Maintenance release with no new pull requests."
fi
if [[ -n "$PREVIOUS" ]]; then
  echo
  echo "**Full changelog:** https://github.com/$REPO/compare/$PREVIOUS...$TAG"
fi

cat <<EOF

Based on upstream Warp [\`$UPSTREAM_TAG\`](https://github.com/warpdotdev/warp/tree/$UPSTREAM_TAG).

## Install

For Macs with Apple silicon (arm64) only.

1. Download \`$DMG_NAME\` from the assets below.
2. Open the \`.dmg\` and drag **Warpium** to **Applications**.
3. Warpium is not notarized by Apple, so macOS blocks it at first. Either run this in Terminal:

   \`\`\`bash
   xattr -dr com.apple.quarantine /Applications/Warpium.app
   \`\`\`

   or try to open it once, then go to **System Settings → Privacy & Security** and click **Open Anyway**.
4. Launch **Warpium** from Applications or Spotlight.

To update, download the new release and replace the app in Applications. Your settings are kept.
EOF

if [[ -n "$checksum" ]]; then
  cat <<EOF

## Checksum

SHA-256 of \`$DMG_NAME\`:

\`\`\`
$checksum
\`\`\`

To verify, download \`$DMG_NAME.sha256\` next to the \`.dmg\` and run
\`shasum -a 256 -c $DMG_NAME.sha256\` in that folder.
EOF
fi

cat <<'EOF'

## Known limitations

- Only Apple silicon (arm64) Macs are supported, and the app is ad-hoc signed, not notarized.
- The built-in `modify-settings` agent skill is missing from downloaded builds; builds from source include it.
- There are no automatic updates: "Check for updates" is not available, so get new versions from Releases.
- Warpium shares `~/.warp-oss` (settings, themes, skills) with upstream Warp's open-source build.
- Don't install it next to an upstream `WarpOss.app`: both handle `warposs://` links, so sign-in may open the wrong app.
EOF
