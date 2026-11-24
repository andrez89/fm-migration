# Format support

## Supported input

- Claris FileMaker Save a Copy as XML (`FMSaveAsXML`) only.
- UTF-8 and UTF-16 XML decoding.
- One or multiple XML files, or a directory of XML files.
- Observed catalog/action entries and unknown ID-bearing catalog entries are preserved with source provenance.
- Missing `DDR_INFO`, catalogs, and unresolved references are reported as coverage limits or diagnostics.
- `render` produces a local interactive HTML viewer from a validated bundle; it adds no remote assets and does not execute FileMaker behavior.

The smallest supplied real export uses format 2.2.2.0, FileMaker 21.0.1, and UTF-16 LE. Other local XML exports are used for compatibility checks; their different source identities do not by themselves establish a linked multi-file application.

## Unsupported or not established

- Legacy DDR XML/HTML is not the same grammar and is not accepted.
- Version-by-version coverage has not been established for every FileMaker release.
- Catalog completeness cannot be inferred from absence alone; exports may omit empty or excluded catalogs.
- Binary content is not interpreted or executed.
- Script and calculation runtime results, effective access after partial actions, and live endpoint behavior are outside static proof.

For a DDR comparison, obtain paired exports from the same file/version and add a distinct adapter and fixtures only if measured coverage shows value.
