# Vendored tree-sitter

`tree-sitter/` is the crates.io package `tree-sitter` 0.27.0 (upstream commit `6070dbfefd326bd735e5683eb128cc1b57dad0c0`, see
`tree-sitter/.cargo_vcs_info.json`) with one change, `tree-sitter.patch`, applied to `src/stack.c`. The root `Cargo.toml` substitutes it
through `[patch.crates-io]`.

## Why

Upstream `ts_stack_pop_all` returns one slice per path through the parse stack. An unterminated `sql<a><a>…` at end of file merges
the stack once per token, so the final accept allocates quadratic memory in one call that no progress callback can interrupt (2.6 GiB
at 64 KiB; the process is killed at 2 MiB). The patch stops forking in `pop_all` once live iterators plus finished slices reach
`MAX_ITERATOR_COUNT` (64), reusing upstream's own skip-fork branch. Every changed line is marked `code-atlas patch` or is the new
`bound_paths` argument.

## Checking the copy

```sh
diff -r ~/.cargo/registry/src/index.crates.io-*/tree-sitter-0.27.0 vendor/tree-sitter
```

prints only the `.cargo-ok` marker and the patched `src/stack.c` hunks.

## Upgrading tree-sitter

1. Bump `tree-sitter` in `[workspace.dependencies]` and run `cargo fetch` so the new release lands in the registry cache.
2. Replace `vendor/tree-sitter` with that release's registry directory, without `.cargo-ok`, and change the version in this file.
3. Re-apply the patch from inside the copy: `patch -p1 < ../tree-sitter.patch`. If it no longer applies, port it by hand (or drop it if
   upstream has bounded `pop_all`), then regenerate `tree-sitter.patch` with
   `diff -u --label a/src/stack.c --label b/src/stack.c <registry copy>/src/stack.c vendor/tree-sitter/src/stack.c`.
4. Run `./scripts/check.sh`. Cargo only warns when a `[patch.crates-io]` entry no longer matches the requested version and then builds
   upstream silently, so `crates/atlas-lang-ts/tests/parse_memory.rs` is the guard: it fails within a second if the patch is missing.
5. Any change to parse output bumps the analyzers' `ANALYZER_VERSION` (see `CLAUDE.md`).

`cargo install --path crates/atlas-cli` and `cargo install --git <url> atlas-cli` build this workspace and use the patched copy. Only a
crate in another workspace that depends on `atlas-lang-ts` or `atlas-lang-rust` gets upstream tree-sitter.
