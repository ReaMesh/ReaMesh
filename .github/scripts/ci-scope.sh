#!/usr/bin/env bash
# Decides which jobs of build.yml a push needs, from what changed since the last
# successful run on this branch. Measuring from there, not from the previous
# push, means a change is only ever skipped once a run has tested it: a push
# that cancels or follows a failed run still gets the earlier commits' jobs.
#
# Anything uncertain (no earlier success, rewritten history, a path nobody
# classified, a failing API call) runs everything.
#
#   ci-scope.sh              writes the outputs to $GITHUB_OUTPUT
#   ci-scope.sh --classify   reads file names on stdin, prints the outputs
set -uo pipefail

print() {
  local any_core=$1 any_app=$2 any_relay=$3
  echo "core=$any_core"
  echo "relay=$any_relay"
  [[ $any_core == true || $any_app == true ]] && echo "build=true" || echo "build=false"
  [[ $any_core == true || $any_relay == true ]] && echo "tests=true" || echo "tests=false"
}

classify() {
  local core=false app=false relay=false f
  while IFS= read -r f; do
    case "$f" in
      # What the workflow's paths-ignore lists: nothing builds these.
      *.md | LICENSE | docs/* | proprietary/changelog/* | proprietary/screenshots/* | \
      proprietary/dmg/* | deny.toml | \
      skills-lock.json | .gitignore | .github/ISSUE_TEMPLATE/* | .github/dependabot.yml | \
      .github/workflows/channel.yml | .github/workflows/downloads.yml | \
      .github/workflows/licenses.yml | .github/workflows/release.yml) ;;
      # The Intel check reads the minimum macOS from here.
      app/src-tauri/tauri.conf.json) core=true app=true ;;
      app/* | proprietary/*) app=true ;;
      rust/* | extension/* | tools/* | build.sh) core=true ;;
      # The core depends on the contract; its list tests run the real relay.
      relay-proto/*) core=true relay=true ;;
      relay/*) relay=true ;;
      *) core=true app=true relay=true ;;
    esac
  done
  print "$core" "$app" "$relay"
}

if [[ ${1:-} == --classify ]]; then
  classify
  exit 0
fi

everything() {
  echo "Running everything: $1"
  print true true true >> "$GITHUB_OUTPUT"
  exit 0
}

[[ $GITHUB_EVENT_NAME == push ]] || everything "started by $GITHUB_EVENT_NAME"
[[ $GITHUB_RUN_ATTEMPT == 1 ]] || everything "a re-run"

base=$(gh run list --repo "$GITHUB_REPOSITORY" --workflow build.yml --branch "$GITHUB_REF_NAME" \
  --status success --limit 1 --json headSha --jq '.[0].headSha // ""') || everything "no run list"
[[ -n $base ]] || everything "no earlier success on $GITHUB_REF_NAME"
[[ $base != "$GITHUB_SHA" ]] || everything "this commit already passed"

compare=$(gh api "repos/$GITHUB_REPOSITORY/compare/$base...$GITHUB_SHA") || everything "no comparison"
status=$(jq -r .status <<< "$compare")
[[ $status == ahead ]] || everything "$GITHUB_SHA is $status of $base"
# The API lists at most 300 files.
[[ $(jq '.files | length' <<< "$compare") -lt 300 ]] || everything "too many changed files"

echo "Changes since $base:"
files=$(jq -r '.files[] | .filename, (.previous_filename // empty)' <<< "$compare")
echo "$files"
classify <<< "$files" | tee -a "$GITHUB_OUTPUT"
