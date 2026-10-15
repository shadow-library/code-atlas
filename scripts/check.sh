#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Tests must start catl only through atlas_test_lsp::hermetic::hermetic_catl, which points HOME, CLAUDE_CONFIG_DIR,
# CODEX_HOME and CODE_ATLAS_HOME at the sandbox; a direct spawn once wrote the developer's real Claude Code config.
test_dirs=(crates/atlas-cli/tests crates/atlas-mcp/tests crates/atlas-eval/tests)
unsandboxed=$(
  {
    grep -rnE 'Command::new\(' "${test_dirs[@]}" | grep -vE 'Command::new\("[^"]*"\)|ServerCommand::new|Command::new\(cargo\)|Command::new\(&wrapper\)' || true
    grep -rn 'CARGO_BIN_EXE_catl' "${test_dirs[@]}" | grep -vE ':[0-9]+:[[:space:]]*//' | grep -vE 'hermetic_catl\(env!\("CARGO_BIN_EXE_catl"\)|fs::canonicalize\(env!\("CARGO_BIN_EXE_catl"\)\)' || true
  }
)
if [[ -n "$unsandboxed" ]]; then
  echo "tests must spawn catl through atlas_test_lsp::hermetic::hermetic_catl:" >&2
  echo "$unsandboxed" >&2
  exit 1
fi

./scripts/check-denylist.sh

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
