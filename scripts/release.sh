#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

remote="${NUCL_RELEASE_REMOTE:-origin}"
release_branch="${NUCL_RELEASE_BRANCH:-main}"

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh [patch|minor|major|<version>]

Create and push a Nucleotide release commit and tag.

Examples:
  ./scripts/release.sh
  ./scripts/release.sh patch
  ./scripts/release.sh 0.2.0
  ./scripts/release.sh v0.2.0

With no argument, the script increments the minor version. It requires a
clean, up-to-date main branch, updates the workspace version and Cargo.lock,
creates an annotated vX.Y.Z tag, then atomically pushes the release commit
and tag to origin.
EOF
}

fail() {
  printf 'release: %s\n' "$*" >&2
  exit 1
}

workspace_version() {
  awk '
    /^\[workspace[.]package\]$/ { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && /^version[[:space:]]*=[[:space:]]*"/ {
      line = $0
      sub(/^[^\"]*\"/, "", line)
      sub(/\".*$/, "", line)
      print line
      exit
    }
  ' Cargo.toml
}

increment_version() {
  local current="$1"
  local increment="$2"
  local core major minor patch

  core="${current%%[-+]*}"
  if ! [[ "${core}" =~ ^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)$ ]]; then
    fail "cannot increment invalid workspace version: ${current}"
  fi

  IFS=. read -r major minor patch <<< "${core}"
  case "${increment}" in
    major)
      major=$((major + 1))
      minor=0
      patch=0
      ;;
    minor)
      minor=$((minor + 1))
      patch=0
      ;;
    patch)
      patch=$((patch + 1))
      ;;
    *)
      fail "unsupported version increment: ${increment}"
      ;;
  esac

  printf '%s.%s.%s\n' "${major}" "${minor}" "${patch}"
}

set_workspace_version() {
  local version="$1"
  local manifest_tmp

  manifest_tmp="$(mktemp "Cargo.toml.release.XXXXXX")"
  trap 'rm -f "${manifest_tmp:-}"' EXIT HUP INT TERM

  if ! awk -v version="${version}" '
    /^\[workspace[.]package\]$/ {
      in_workspace_package = 1
      print
      next
    }
    in_workspace_package && /^\[/ {
      in_workspace_package = 0
    }
    in_workspace_package && /^version[[:space:]]*=/ {
      print "version = \"" version "\""
      replaced += 1
      next
    }
    { print }
    END {
      if (replaced != 1) {
        exit 1
      }
    }
  ' Cargo.toml > "${manifest_tmp}"; then
    fail 'could not replace [workspace.package].version in Cargo.toml'
  fi

  cp "${manifest_tmp}" Cargo.toml
  rm -f "${manifest_tmp}"
  manifest_tmp=""
  trap - EXIT HUP INT TERM
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "$#" -gt 1 ]]; then
  usage >&2
  exit 2
fi

release_spec="${1:-minor}"

for command in git cargo awk mktemp; do
  command -v "${command}" >/dev/null 2>&1 || fail "required command not found: ${command}"
done

git rev-parse --show-toplevel >/dev/null 2>&1 || fail 'not inside a Git repository'

current_branch="$(git branch --show-current)"
if [[ "${current_branch}" != "${release_branch}" ]]; then
  fail "releases must be created from ${release_branch}; current branch is ${current_branch:-detached HEAD}"
fi

if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  fail 'working tree is not clean'
fi

git remote get-url "${remote}" >/dev/null 2>&1 || fail "Git remote not found: ${remote}"
git check-ref-format --branch "${release_branch}" >/dev/null 2>&1 || fail "invalid release branch: ${release_branch}"

printf 'Fetching %s/%s and tags...\n' "${remote}" "${release_branch}"
git fetch --tags "${remote}" "${release_branch}"

local_head="$(git rev-parse HEAD)"
remote_head="$(git rev-parse FETCH_HEAD)"
if [[ "${local_head}" != "${remote_head}" ]]; then
  fail "local ${release_branch} is not up to date with ${remote}/${release_branch}"
fi

current_version="$(workspace_version)"
if [[ -z "${current_version}" ]]; then
  fail 'could not read [workspace.package].version from Cargo.toml'
fi

case "${release_spec}" in
  major|minor|patch)
    version="$(increment_version "${current_version}" "${release_spec}")"
    ;;
  *)
    version="${release_spec#v}"
    ;;
esac

if ! [[ "${version}" =~ ^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)([-+][0-9A-Za-z.-]+)?$ ]]; then
  fail "invalid semantic version: ${release_spec}"
fi

tag="v${version}"
if git show-ref --verify --quiet "refs/tags/${tag}"; then
  fail "tag already exists: ${tag}"
fi

if [[ "${current_version}" == "${version}" ]]; then
  fail "workspace version is already ${version}"
fi

printf 'Updating workspace version %s -> %s...\n' "${current_version}" "${version}"
set_workspace_version "${version}"

# Refresh first-party package versions in the committed lockfile without
# compiling the workspace. The locked pass verifies that the result is stable.
cargo metadata --format-version 1 --no-deps >/dev/null
cargo metadata --format-version 1 --no-deps --locked >/dev/null

updated_version="$(workspace_version)"
if [[ "${updated_version}" != "${version}" ]]; then
  fail "workspace version update produced ${updated_version:-no version}, expected ${version}"
fi

if git diff --quiet -- Cargo.toml Cargo.lock; then
  fail 'version update did not change Cargo.toml or Cargo.lock'
fi

git add Cargo.toml Cargo.lock
git diff --cached --check
git commit -m "chore(release): ${tag}"
git tag -a "${tag}" -m "Nucleotide ${tag}"

printf 'Atomically pushing %s and %s to %s...\n' "${release_branch}" "${tag}" "${remote}"
git push --atomic "${remote}" \
  "HEAD:refs/heads/${release_branch}" \
  "refs/tags/${tag}:refs/tags/${tag}"

printf 'Released %s. The tag push will start the release workflow.\n' "${tag}"
