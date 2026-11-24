# Architecture

`fmblueprint` uses one Cargo binary with a GTK desktop interface and CLI, sharing the analysis pipeline:

- `components.rs` streams `FMSaveAsXML`, preserves source/action records and attached records, and redacts credential-like values.
- `analysis.rs` builds symbols, typed evidence edges, script findings, calculation candidates, workflow candidates, and relationship join edges.
- `html.rs` renders a validated bundle into an offline searchable site with object, relationship, workflow, diagnostic, and coverage views.
- `ui.rs` provides XML and output-folder pickers, HTML selection, background analysis status, and an explicit action to open the result folder.
- `main.rs` owns the CLI, shared analysis entry points, per-file inventory, Markdown/JSON/JSONL reports, schemas, migration artifacts, agent context, and bundle validation.

The pipeline is `XML inputs → normalized component records → deterministic analysis edges/findings → stable bundle`. All generated views consume the normalized records and typed edges; report code does not parse source XML a second time. Add/Modify/Remove records remain separate. A unique cross-file UUID match is heuristic because supplied files are not proof of FileMaker external-source identity.

HTML rendering consumes the validated JSONL and JSON contract; it does not reparse XML or invent analysis. The generated site works from `file://`, loads only local CSS/JavaScript, uses a restrictive Content Security Policy, and escapes all source-derived text before placing it in HTML. The relationship view draws `joins_to` edges between table occurrences and keeps predicates grouped by relationship record and source action. Workflow pages draw only typed edges whose endpoints occur in that candidate; their layout is not runtime ordering. Unresolved references remain warnings. Both SVG views support pan and zoom and link nodes to component detail pages. Generated `html/` files are excluded from the bundle manifest so repeated analysis remains deterministic.
