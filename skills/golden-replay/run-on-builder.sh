#!/usr/bin/env bash
# Run argv as the current checkout's eb-stack on the configured builder.
# Paths in argv are evaluated on the builder. There is no default script.
#
#   EB_STACK_BUILDER=HOST [EB_STACK_BIN=PATH] run-on-builder.sh package ...
#
# HOST is the builder's ssh destination. PATH is the eb-stack binary there;
# unset, it is the debug build of the builder's checkout at the same place
# under the home directory as this one.
set -euo pipefail

if [[ $# -eq 0 ]]; then
  printf 'usage: EB_STACK_BUILDER=HOST run-on-builder.sh package bump|plan|inspect ...\n' >&2
  exit 2
fi

builder=${EB_STACK_BUILDER:?set EB_STACK_BUILDER to the builder, an ssh destination}

if [[ -z ${EB_STACK_BIN:-} ]]; then
  checkout=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
  if [[ $checkout != "$HOME"/* ]]; then
    printf 'this checkout is not under %s; set EB_STACK_BIN\n' "$HOME" >&2
    exit 2
  fi
  EB_STACK_BIN=$(ssh -o BatchMode=yes "$builder" 'bash -s' -- "$(printf '%q' "${checkout#"$HOME"/}")" <<'EOS'
set -euo pipefail
bin=$HOME/$1/target/debug/eb-stack
if [[ -x $bin ]]; then
  printf '%q\n' "$bin"
  exit 0
fi
printf 'no eb-stack debug binary at %s on the builder; set EB_STACK_BIN\n' "$bin" >&2
exit 1
EOS
)
fi

exec ssh -o BatchMode=yes "$builder" -- "$EB_STACK_BIN" "$@"
