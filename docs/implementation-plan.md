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
| Expand script semantics | Next | Improve parameter/result flow, direct and dynamic call summaries, recursion/cycles, error handling, and broader step effects with source evidence. |

## In progress and future

- First three coverage slices (initial implementation complete; broader fixture validation remains):
  1. Input: `analyze` accepts multiple XML paths or a directory of XML files, combines inventories and components, reports catalog counts per source file, and retains ID-bearing entries from unknown catalogs as raw objects. Format/version fixtures and split-export completeness rules remain open.
  2. References: explicit reference elements and structured field/custom-function calculation chunks in component properties, attached calculation records, and script steps resolve to same-file objects where identity is unambiguous; unresolved targets remain explicit. Calculation text parsing, cross-file references, and complete relationship semantics remain open.
  3. UI/security: button/menu/layout-trigger script references and account privilege-set references produce typed edges; `security/permission-matrix.md` transcribes privilege-set categories and table-level record rules. Effective access, object-level access rows, and end-to-end validation remain open.
- Next: expand script analysis and validate control-flow and reference edge cases against more fixtures.
- Later: calculation/custom-function dependencies; complete layout/UI navigation; full privilege analysis; external integrations; workflow inference; migration contracts; versioned schemas; bounded agent context packages; and offline HTML documentation.
- DDR support remains an optional decision after paired-export comparison; it must use a separate input adapter.
- Preserve unsupported or ambiguous source constructs as diagnostics; do not infer a final application state from partial action records.
- Sample-specific evidence and ranked gaps are tracked in [sample-analysis-gaps.md](sample-analysis-gaps.md). The current sample has no `DDR_INFO`; compare paired exports before deciding whether a separate DDR adapter adds required coverage.

## Verification

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- Local sample check: schema version, action provenance, attached-detail counts, credential redaction, deterministic output, and Markdown links.

## Constraints

- Input is Claris FileMaker **Save a Copy as XML** (`FMSaveAsXML`), not the legacy DDR grammar.
- The real sample stays local and is not a committed test fixture; generated output stays under ignored `output/` or a caller-selected directory.
- Catalog/action inventory describes entries present in the export; it does not claim a complete final application state.
- Analysis is offline and deterministic. It does not execute FileMaker scripts or use AI at runtime.
