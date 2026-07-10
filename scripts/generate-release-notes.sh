#!/usr/bin/env bash

set -euo pipefail

output="${1:-release-notes.md}"
current_tag="${2:-$(git describe --tags --exact-match HEAD)}"
previous_tag="$(git describe --tags --abbrev=0 "${current_tag}^" 2>/dev/null || true)"

{
  printf '# Nucleotide %s\n\n' "${current_tag#v}"
  if [ -n "${previous_tag}" ]; then
    git log --no-merges --pretty='format:- %s' "${previous_tag}..${current_tag}"
  else
    git log -1 --pretty='format:- %s' "${current_tag}"
  fi
  printf '\n'
} >"${output}"

test -s "${output}"
