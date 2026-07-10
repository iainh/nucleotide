#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

workspace_version() {
  awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && /^version = "/ {
      sub(/^version = "/, "")
      sub(/"$/, "")
      print
      exit
    }
  ' Cargo.toml
}

version="${NUCL_VELOPACK_VERSION:-$(workspace_version)}"
version="${version#v}"
if [ -z "${version}" ]; then
  echo "Unable to determine Velopack version. Set NUCL_VELOPACK_VERSION." >&2
  exit 1
fi

pack_dir="${NUCL_VELOPACK_PACK_DIR:-Nucleotide.app}"
output_dir="${NUCL_VELOPACK_OUTPUT_DIR:-target/release/bundle/velopack}"
pack_id="${NUCL_VELOPACK_PACK_ID:-org.spiralpoint.nucleotide.macos}"
channel="${NUCL_VELOPACK_CHANNEL:-macos}"
title="${NUCL_VELOPACK_TITLE:-Nucleotide}"
main_exe="${NUCL_VELOPACK_MAIN_EXE:-Nucleotide}"
icon="${NUCL_VELOPACK_ICON:-crates/nucleotide/assets/nucleotide.icns}"
runtime="${NUCL_VELOPACK_RUNTIME:-}"
release_notes="${NUCL_VELOPACK_RELEASE_NOTES:-}"
sign_app_identity="${NUCL_VELOPACK_SIGN_APP_IDENTITY:-}"
sign_install_identity="${NUCL_VELOPACK_SIGN_INSTALL_IDENTITY:-}"
notary_profile="${NUCL_VELOPACK_NOTARY_PROFILE:-}"
keychain="${NUCL_VELOPACK_KEYCHAIN:-}"

if ! command -v vpk >/dev/null 2>&1; then
  echo "vpk was not found. Install it with: dotnet tool update -g vpk" >&2
  exit 1
fi

if [ ! -d "${pack_dir}" ]; then
  echo "Velopack pack directory not found: ${pack_dir}" >&2
  exit 1
fi

if [ "$(uname -s)" = "Darwin" ]; then
  apple_tool_shim_dir="$(mktemp -d "${TMPDIR:-/tmp}/nucleotide-apple-tools.XXXXXX")"
  trap 'rm -rf "${apple_tool_shim_dir}"' EXIT

  for tool in plutil pkgbuild productbuild xcrun codesign security; do
    if [ -x "/usr/bin/${tool}" ]; then
      {
        printf '#!/usr/bin/env bash\n'
        printf 'exec /usr/bin/%s "$@"\n' "${tool}"
      } >"${apple_tool_shim_dir}/${tool}"
      chmod +x "${apple_tool_shim_dir}/${tool}"
    fi
  done

  export PATH="${apple_tool_shim_dir}:${PATH}"
fi

mkdir -p "${output_dir}"

args=(
  pack
  --packId "${pack_id}"
  --packTitle "${title}"
  --packVersion "${version}"
  --packDir "${pack_dir}"
  --mainExe "${main_exe}"
  --outputDir "${output_dir}"
  --channel "${channel}"
)

if [ -n "${icon}" ]; then
  args+=(--icon "${icon}")
fi

if [ -n "${runtime}" ]; then
  args+=(--runtime "${runtime}")
fi

if [ -n "${release_notes}" ]; then
  args+=(--releaseNotes "${release_notes}")
fi

if [ -n "${sign_app_identity}" ]; then
  args+=(--signAppIdentity "${sign_app_identity}")
fi

if [ -n "${sign_install_identity}" ]; then
  args+=(--signInstallIdentity "${sign_install_identity}")
fi

if [ -n "${notary_profile}" ]; then
  args+=(--notaryProfile "${notary_profile}")
fi

if [ -n "${keychain}" ]; then
  args+=(--keychain "${keychain}")
fi

vpk "${args[@]}"

echo "Velopack package files written to ${output_dir}"
