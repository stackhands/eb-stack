#!/usr/bin/env bash
# Run scripts/parity_audit.py over the easybuild-easyconfigs commit, seed
# and sample size that a baseline records, and hold the run to it.
#
# Usage: scripts/parity-audit-corpus.sh <cache-dir> [parity_audit.py args...]
#
# The checkout is a shallow fetch of that one commit into
# <cache-dir>/easybuild-easyconfigs. A clean checkout already at the commit
# is reused; anything else there is fetched again.
#
# PARITY_BASELINE  baseline (default scripts/parity-audit-corpus-baseline.json)
# EASYCONFIGS_URL  remote (default the easybuilders GitHub repository)
# PYTHON           interpreter with the locked EasyBuild (default python)

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cache=${1:?usage: scripts/parity-audit-corpus.sh <cache-dir> [parity_audit.py args...]}
shift
baseline=${PARITY_BASELINE:-$root/scripts/parity-audit-corpus-baseline.json}
url=${EASYCONFIGS_URL:-https://github.com/easybuilders/easybuild-easyconfigs.git}
python=${PYTHON:-python}

read -r commit seed sample_size < <(
  "$python" -c 'import json, sys
doc = json.load(open(sys.argv[1], encoding="utf-8"))
print(doc["easyconfigs_commit"], doc["seed"], doc["sample_size"])' "$baseline"
)

checkout=$cache/easybuild-easyconfigs
if [[ -d $checkout/.git ]]; then
  head=$(git -C "$checkout" rev-parse HEAD 2>/dev/null || true)
  if [[ $head != "$commit" || -n $(git -C "$checkout" status --porcelain) ]]; then
    rm -rf "$checkout"
  fi
fi
if [[ ! -d $checkout/.git ]]; then
  rm -rf "$checkout"
  mkdir -p "$cache"
  git init -q "$checkout"
  git -C "$checkout" fetch -q --depth 1 --no-tags "$url" "$commit"
  git -C "$checkout" -c advice.detachedHead=false checkout -q --detach FETCH_HEAD
fi
test "$(git -C "$checkout" rev-parse HEAD)" = "$commit"

exec "$python" "$root/scripts/parity_audit.py" \
  --easyconfigs "$checkout/easybuild/easyconfigs" \
  --seed "$seed" \
  --sample-size "$sample_size" \
  --baseline "$baseline" \
  "$@"
