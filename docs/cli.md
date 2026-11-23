# CLI reference

Build and run the tool with stable Rust:

```sh
cargo run -- analyze samples/sample-small.xml
cargo run -- analyze exports/ --out /tmp/blueprint
cargo run -- validate /tmp/blueprint
cargo run -- context /tmp/blueprint --id 'workflow:<workflow-id>' --depth 2 --max-objects 250
```

`analyze` accepts one or more XML paths or a directory containing XML files. A single file defaults to `output/<filename-stem>`; `--out` selects the output directory. It generates Markdown, JSON/JSONL, graphs, coverage/diagnostic reports, migration notes, agent artifacts, and schemas. It never executes FileMaker behavior.

`validate` checks the generated bundle manifest, listed files, JSON/JSONL syntax, local Markdown links, and object/record references between exported edges and components. It does not currently perform complete JSON Schema validation.

`context` writes a bounded JSON package to stdout for an object or workflow ID. `--depth` controls graph traversal; `--max-objects` caps included components. When truncated, the package includes `continuation_ids`; increase the cap or request a continuation explicitly.

Generated output may contain confidential business design and access rules. Keep it local and apply the same access controls as the source export.
