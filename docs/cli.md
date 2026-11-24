# CLI reference

Build and run the tool with stable Rust:

```sh
cargo run -- analyze path/to/sample.xml
cargo run -- analyze exports/ --out /tmp/blueprint
cargo run -- validate /tmp/blueprint
cargo run -- context /tmp/blueprint --id 'workflow:<workflow-id>' --depth 2 --max-objects 250
cargo run -- render /tmp/blueprint
```

`analyze` accepts one or more XML paths or a directory containing XML files. A single file defaults to `output/<filename-stem>`; `--out` selects the output directory. Immediately after selecting the output directory and before analysis starts, an interactive run asks `Generate interactive HTML after analysis? [y/N]`; pressing Enter skips HTML. Answer `y` to generate it after the bundle is written. Non-interactive runs skip HTML unless rendered separately with `render`. It never executes FileMaker behavior.

`validate` checks the generated bundle manifest, listed files, JSON/JSONL syntax, local Markdown links, and object/record references between exported edges and components. It does not currently perform complete JSON Schema validation.

`context` writes a bounded JSON package to stdout for an object or workflow ID. `--depth` controls graph traversal; `--max-objects` caps included components. When truncated, the package includes `continuation_ids`; increase the cap or request a continuation explicitly.

`render` validates a bundle and writes an offline interactive HTML site to `<bundle>/html/`; pass `--out <directory>` to choose another destination. Open `index.html` directly in a browser. The site supports object search and detail pages, table-occurrence relationship diagrams filtered by source and action, searchable workflow candidates with one evidence-backed graph per candidate, diagnostics, and coverage. Diagrams support pointer pan and wheel zoom. It uses local CSS and JavaScript only; exported text is HTML-escaped, and unresolved targets are warnings rather than invented nodes.

Generated output may contain confidential business design and access rules. Keep it local and apply the same access controls as the source export.
