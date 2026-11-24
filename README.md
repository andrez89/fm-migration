# FileMaker Migration Blueprint

`fmblueprint` is an offline Rust command-line tool that analyzes Claris FileMaker **Save a Copy as XML** exports and builds a navigable migration blueprint. It preserves source-action history and records evidence and uncertainty instead of claiming runtime behavior the XML cannot prove.

## What it produces

- Markdown documentation and versioned JSON/JSONL inventories.
- Typed relationship, navigation, script, and workflow analysis.
- Coverage reports, diagnostics, migration notes, and agent context files.
- An optional offline HTML viewer with searchable objects, relationship diagrams, and workflow diagrams.

The tool does not execute FileMaker scripts, connect to services, or require network access at runtime. DDR XML is a separate format and is not accepted as `FMSaveAsXML`.

## Quick start

Install stable Rust with Edition 2024 support, then build and analyze an export:

```sh
cargo build --release
cargo run --release -- analyze /path/to/export.xml --out /tmp/fmblueprint
cargo run --release -- validate /tmp/fmblueprint
cargo run --release -- render /tmp/fmblueprint
```

For one input file, `analyze` defaults to `output/<filename-stem>`. In a terminal, it asks whether to generate HTML after the bundle is written; press Enter to skip or answer `y` to opt in. Non-interactive runs skip HTML; use `render` explicitly when needed.

## Commands

| Command | Purpose |
|---|---|
| `fmblueprint analyze <INPUT...> [--out <DIR>]` | Analyze XML files or a directory of XML files. |
| `fmblueprint validate <BUNDLE>` | Check the bundle manifest, JSON/JSONL files, Markdown links, and references. |
| `fmblueprint context <BUNDLE> --id <ID>` | Print bounded context for an object or workflow. |
| `fmblueprint render <BUNDLE> [--out <DIR>]` | Generate the local HTML viewer. |

Run `cargo run -- --help` or `cargo run -- <COMMAND> --help` for all options. See [the CLI guide](docs/cli.md) for details.

## Data handling and limits

Real XML exports and generated bundles can contain confidential design and identity data. They are not included in this repository; keep them outside Git and protect generated output accordingly. Account names and roles are retained where present; passwords and credentials are redacted. Analysis is static: workflow diagrams show evidence-backed candidate links, not execution order or verified application behavior.

For format coverage, architecture, and remaining analysis gaps, see [format support](docs/format-support.md), [architecture](docs/architecture.md), and the [implementation plan](docs/implementation-plan.md).

## Project layout

- `src/components.rs` parses XML and preserves component records.
- `src/analysis.rs` resolves references and derives static findings and typed edges.
- `src/html.rs` renders the local documentation viewer.
- `src/main.rs` wires the CLI, bundle generation, and validation.
- `schemas/` contains the versioned artifact contracts; `docs/` contains the [project objectives](docs/filemaker_rust_migration_tool_prompt.md) and implementation notes.

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

## Releases

Pushing a version tag such as `v0.1.1` runs the release workflow. The tag must match `package.version` in `Cargo.toml`; the workflow tests and builds Linux, macOS (Apple Silicon and Intel), and Windows binaries, then creates a GitHub Release with downloadable archives. Bump `Cargo.toml` and update `CHANGELOG.md` before tagging.

## License and author

Released under the [MIT License](LICENSE). Copyright © 2026 Andrez. Contact: [andre@amovez.it](mailto:andre@amovez.it).
