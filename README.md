# code-atlas

AI coding assistant for large polyglot repositories.

## Install

```bash
uv sync
```

The `code-atlas` CLI entry point is declared in `pyproject.toml` but not wired up until task 024 — invoking it now will fail to import `code_atlas.cli`.

```bash
# code-atlas --help   # available after task 024
```
