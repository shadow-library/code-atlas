#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

main_checkout="$(git rev-parse --path-format=absolute --git-common-dir)/.."
denylist="${ATLAS_DENYLIST:-$main_checkout/eval/private/denylist.txt}"
[[ -f "$denylist" ]] || exit 0

terms=$(mktemp)
trap 'rm -f "$terms"' EXIT
tr -d '\r' < "$denylist" | grep -v -E '^[[:space:]]*(#|$)' > "$terms" || true
[[ -s "$terms" ]] || exit 0

hits=$(git grep -n -i -F -f "$terms" -- . ':(exclude)eval/private' ':(exclude)scratch-pad' | cut -d: -f1,2 || true)
[[ -z "$hits" ]] && exit 0

echo "tracked files contain denylisted terms at:" >&2
echo "$hits" >&2
exit 1
