#!/usr/bin/env bash
#
# Pull the team's latest changes (branch review-stable of the `team` remote) into
# the working tree, as ordinary uncommitted changes.
#
# The team's history is never merged, so what you commit is a normal commit under
# your own name. The last commit that was imported is recorded in .team-sync, and
# only what changed after it is applied (with a three-way merge, so edits we made
# here since then are kept and real conflicts are marked in the files).
#
#   build-tools/sync-team.sh        # fetch and apply
#   git status && git diff          # review
#   git add -A && git commit        # commit it as yourself
#
# Needs: a remote named `team` pointing at the team's repository.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
BRANCH=${TEAM_BRANCH:-review-stable}

git remote get-url team > /dev/null 2>&1 || { echo "no git remote named 'team'" >&2; exit 1; }
git diff --quiet && git diff --cached --quiet || { echo "commit or stash your changes first" >&2; exit 1; }

git fetch team "$BRANCH"
last=$(cat .team-sync)
new=$(git rev-parse "team/$BRANCH")
if [ "$last" = "$new" ]; then
	echo "already up to date at ${new:0:8}"
	exit 0
fi

echo "importing ${last:0:8}..${new:0:8}: $(git rev-list --count "$last..$new") commits"
# node_modules and a build log were committed upstream by mistake; keep them out.
git diff --binary "$last" "$new" -- . \
	':(exclude)ui/osaima-shell/node_modules' \
	':(exclude)ui/osaima-shell/src-tauri/build_error.txt' |
	git apply --3way --index --whitespace=nowarn || {
	echo "some changes conflict: fix the marked files, then git add them" >&2
	echo "$new" > .team-sync
	exit 1
}
echo "$new" > .team-sync
git add .team-sync
echo "applied. Review with: git diff --cached --stat"
