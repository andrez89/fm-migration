# Development Prompt: FileMaker XML to Migration Blueprint (Rust)

## Your role and mandate

You are the principal Rust engineer and software-analysis architect responsible for **designing, implementing, testing, and documenting** a production-quality open-source command-line application. The working name is **`fmblueprint`**.

The tool accepts one or more **Claris FileMaker "Save a Copy as XML" exports** and generates exhaustive, navigable, framework-neutral documentation of the application. The result must be useful both to humans and to **coding agents tasked with reimplementing the application in another technology stack**.

**Implement the software, not merely a plan or proof of concept.** Work incrementally in small, testable vertical slices, make progress autonomously, and keep the repository continuously buildable. Where requirements are technically underdetermined, choose a defensible approach, record the assumption, and proceed. Never invent FileMaker behavior or claim certainty unsupported by XML evidence.

### Non-negotiable constraint: no AI at runtime

The CLI must operate **fully offline, without LLMs, embeddings, cloud APIs, semantic model downloads, prompt generation services, or any other AI inference**. Use deterministic parsing, static analysis, symbol resolution, graph algorithms, rule-based heuristics, and template-driven text generation. The generated output may be consumed *later* by an independent coding agent, but this tool must neither call nor require such an agent.

If a conclusion cannot be established statically, explain the limitation, preserve the evidence, and classify the issue as **unresolved**. A conservative, truthful analyzer is preferable to plausible but fabricated requirements.

---

## 1. Product objective

Given an XML export of a FileMaker application, reconstruct as much of its **structure, functionality, navigation, dependencies, data model, access policy, and likely business workflows** as can be justified by static analysis. Generate:

1. **One Markdown file per documented component**, containing its complete available properties, readable behavior, outgoing and incoming links, and source evidence.
2. **High-level documentation** of architecture, relational model, screen navigation, script execution, data flows, integrations, security, and inferred workflows.
3. **Interactive, offline HTML documentation** with searchable pages and clickable graphs for database relationships, script calls, screens/navigation, and workflow/data dependencies.
4. **Agent-friendly machine-readable artifacts** with a stable, versioned schema and normalized component IDs, so an agent can retrieve application facts without reparsing raw XML.
5. **A migration blueprint**: framework-neutral specifications, screen and data contracts, operational semantics, migration considerations, uncertainties, blockers, and candidate validation scenarios.
6. **An auditable analysis report** describing coverage, unresolved references, unsupported XML features, missing exports, risky dynamic behavior, and provenance of each claim.

**Out of scope:** executing a FileMaker application, converting data records, guaranteeing pixel-perfect layout reproduction, decrypting protected values, automatically generating a complete replacement application, or claiming access to business intent that is absent from the export.

The central success criterion is not “lots of text.” It is that a third-party developer can answer **what exists, what calls what, what a user can do, which data and permissions apply, what is known, and what still requires discovery**, without opening FileMaker or manually reverse-engineering the XML.

---

## 2. Source format, fidelity, and compatibility

### Primary input

Support actual **FileMaker Save a Copy as XML** exports (`FMSaveAsXML`), including at minimum:

- A single comprehensive XML file.
- Several XML exports representing a multi-file application.
- Catalog-split exports, including detection of whether expected catalogs are absent.
- XML with and without **“Include details for analysis tools”**, including `DDR_INFO` when present.
- UTF-8 and legacy UTF-16 XML encodings, as encountered in real exports.
- Version differences. Record source FileMaker version, export format version, file UUID, file name, locale, and available catalogs.
- Binary resource references and optionally embedded binary data; do not load large blobs unnecessarily.

The XML schema is **not guaranteed stable between FileMaker releases**. Build explicit format detection and version-aware interpretation. Handle unknown elements/attributes gracefully, preserve useful source metadata, and report gaps rather than silently discarding them. Do not assume all catalogs have the same shape across versions.

**Do not confuse `Save a Copy as XML` with the older Database Design Report (DDR) XML format.** DDR is a separate format, not required for the first supported release. If you implement DDR later, introduce an explicit input adapter and separate tests; do not pretend its structure is identical.

### Authoritative implementation references

Read and verify these sources before coding format-specific assumptions:

- Claris: https://help.claris.com/en/pro-help/content/documenting-schemas.html
- Claris: https://help.claris.com/en/pro-help/content/save-a-copy-as-xml.html
- Claris FileMaker release notes: https://help.claris.com/en/pro-release-notes/content/index.html
- Community structure examples (informative only; verify against fixtures):
  - https://github.com/marcel-more/fm-lab/blob/main/docs/agents/xml-schema.md
  - https://github.com/marcel-more/fm-lab/blob/main/docs/agents/xml-schema-extended.md

Inspect existing projects such as `fmsonar` and Clockwork Inspector for feature comparison, but **do not copy code with incompatible licensing, do not assume their interpretations are correct, and do not make this project a shallow wrapper around them**.

If web access is unavailable to the development agent, proceed from locally supplied fixtures and documented assumptions. Never hard-code invented XML structures merely to make tests pass.

### Data limitations

A FileMaker design export does **not contain normal record data**, and sensitive account credentials may be encrypted or unavailable. Explicitly distinguish design metadata from runtime data, derived facts from hypotheses, and known policies from authenticated runtime behavior.

---

## 3. Rust implementation requirements

- Stable Rust toolchain; current supported edition; cross-platform binaries for Linux, macOS, and Windows.
- A modular Cargo workspace, or a comparably clear module architecture, separating **input, parser, normalized model, reference resolver, analyzers, renderers, and CLI**.
- Prefer established crates where useful: `clap`, `serde`, `serde_json`, `quick-xml` (or a demonstrably suitable XML alternative), `petgraph`, `thiserror`/`anyhow`, `tracing`, `sha2`, and a deterministic Markdown/HTML templating solution. Choose exact dependencies based on maintainability and verified capabilities.
- Use streaming or bounded-memory processing for large XML documents. Avoid building multiple full in-memory copies of binary-heavy exports. If a document-tree representation is needed for a subtree, bound its lifetime.
- Explicit, typed intermediate representation (IR) with strong stable IDs and provenance; avoid pervasive loosely typed JSON blobs as the sole internal model.
- Output must be stable and deterministic: same inputs + same CLI version/options produce byte-for-byte equivalent generated artifacts, excluding explicitly optional timestamps.
- No dependency on a running FileMaker installation, database server, GUI, browser service, Node installation, or an internet connection at analysis or viewing time.
- Vendor/bundle any required static HTML/CSS/JS assets. **No CDNs or externally hosted graph libraries**.
- Secure treatment of malformed/untrusted XML and embedded strings, with graceful error messages and resource limits.
- Licensable and redistributable build; record third-party licenses.

### Suggested architecture

```text
fmblueprint/
  crates/
    fmblueprint-cli/        # Clap commands, CLI UX, orchestration
    fmblueprint-xml/        # Encodings, XML streaming, catalogs, version adapters
    fmblueprint-model/      # Versioned IR, IDs, source locations, serialization
    fmblueprint-resolve/    # Symbol table, file/TO references, graph construction
    fmblueprint-analyze/    # Calculations, scripts, layouts, navigation, permissions
    fmblueprint-render/     # Markdown, JSON/JSONL, graph data, static HTML
  tests/fixtures/          # Synthetic and permitted real XML fixtures
  docs/                    # Architecture, supported formats, developer docs
  schemas/                 # JSON Schema for stable output contracts
```

You may simplify workspace boundaries if they create excessive overhead, but keep the boundaries testable and explicit.

---

## 4. Intermediate representation and traceability

**Build the normalized IR first.** Renderers and analyzers must depend on it rather than parsing XML independently.

Every object should carry as applicable:

- Stable globally unique identity scoped to the source file, preferably based on actual XML UUID/ID; deterministic fallback when absent.
- Object kind, display name, original name, FileMaker numeric ID/UUID, and source file.
- Parent/child relationships and FileMaker **table occurrence (TO) context**, distinct from the base table.
- Structured, typed properties; unrecognized raw attributes stored in a safe extension field.
- Source span or usable source locator (file + XML path + node identifier and, where feasible, line/column).
- Outbound and inbound typed references.
- Diagnostic list and unsupported-feature markers.
- Confidence/epistemic status: `explicit`, `derived`, `heuristic`, `unresolved` (do not use fabricated numeric confidence scores).

Define typed edges such as:

`contains`, `based_on`, `references_field`, `uses_table_occurrence`, `joins_to`, `calls_script`, `navigates_to`, `is_triggered_by`, `writes_field`, `reads_field`, `runs_calculation`, `uses_value_list`, `uses_custom_function`, `uses_privilege_set`, `accesses_external_source`, `imports_from`, `exports_to`, `opens_url`, and `unresolved_reference`.

Provide reverse indexes and graph traversal APIs. An object must be addressable consistently across Markdown, graph nodes, JSON, reports, and links.

### Provenance contract

Every nontrivial assertion about inferred behavior must include an evidence record:

```json
{
  "status": "derived",
  "claim": "The button can navigate to the Invoice layout",
  "evidence": [
    { "object_id": "...", "source": "...", "xml_path": "..." }
  ],
  "rule_id": "navigation.static-go-to-layout.v1",
  "limitations": []
}
```

This is a **schema illustration**, not a substitute for defining and validating the real schema. Distinguish actual findings from inferred effects. Preserve **why** a workflow edge was created, not only the edge itself.

---

## 5. Required component coverage

Extract and document all discoverable component kinds, including:

### 5.1 File/application level

File names, UUIDs, export versions/locales, startup/default layout/script references, file options, global triggers, file-access authorizations, external dependencies, and catalog completeness. For multi-file applications, model each file separately and show cross-file references.

### 5.2 Data model

- Base tables; external SQL sources where identifiable; table occurrences; aliases.
- Every field: type, repetition, storage, indexing, calculation, summary, global, container, auto-entry, default, validation, formatting, comments, internal IDs, and available options.
- Calculation fields and stored/unstored distinctions where visible.
- Relationships: each TO pair, join predicates and operators, multiple predicates, sort conditions, options to create/delete related records, cascading behavior, and context.
- Relationship graph connectivity, paths, cycles, disconnected groups, and inferred join cardinality **only if supported by evidence**. Never silently label a relationship `1:N` from a field name alone.
- Value lists: source, static/dynamic values, display vs stored values, referenced TOs, and dependencies.
- Data dictionary plus optional **conceptual domain model**, explicitly separated from the literal FileMaker schema.

### 5.3 Scripts and control flow

- All scripts, folder hierarchy, comments, parameters where statically discoverable, result handling, privileges/execution metadata, and every script step in order.
- Preserve step arguments and calculations; represent both human-readable and structured forms.
- Parse structured control flow: `If`, `Else If`, `Else`, `End If`, `Loop`, `Exit Loop If`, `End Loop`, `Exit Script`, `Halt Script`, error handling and other supported branching behavior.
- Construct a **control-flow graph (CFG)** per script, handling branches, loops, early returns, unreachable blocks, and script calls.
- Script call graph (direct calls, dynamic calls when discoverable, cross-file scripts, recursion/cycles, missing calls).
- Identify deterministic step effects such as navigation, field writes, variable assignments, record creation/deletion, finds, sorts, imports, exports, transactions, SQL, external HTTP, and script scheduling; mark context dependence.
- Model FileMaker variables (`$`, `$$`) separately, their scope, reads/writes, and static parameter flow when possible.
- Recognize server-side versus client-side behavior, script triggers, scheduled/on-timer behavior, WebDirect/platform distinctions where represented.
- **Do not execute scripts.** Do not pretend to know runtime values of variables, global fields, found sets, sort states, current records, or window state.

### 5.4 Calculations and custom functions

- Every custom function, parameters, body, recursion, references, and call graph.
- Extract calculations from all relevant places: fields, validation, privilege rules, conditional formatting, hide conditions, scripts, tooltips, value lists, etc.
- Implement a conservative tokenizer/partial parser or AST extractor sufficient for function references, field/TO references, literals, operators, nested calls, and relevant special syntax.
- Use explicit `DDR_INFO` reference chunks where available; treat their presence as additional evidence, not the only path.
- When a calculation is not fully parseable, preserve the exact available source and safely extract only justified references; do not invent semantics.
- Detect known static-analysis obstacles: `Evaluate`, `GetField`, `GetFieldName`, `GetAsNumber` in dynamic contexts, `ExecuteSQL` with dynamically constructed SQL, `Let`, variable indirection, `While`, plug-in functions, custom functions with external dependencies, etc. Treat function behavior contextually rather than assuming every use is dynamic.
- Build dependency cycles and identify external/plugin or unsupported function calls.

### 5.5 Layouts and UI

- Layout metadata, based-on TO, view type, theme, parts, dimensions, and triggers.
- Every discoverable layout object: type, ID/name, bounding box, hierarchy, tab/slide/popover panel, repetition, display conditions, related field/TO, formatting, validation UI, data entry control, button actions, object-specific script triggers, tooltip, conditional formatting, and other properties available in XML.
- Portals, portal filters, sorting and rows, related TO, and context-sensitive interactions.
- Button bars, buttons, menus, custom menu sets, navigation targets, value list controls, web viewers, charts, and possible embedded resources.
- Optionally render an **approximate layout wireframe**, clearly labeled non-pixel-perfect. If the XML cannot support a particular visual feature, preserve its definition in documentation rather than simulating it inaccurately.
- Never execute HTML/JavaScript extracted from Web Viewer definitions; render as escaped source or inert text only.

### 5.6 Security and authorization

- Accounts and privilege set metadata to the extent disclosed by the export, extended privileges, authentication settings, available file-access rules.
- Record-, field-, script-, layout-, value-list- and menu-level privileges, including calculations or custom rules.
- Cross-reference privilege effects against workflows and script execution (without claiming that static privilege data proves actual runtime access).
- Explicitly report encrypted/absent credentials and data-dependent privilege expressions.
- Produce a human-readable role/permission matrix and machine-readable equivalent.
- Mask secrets and tokens exposed incidentally in XML by default. Never display decrypted password claims.

### 5.7 External integrations and resources

- External FileMaker files and data sources, ESS/ODBC hints, `ExecuteSQL`, web service calls, URLs, email, exports/imports, plug-ins, external scripts, JavaScript interactions, server-side operations, and API-related constructs where statically visible.
- Derive **possible** integration endpoints, formats, and dependencies from literal strings/configuration; classify dynamically constructed endpoints as unresolved.
- Show read/write direction and associated scripts/fields when justified.
- Identify resources and binary blobs by metadata without embedding unsafe executable content.

If an XML export includes additional catalogs or component families not listed here, document them as structured components or list them as unsupported, with counts and source pointers. Do not silently drop catalogs.

---

## 6. Analysis passes (deterministic)

Implement discrete analysis passes; make each independently testable and record its findings:

1. **Inventory:** format/version detection, files/catalogs, total object counts, missing catalogs, parse diagnostics.
2. **Normalization:** import objects and stable IDs without loss of identity; preserve original naming and hierarchy.
3. **Reference resolution:** connect IDs/UUIDs/names where justified; rank evidence and retain ambiguity. Resolve cross-file references when exports are supplied, otherwise create external stubs.
4. **Calculation analysis:** extract symbols and dependencies conservatively; detect unresolved dynamic access.
5. **Script semantics:** CFG and call graph; annotate known effects and side effects. Create summary views with field read/write sets, navigation targets, and transitive call dependencies.
6. **UI navigation:** derive edges from buttons, menus, layout triggers, script calls, and explicit navigation steps. Keep context-sensitive alternatives distinct.
7. **Permission analysis:** annotate accessible actions per privilege set where statically determinable; surface rule-dependent cases.
8. **Workflow inference:** identify candidate entry points and navigation/call-flow clusters, then build explainable user journeys.
9. **Migration assessment:** emit deterministic checklists, data contracts, screen specifications, risk lists, and test-case seeds.
10. **Coverage audit:** summarize parsed vs unsupported vs uncertain constructs; validate all links and outputs.

### Important FileMaker semantic rules

- Distinguish **base tables** from **table occurrences** and **layout context**; FileMaker relationships depend on context.
- Distinguish object IDs from display names, which can collide or change.
- `Go to Related Record` requires source context and relationship semantics; do not equate it to an ordinary hyperlink.
- A script may run in multiple contexts; separate its *intrinsic effects* from effects that depend on its entry-point layout/window/script parameter.
- FileMaker navigation may be triggered by buttons, menu items, layout/object triggers, startup scripts, scripts run by other scripts, and timer events.
- Globals, found sets, omitted records, windows, relationship predicates, privileges, transactions, commit behavior, and script errors affect actual behavior. Model them as relevant state when needed, not as static facts.
- Executing a script on server changes execution context; do not merge server and UI navigation semantics.
- Do not infer domain meaning solely from names. “Invoice” in a layout name is a label, not proof of invoice business rules.

### Bounded static interpretation (stretch goal)

Where helpful, build a **non-executing abstract interpreter** that tracks finite symbolic facts (literal values, known layout, possible table occurrences, variable constants, possible script destinations, and branch predicates). Use conservative joins and widening to ensure termination. Never execute external calls, scripts, calculated SQL, plugins, or arbitrary expression code.

Prioritize exact static references before speculative analyses. If an edge is derived through several steps, retain its full evidence chain. Prevent workflow explosion by bounding path depth and displaying summarized cycles/branches.

---

## 7. Workflow reconstruction

A workflow is a **candidate path through UI entry points, scripts, branches, and persistent state changes**, not merely a connected component of a graph.

Create a rule-based workflow discovery algorithm:

- Identify entry points: startup layout, visible/home layouts, menus, explicit navigational buttons, layout/script triggers, public or externally invoked scripts where evidence exists.
- Follow navigation and call edges while preserving branches, recursion/cycles, and unresolved dynamic targets.
- Group related paths based on shared screens, actions, effects, and reachability; avoid pretending that graph clusters are verified business processes.
- Infer deterministic action categories: create/edit/delete/find/list/export/import/approve-like transition **only where actual script logic supports it**. A label like “approve” can be a naming hint, not a verified state transition.
- For each candidate workflow, enumerate entry points, actors/privilege constraints, layouts visited, scripts executed, records/fields read and written, decision points, exceptions, external side effects, and exit states.
- Generate one or more **state-transition diagrams** when a finite state field and its transitions can be established. Otherwise generate flow/navigation diagrams, not a fabricated state machine.
- Explicitly list missing runtime assumptions, data-dependent conditions, dynamic calls, and ambiguous branches.
- Emit reproducible, evidence-backed **test scenario seeds** (preconditions, actions, expected static effects, open questions), without pretending to know the actual test data or business expectations.

For each workflow, output a `status` such as `structurally_derived`, `heuristic_candidate`, or `partially_unresolved`. Do not introduce business-process titles or narratives as if validated by users.

---

## 8. Documentation deliverables

The output should resemble the following (exact organization may evolve, but all capabilities must remain):

```text
output/
  README.md
  manifest.json
  inventory.json
  coverage.json
  diagnostics.json
  application.json
  objects.jsonl
  edges.jsonl
  schemas/
    application.schema.json
    object.schema.json
    edge.schema.json
    workflow.schema.json
  architecture/
    overview.md
    entry-points.md
    dependency-map.md
    data-flows.md
  files/
    <source-file-id>.md
  data/
    tables/<id>.md
    fields/<id>.md
    table-occurrences/<id>.md
    relationships/<id>.md
    value-lists/<id>.md
    data-dictionary.md
  scripts/<id>.md
  custom-functions/<id>.md
  layouts/<id>.md
  ui-objects/<id>.md
  security/
    overview.md
    privilege-sets/<id>.md
    permission-matrix.md
  integrations/
    overview.md
    <id>.md
  workflows/
    index.md
    <id>.md
  migration/
    README.md
    domain-model.md
    feature-inventory.md
    requirements-derived.md
    screens.md
    data-contracts.md
    security-contracts.md
    integration-contracts.md
    testing-scenarios.md
    implementation-order.md
    limitations-and-decisions.md
  agent/
    AGENTS.md
    INDEX.md
    context-map.json
    task-units.json
    requirements.jsonl
    unknowns.jsonl
  graphs/
    relationships.json
    script-calls.json
    navigation.json
    dependencies.json
    workflows.json
  site/
    index.html
    assets/...
  reports/
    unresolved-references.md
    unsupported-features.md
    analysis-quality.md
```

### Per-component Markdown minimum contract

Every documented component page must contain:

1. Title with object type and human name; stable ID.
2. **What this component is** (literal, deterministic description).
3. **Complete available properties**, including values whose semantic meaning is unknown.
4. **Behavior or purpose**, derived from known FileMaker constructs; clearly marked where only a heuristic.
5. **References/dependencies** and **used-by** backreferences, linked to other local pages.
6. Structured script steps or calculation source when relevant, with helpful annotations.
7. Relevant diagrams or graph deep links.
8. **Evidence**: file/version, XML locator, linked source IDs, rule IDs for derived facts.
9. **Migration implications** (data, UI, security, control flow, integration, environmental dependencies).
10. **Uncertainties and unsupported details**, never hidden under generic language.

Create deterministic readable summaries, for example:

> "This script reads `Orders::Status`, assigns `Orders::ApprovedAt`, and calls `LogAudit`. A navigation target depends on `$destination`, which cannot be resolved from the export."

This is an illustrative sentence. Generate similar descriptions only when each clause has supported evidence.

Markdown links must work in a local file tree; avoid fragile name-based URLs. Keep filenames deterministic, case-insensitive-filesystem-safe, collision-safe, and readable.

---

## 9. Agent-facing contract: first-class output

The agent outputs are **not an afterthought**. Design them so a coding agent can reconstruct features incrementally without consuming the entire original XML or blindly trusting prose.

Deliver:

- `agent/AGENTS.md`: how to navigate documentation, how to interpret evidence, where to find component specifications, and the rule to never treat heuristic inference as a verified requirement.
- `agent/context-map.json`: IDs, paths, type indexes, important entry points, and bounded dependency traversal hints.
- `agent/requirements.jsonl`: atomic, typed requirements or behavior assertions, each with ID, category, observed behavior, evidence references, status, dependencies, and unresolved questions.
- `agent/task-units.json`: proposed implementation slices organized by data entities, screens, workflows, and dependencies; **planning hints only**, not claims of complete domain knowledge.
- `agent/unknowns.jsonl`: explicit human/runtime verification tasks with impacted objects/workflows and severity.
- Versioned JSON Schemas with documented compatibility policy.

Example conceptual requirement:

```json
{
  "id": "req:...",
  "kind": "data_mutation",
  "statement": "An action sets Orders::Status to a literal value",
  "status": "derived",
  "source_objects": ["script:...", "field:..."],
  "evidence": ["ev:..."],
  "preconditions": ["A target order record is in context"],
  "unknowns": ["Whether additional business constraints exist"]
}
```

Build a documented mechanism for selecting an object or workflow plus all relevant first-degree dependencies to form a **bounded context package** for a coding agent; e.g. `fmblueprint context --id workflow:<id> --depth 2 --format json`. Respect size limits and include continuation references rather than truncating silently.

**Do not generate target-framework implementation code.** The output should be clear enough to support a later Laravel/Django/.NET/Rust/other migration project without binding the analysis to one framework.

---

## 10. Interactive offline site and graphs

Produce a portable static site under `site/` that can be opened locally or served with an optional built-in local web server. It must not require a running backend or internet connection.

Required features:

- Full-text/local searchable component index; filters by type/source file/status.
- Object detail pages with links to Markdown and related objects.
- Clickable graphs: **ER/TO relationship graph**, script call graph, layout-navigation graph, component dependency graph, candidate workflow/CFG graph.
- Graph node search, zoom/pan, neighbor highlighting, type filters, cycle highlighting, graph-to-document deep links.
- Evidence/uncertainty panel for edges; distinguish explicit, derived, heuristic, unresolved edges visually and accessibly.
- Gracefully handle large graphs with local filtering, collapsing clusters, limited neighborhoods, and on-demand data loading from bundled files where available.
- Accessible and keyboard-usable UI; readable layout on desktop; deterministic static asset generation.
- Escape all user-supplied XML values before putting them in HTML/JS; prevent XSS and path traversal. Embedded Web Viewer scripts or URLs must be displayed inertly, never executed.

You may use a **vendored permissively licensed graph-rendering library** or implement a minimal custom viewer if appropriate. Do not require CDN access. If `file://` browser restrictions prevent loading local JSON, either embed small graph datasets, emit self-contained pages, or provide a documented `fmblueprint serve` option with an explicit local-only bind address.

---

## 11. CLI interface

Use a clear interface such as:

```bash
fmblueprint analyze MyApp.xml --out ./blueprint
fmblueprint analyze ./exports/ --out ./blueprint
fmblueprint validate ./exports/
fmblueprint inspect ./blueprint --id script:<id>
fmblueprint context ./blueprint --id workflow:<id> --depth 2 --format json
fmblueprint serve ./blueprint/site --bind 127.0.0.1:8787
fmblueprint --help
```

Implement the primary `analyze` and `validate` commands first; `inspect`, `context`, and `serve` are important follow-up commands. Additional desirable flags:

- `--strict`: fail with a nonzero exit when a defined class of analysis errors is present; distinguish incomplete analysis from malformed input.
- `--format markdown,json,html` or equivalent selectors; default to generating the full bundle.
- `--no-binary` / resource-size limits.
- `--max-input-size`, `--max-depth`, and other safe controls.
- `--redact` defaults on; explicit opt-in is required for sensitive literal content, with prominent warnings.
- `--verbose`, `--quiet`, `--log-format json` for reproducible automation.
- `--version` and `--help` with examples.

Design exit codes, diagnostics categories, and stdout/stderr behavior. Log analysis metrics and coverage. Do not silently ignore malformed input or overwrite an unrelated output directory.

---

## 12. Security, privacy, and robustness

Treat exported XML as **untrusted input** that may contain confidential business metadata.

- Never fetch external DTDs/entities or files, execute embedded scripts, evaluate calculations, run SQL, or invoke plugin code.
- Guard against XML entity expansion attacks, decompression bombs (if archive support is later added), oversized blobs, cyclic references, pathological recursion, and resource exhaustion.
- Escape Markdown, HTML, JSON, and JS contexts correctly. Sanitize file names and links. Prevent output path traversal.
- Avoid exposing credentials, API keys, bearer tokens, connection strings with passwords, and other sensitive literals. Redact by default while retaining evidence that an integration or secret-bearing setting exists.
- Support a deterministic review mode showing redaction counts without revealing the values.
- Do not leak data through telemetry; **no analytics or network calls**.
- Document sensitive output handling and recommend access controls when documentation includes business logic/security rules.
- Prefer clear, recoverable diagnostics for unsupported versions and corrupt exports.

---

## 13. Testing and acceptance criteria

Create synthetic **valid FileMaker-structured fixtures grounded in inspected real examples**, plus real anonymized/permitted XML fixtures when available. Do not create an imaginary format and then declare support complete. Include small, medium, and larger fixtures; UTF-8/UTF-16, `DDR_INFO` present/absent, split catalogs, absent references, and multi-file scenarios.

Use:

- Unit tests for catalog extraction, IDs, reference resolution, parsing, graph algorithms, redaction, Markdown escaping, and renderers.
- Golden/snapshot tests for Markdown, agent JSON, graph data, and inventory outputs.
- Integration tests invoking the compiled CLI and validating the generated directory tree.
- Property/fuzz tests for malformed XML, cyclical graphs, hostile names/strings, and non-UTF8/encoding edge cases where relevant.
- Determinism tests across repeated runs; validate no unstable iteration order leaks into output.
- JSON Schema validation for agent-facing outputs.
- Link verification across all generated Markdown and static-site pages.
- Tests that verify every unsupported catalog and unresolved reference is reported.
- Negative tests proving that source HTML/JS is not executed and sensitive literals are redacted.
- Performance profiling/benchmarks for representative large exports, with reported methodology rather than arbitrary performance claims.

### Required end-to-end scenarios

1. A basic file with tables, fields, layouts and scripts yields individual pages and correct bidirectional links.
2. Two table occurrences of one base table remain distinct across joins, scripts and layouts.
3. A button triggers a script that navigates to a layout: the generated navigation graph links source button/layout, script and target layout with evidence.
4. A script with conditionals, loop, subscript, and early exit yields an accurate CFG and call graph.
5. A field calculation depends on custom functions/related fields: dependencies resolve correctly, including reverse references.
6. A script with dynamically computed layout or field names leaves an **unresolved** edge rather than inventing a destination.
7. Multi-file export: inter-file references resolve if the target export is present; otherwise external stubs and a report are generated.
8. Missing `DDR_INFO` produces appropriately reduced coverage, not fabricated metadata.
9. Privilege sets generate a matrix; custom data-dependent privilege calculations are flagged as unresolved conditions.
10. A hostile name containing `../`, XML entities, Markdown syntax, or HTML/JS cannot escape output paths or execute in the generated site.
11. The same input twice produces byte-identical stable files; all internal documentation links and schema validations pass.
12. The generated `AGENTS.md` and context package allow a separate reader to trace an inferred requirement to its specific originating FileMaker elements.

### Definition of done

A capability is not “implemented” unless it is:

- demonstrated against a real or structurally authentic fixture;
- covered by automated tests;
- documented;
- integrated into both human-readable and machine-readable output as applicable;
- represented in coverage/diagnostics when unsupported or incomplete.

Do **not** claim 100% format coverage without a measurable inventory and version-specific evidence.

---

## 14. Recommended implementation sequence

Use narrow vertical slices with verifiable deliverables. Maintain an implementation checklist in the repository.

**Milestone 0 — Discovery and baseline**
- Inspect real FileMaker XML structures and version variation.
- Create format/feature matrix, collect legal/anonymized fixtures, define scope of v0.1.
- Record assumptions/risks and document architecture/IR contracts.
- Deliver a compileable Rust CLI skeleton and passing CI/tests.

**Milestone 1 — Parser, version detection, inventory**
- Implement robust XML input, catalog coverage, stable IDs, diagnostics.
- Produce basic inventory JSON and application overview Markdown.
- Show genuine fixture coverage.

**Milestone 2 — Complete structural extraction**
- Implement tables, fields, TOs, relationships, custom functions, scripts, layouts, UI objects, security, value lists, and other discoverable catalogs.
- Generate one page per object with property fidelity and links.
- Emit unknown catalog reports rather than dropping data.

**Milestone 3 — Cross-references and deterministic analysis**
- Build symbol resolver; field/calculation/function dependencies; script calls/CFG; UI trigger and navigation graph; security references.
- Introduce evidence and unresolved-reference audit.

**Milestone 4 — Migration/agent contracts**
- Version JSON Schemas; implement machine-readable outputs, bounded context packages, migration specifications and evidence-backed test seeds.
- Validate provenance and reverse references end-to-end.

**Milestone 5 — Interactive documentation**
- Build self-contained site, search and graph viewer, graph filtering, deep links and accessibility.
- Validate `file://` or local-server operation without internet.

**Milestone 6 — Advanced semantic analysis**
- Improve abstract context interpretation and candidate workflow reconstruction.
- Add findings for complex constructs: GTRR, portals, script parameters, external effects, optional transaction semantics, and special layouts.
- Benchmark and optimize large exports.

**Milestone 7 — Release readiness**
- Security/fuzz tests, real-world fixtures, reproducible releases for major OSes, licenses, CLI reference, and sample documentation bundle.

At every milestone, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (with workspace flags as needed), and `cargo test --workspace`, plus integration tests and schema/link validation when applicable. Report what passed, what remains unsupported, and the next slice.

---

## 15. Required engineering behavior for the coding agent

You are implementing this repository in a multi-turn coding session. Follow these rules:

1. **Start by examining the actual repository and any supplied XML fixtures.** Do not discard existing work.
2. If there is no repository, create one with the planned Rust architecture and a useful README.
3. Write a concise implementation plan and explicit component/format assumptions in repository documentation, **then execute the first implementation slice immediately**. Do not stop after a proposal.
4. Keep tasks small enough for independent review. Finish and test one vertical slice before beginning another.
5. Prefer faithful data extraction and reference resolution over flashy UI or polished speculative prose.
6. Use exact FileMaker IDs/UUIDs and proven references; never use name-only joins when ambiguous.
7. Keep the XML ingestion, analysis rules, normalized model, and output templates independently testable.
8. Use `Result`-based errors, useful CLI diagnostics, and Rust conventions; do not use `unwrap()`/`expect()` on untrusted XML paths.
9. Make unsupported constructs visible, not silently skipped. Add a regression fixture for every format-specific bug.
10. If external documentation is inaccessible, note the limitation explicitly and proceed using authentic local evidence.
11. Preserve all existing passing tests while extending coverage.
12. Never invent performance results, feature support, or successful test runs.
13. Keep `CHANGELOG.md`, `docs/format-support.md`, `docs/architecture.md`, and `docs/decisions.md` current.
14. Where implementation requires tradeoffs, choose the simplest extensible approach, record the decision, and proceed.
15. At the end of each work session, summarize **implemented files/features, exact commands run and outcomes, known gaps, and the next concrete implementation step**.

### Prioritization rule

If time or context becomes limited, deliver a **fully working smaller pipeline** rather than large untested scaffolding:

`real XML fixture -> parser -> stable IR -> resolved references -> documented objects -> valid JSON -> meaningful tests`.

Then expand to CFG, graphs, agent contracts, and workflows. Never substitute mock-only functionality for documented real-format support without marking it incomplete.

---

## 16. Final quality bar

The final CLI should take a substantial FileMaker XML export, create an organized, navigable technical reference and migration blueprint, expose real inter-component relationships, and produce reliable structured context that other coding agents can use to reimplement features in any framework.

The tool's credibility rests on **fidelity, provenance, completeness reporting, and restraint about what static XML can prove**.

**Begin implementation now by inspecting the available repository and fixtures, verifying the FileMaker XML schema with real examples, defining the first IR slice, and creating a running Rust CLI with its first end-to-end test.**
