#!/bin/sh
# Print the CHANGELOG.md section of one version (no heading), for the tag
# message and the GitHub release. Fails if the section is missing or empty.
#   scripts/release-notes.sh 0.10.0
set -eu
[ $# -eq 1 ] || { echo "usage: $0 VERSION" >&2; exit 2; }
dir=$(cd "$(dirname "$0")/.." && pwd)
notes=$(awk -v v="$1" '
  /^## \[/ { on = (index($0, "## [" v "]") == 1); next }
  on { print }
' "$dir/CHANGELOG.md" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}' | sed '/./,$!d')
[ -n "$notes" ] || { echo "no CHANGELOG.md entry for version $1" >&2; exit 1; }
printf '%s\n' "$notes"
