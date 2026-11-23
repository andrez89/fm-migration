# `fmblueprint` implementation plan

Live status for the Rust FileMaker XML analysis CLI. Keep generated reports and real exports outside Git.

## Phase 1 — inventory (complete)

| Step | Status | Result |
|---|---|---|
| Inspect objective and sample | Complete | `sample-small.xml` is the smallest supplied `FMSaveAsXML` sample and uses UTF-16 LE with BOM. |
| Establish safe output handling | Complete | Output defaults to Git-ignored `output/<input-stem>`; passwords and credentials are redacted. |
| Parse and inventory | Complete | `fmblueprint analyze [xml] [--out directory]` reports version, file metadata, catalogs by action, diagnostics, and `DDR_INFO`. |
| Write inventory reports | Complete | Generates `inventory.json` and `overview.md`; UTF-8 and UTF-16 parsing are covered. |

## Phase 2 — complete structural records (complete)

| Step | Status | Result |
|---|---|---|
| Preserve source actions | Complete | Add, Modify, and Remove records remain separate; logical `object_id` is stable across actions and each record has a unique `record_id`. Partial modifications are not merged. |
| Extract recognized objects | Complete | Preserves the 17 catalog families observed in the sample, account names/roles, and source properties; binary payloads remain omitted. |
| Attach detached details | Complete | Links all 2,120 script steps in 113 groups, 36 custom-function bodies, and 15 value-list option records by UUID or unique ID. Empty step groups remain represented. |
| Produce component documentation | Complete | `objects.json` schema v2 and per-kind Markdown pages include source properties, attached records, diagnostics, and analysis. |
| Validate the sample | Complete | 1,790 records/pages, including 31 ModifyAction records; no duplicate record IDs, broken Markdown links, unresolved detached records, or password/credential leaks. |

## Phase 3 — script analysis (in progress)

| Step | Status | Result / next action |
|---|---|---|
| Resolve explicit script references | Complete | Emits evidence-backed call edges; UUID is primary, conflicting IDs and missing/ambiguous targets remain unresolved. |
| Analyze ordered steps | Complete | Recognizes basic effects, variable writes/calculation reads, disabled steps, If/Else/Loop control flow, and unreachable-step candidates. Scripts are never executed. |
| Expand script semantics | Partial | Adds caller/callee parameter and result marker findings, resolved call reachability/cycle candidates, and more server/web/transaction/error-mode effects. Expression evaluation and parameter/result matching remain open. |

## In progress and future

- Analysis prerequisites implemented: multiple-input inventory, typed references, relationship join edges, script parameter/result candidates and call summaries, heuristic calculation references, workflow links, and source-action security rows. Broader authentic fixture validation remains required.
- The generated bundle includes versioned JSON/JSONL artifacts and schemas, coverage/diagnostics, graph datasets, migration documents, agent context, plus `validate` and bounded `context` commands. `validate` checks manifest paths, JSON syntax, local Markdown links, and graph references. External JSON Schema validation passed for the sample-small and sample-large-b bundles; it is not yet part of the CLI.
- Offline HTML graphs implemented as `fmblueprint render <bundle> [--out <directory>]`. `relationships.html` draws table occurrences and explicit join predicates, separates source UUIDs and Add/Modify/Remove actions, and filters edges by action. `workflows.html` searches candidates and links to one static, layered SVG per candidate. Nodes link to object details; edge evidence/status and unresolved references remain visible. SVGs pan/zoom; source text is escaped and CSP permits only local assets.
- Remaining analysis/quality work: validate the latest renderer and bundle writer on the large sample-large-a export (the final rerun was stopped); add hostile-text and multi-input regression coverage; make schema contract validation reproducible; expand unsupported-catalog/version reporting. Multi-file identity, calculation aliases and expression parsing, dynamic script behavior, and effective authorization still require evidence. Do not present runtime workflows or effective access as proven.
- sample-small verification should record the object, relationship, and workflow page counts and check every local `href`/`src` target. Browser-based visual and keyboard review remains future work. DDR comparison is optional and remains a separate adapter if later justified.
- DDR support remains an optional decision after paired-export comparison; it must use a separate input adapter.
- Preserve unsupported or ambiguous source constructs as diagnostics; do not infer a final application state from partial action records.
- Sample-specific evidence and ranked gaps are tracked in [sample-analysis-gaps.md](sample-analysis-gaps.md). The current sample has no `DDR_INFO`; compare paired exports before deciding whether a separate DDR adapter adds required coverage.

## Verification

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo run -- validate <bundle>` validates generated manifest paths, JSON/JSONL syntax, Markdown links, and edge endpoints.
- `cargo run -- render <bundle>` writes an offline searchable site to `<bundle>/html/`.
- Local sample check: schema version, action provenance, attached-detail counts, credential redaction, deterministic output, and Markdown links.

## Constraints

- Input is Claris FileMaker **Save a Copy as XML** (`FMSaveAsXML`), not the legacy DDR grammar.
- The real sample stays local and is not a committed test fixture; generated output stays under ignored `output/` or a caller-selected directory.
- Catalog/action inventory describes entries present in the export; it does not claim a complete final application state.
- Analysis is offline and deterministic. It does not execute FileMaker scripts or use AI at runtime.
