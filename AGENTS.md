# Repository Guidelines

## Project Structure & Module Organization

- `src/main.rs` contains the `fmblueprint` CLI and bundle/report generation; `src/components.rs` streams source/action records and attached XML details; `src/analysis.rs` builds conservative findings, typed edges, and workflow candidates; `src/html.rs` renders validated bundles into a local interactive site. Unit tests live beside these modules.
- `schemas/` defines the versioned JSON/JSONL contracts. `docs/filemaker_rust_migration_tool_prompt.md` is the product requirements source; keep `docs/implementation-plan.md`, `docs/sample-analysis-gaps.md`, `docs/format-support.md`, `docs/architecture.md`, `docs/decisions.md`, and `CHANGELOG.md` current.
- Use synthetic XML in unit tests. Keep real FileMaker exports and generated analysis output outside the repository; they may contain confidential design and identity data.
- The CLI accepts XML files or directories and emits a versioned bundle with coverage, diagnostics, graphs, migration notes, and agent artifacts. A single file defaults to `output/<input-stem>`; `--out` overrides the destination. Component pages are grouped under `objects/<kind>/`; generated output stays outside Git.

## Build, Test, and Development Commands

- `cargo build` compiles the CLI; `cargo build --release` creates an optimized binary.
- `cargo test` runs unit tests, including encoding, malformed XML, redaction, and deterministic output checks.
- `cargo fmt --check` verifies formatting. Run `cargo fmt` to apply formatting.
- `cargo clippy --all-targets -- -D warnings` checks all targets and treats warnings as errors.
- `cargo run -- analyze path/to/sample.xml` analyzes a supplied XML file and writes to `output/<input-stem>/`. Before analysis starts, it asks whether to generate HTML after the bundle (default no). Pass multiple files or a directory to combine exports. Use `--out /tmp/fmblueprint-output` to choose another location.
- `cargo run -- validate output/<input-stem>` checks bundle files, JSON/JSONL syntax, local Markdown links, and edge targets.
- `cargo run -- context output/<input-stem> --id '<object-or-workflow-id>' --depth 2` writes a bounded JSON context package to stdout.
- `cargo run -- render output/<input-stem>` writes an offline interactive site to `output/<input-stem>/html/`; open `index.html` directly.

## Coding Style & Naming Conventions

Use standard Rust formatting (4-space indentation). Name modules, functions, and local values with `snake_case`; types and enums with `PascalCase`; constants with `SCREAMING_SNAKE_CASE`. Keep XML parsing streaming and deterministic. Include account names and privilege-set roles, but redact passwords and credentials. Preserve unknown catalog items as raw components and report their catalog names; do not guess semantics. The tool must work offline and must not use AI at runtime.

## Testing Guidelines

Add focused unit tests alongside the code in `src/main.rs` (`#[cfg(test)] mod tests`). Name tests for observable behavior, such as `decodes_utf16_xml_with_a_bom`. Use small synthetic fixtures for routine cases; use real samples only for local end-to-end checks. Run `cargo test`, `cargo fmt --check`, and Clippy before submitting parser or output changes.

## Commit & Pull Request Guidelines

This repository has no commit history yet, so no established commit convention can be inferred. Use concise Conventional Commit subjects (for example, `feat: inventory XML catalogs`). Pull requests should summarize behavior changes, list verification commands and results, note format limitations, and confirm generated artifacts or sensitive XML were not added unintentionally. Screenshots are unnecessary for this CLI.

## Architecture & Compatibility

The current flow is XML input(s) → per-file version/catalog inventory → action-aware component records → attached detail extraction → deterministic script and explicit component-reference analysis → JSON and Markdown output. Preserve Add/Modify/Remove provenance; do not merge partial edits into a final state. Support only behavior justified by `FMSaveAsXML` evidence; do not execute scripts or treat legacy DDR XML as the same format.
