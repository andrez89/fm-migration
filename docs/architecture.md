# Architecture

`fmblueprint` currently uses one Cargo binary with three implementation modules:

- `components.rs` streams `FMSaveAsXML`, preserves source/action records and attached records, and redacts credential-like values.
- `analysis.rs` builds symbols, typed evidence edges, script findings, calculation candidates, workflow candidates, and relationship join edges.
- `main.rs` owns the CLI, per-file inventory, Markdown/JSON/JSONL reports, schemas, migration artifacts, agent context, and bundle validation.

The pipeline is `XML inputs → normalized component records → deterministic analysis edges/findings → stable bundle`. All generated views consume the normalized records and typed edges; report code does not parse source XML a second time. Add/Modify/Remove records remain separate. A unique cross-file UUID match is heuristic because supplied files are not proof of FileMaker external-source identity.

HTML rendering is intentionally a later stage. It should read `manifest.json`, versioned schemas, `objects.jsonl`, graph JSON, workflows, diagnostics, and local Markdown. It must not invent additional analysis or load remote assets.
