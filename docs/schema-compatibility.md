# Machine-readable schema compatibility

Generated JSON and JSONL artifacts carry a `schema_version` where they are document containers. Object, edge, workflow, coverage, and diagnostic records have separate JSON Schemas in `schemas/` and are copied into each generated bundle.

Consumers should reject unsupported major versions and ignore unknown fields only when their schema permits it. Breaking field removals, semantic changes, or status-enum changes require a version increment. Additive optional fields may remain in the current version only when the corresponding schema allows them. JSONL artifacts validate one record per line against the record schema named in `manifest.json`.

The export `schema_version` records the output contract, not the Claris XML version. `inventory.applications[*].export_version` records the source export version separately.
