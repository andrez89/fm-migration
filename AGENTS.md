# Repository Guidelines

## Project Structure & Module Organization

- `src/main.rs` contains the `fmblueprint` CLI and output orchestration; `src/components.rs` streams source/action records and attached XML details; `src/analysis.rs` builds conservative script findings and typed edges. Keep tests focused in those modules.
- `docs/filemaker_rust_migration_tool_prompt.md` is the product requirements source. `docs/implementation-plan.md` tracks completed and future work; keep it current when phase status changes.
- `samples/` contains real FileMaker XML exports for local analysis. Treat these as potentially sensitive and do not add new exports or generated output to Git without explicit approval.
- The CLI writes inventory, schema-v2 `objects.json`, `edges.json`, and Markdown pages to `output/<input-stem>` by default; `--out` overrides the destination. Component pages are grouped under `objects/<kind>/`; the root `output/` directory is Git-ignored.

## Build, Test, and Development Commands

- `cargo build` compiles the CLI; `cargo build --release` creates an optimized binary.
- `cargo test` runs unit tests, including encoding, malformed XML, redaction, and deterministic output checks.
- `cargo fmt --check` verifies formatting. Run `cargo fmt` to apply formatting.
- `cargo clippy --all-targets -- -D warnings` checks all targets and treats warnings as errors.
- `cargo run -- analyze samples/sample-small.xml` analyzes the smallest supplied sample and writes to `output/sample-small/`. Use `--out /tmp/fmblueprint-output` to choose another location.

## Coding Style & Naming Conventions

Use standard Rust formatting (4-space indentation). Name modules, functions, and local values with `snake_case`; types and enums with `PascalCase`; constants with `SCREAMING_SNAKE_CASE`. Keep XML parsing streaming and deterministic. Include account names and privilege-set roles, but redact passwords and credentials. Preserve unknown catalogs as diagnostics instead of guessing their meaning. The tool must work offline and must not use AI at runtime.

## Testing Guidelines

Add focused unit tests alongside the code in `src/main.rs` (`#[cfg(test)] mod tests`). Name tests for observable behavior, such as `decodes_utf16_xml_with_a_bom`. Use small synthetic fixtures for routine cases; use real samples only for local end-to-end checks. Run `cargo test`, `cargo fmt --check`, and Clippy before submitting parser or output changes.

## Commit & Pull Request Guidelines

This repository has no commit history yet, so no established commit convention can be inferred. Use concise Conventional Commit subjects (for example, `feat: inventory XML catalogs`). Pull requests should summarize behavior changes, list verification commands and results, note format limitations, and confirm generated artifacts or sensitive XML were not added unintentionally. Screenshots are unnecessary for this CLI.

## Architecture & Compatibility

The current flow is XML input → version and catalog inventory → action-aware component records → attached detail extraction → deterministic script analysis → JSON and Markdown output. Preserve Add/Modify/Remove provenance; do not merge partial edits into a final state. Support only behavior justified by `FMSaveAsXML` evidence; do not execute scripts or treat legacy DDR XML as the same format.
