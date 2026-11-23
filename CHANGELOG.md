# Changelog

## Unreleased

- Add offline SVG diagrams for table relationships and workflow candidates, with evidence, source/action filtering, object links, pan, and zoom.
- Keep generated HTML out of the bundle manifest so repeated analysis stays deterministic.
- Escape all source-derived content and apply a restrictive local-only Content Security Policy.
- Ask before analysis whether to generate HTML afterward; default to no, including non-interactive runs.

## Unreleased

- Added deterministic relationship join edges, calculation/script findings, bounded context export, and bundle validation.
- Added coverage, diagnostic, graph, migration, and agent-facing artifacts with published versioned schemas.
- Added regression tests for script parameter/result candidates, calculation-name dependencies, cross-file UUID candidates, relationship joins, and deterministic bundle validation.
