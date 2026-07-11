#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/nucleotide-release-test.XXXXXX")"
trap 'rm -rf "${fixture_root}"' EXIT HUP INT TERM

remote="${fixture_root}/origin.git"
worktree="${fixture_root}/worktree"

git init --bare --initial-branch=main "${remote}" >/dev/null
git init --initial-branch=main "${worktree}" >/dev/null

mkdir -p "${worktree}/app/src" "${worktree}/scripts"
cp "${repo_root}/scripts/release.sh" "${worktree}/scripts/release.sh"

cat > "${worktree}/Cargo.toml" <<'EOF'
[workspace]
members = ["app"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
EOF

cat > "${worktree}/app/Cargo.toml" <<'EOF'
[package]
name = "release-fixture"
version.workspace = true
edition.workspace = true
EOF

cat > "${worktree}/app/src/lib.rs" <<'EOF'
pub fn fixture() -> bool {
    true
}
EOF

(
  cd "${worktree}"
  git config user.name "Nucleotide Release Test"
  git config user.email "release-test@example.invalid"
  cargo generate-lockfile
  git add Cargo.toml Cargo.lock app scripts
  git commit -m "test: initialize release fixture" >/dev/null
  git remote add origin "${remote}"
  git push --set-upstream origin main >/dev/null

  ./scripts/release.sh >/dev/null

  test "$(git log -1 --format=%s)" = "chore(release): v0.2.0"
  test "$(git describe --tags --exact-match HEAD)" = "v0.2.0"
  grep -Fxq 'version = "0.2.0"' Cargo.toml
  cargo metadata --format-version 1 --no-deps --locked >/dev/null

  ./scripts/release.sh patch >/dev/null

  test "$(git log -1 --format=%s)" = "chore(release): v0.2.1"
  test "$(git describe --tags --exact-match HEAD)" = "v0.2.1"
  grep -Fxq 'version = "0.2.1"' Cargo.toml
  cargo metadata --format-version 1 --no-deps --locked >/dev/null

  ./scripts/release.sh major >/dev/null

  test "$(git log -1 --format=%s)" = "chore(release): v1.0.0"
  test "$(git describe --tags --exact-match HEAD)" = "v1.0.0"
  grep -Fxq 'version = "1.0.0"' Cargo.toml
  cargo metadata --format-version 1 --no-deps --locked >/dev/null

  printf 'dirty\n' >> app/src/lib.rs
  if ./scripts/release.sh patch >/dev/null 2>&1; then
    echo "release script accepted a dirty worktree" >&2
    exit 1
  fi
)

branch_commit="$(git --git-dir="${remote}" rev-parse refs/heads/main)"
tag_commit="$(git --git-dir="${remote}" rev-parse 'refs/tags/v1.0.0^{}')"
test "${branch_commit}" = "${tag_commit}"

printf 'release script tests passed\n'
