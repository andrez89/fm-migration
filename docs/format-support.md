# Format support

## Supported input

- Claris FileMaker Save a Copy as XML (`FMSaveAsXML`) only.
- UTF-8 and UTF-16 XML decoding.
- One or multiple XML files, or a directory of XML files.
- Observed catalog/action entries and unknown ID-bearing catalog entries are preserved with source provenance.
- Missing `DDR_INFO`, catalogs, and unresolved references are reported as coverage limits or diagnostics.

The smallest real fixture is `samples/sample-small.xml` (export format 2.2.2.0, FileMaker 21.0.1, UTF-16 LE). Other supplied XML samples are used for local compatibility checks; their different source identities do not by themselves establish a linked multi-file application.

## Unsupported or not established

- Legacy DDR XML/HTML is not the same grammar and is not accepted.
- Version-by-version coverage has not been established for every FileMaker release.
- Catalog completeness cannot be inferred from absence alone; exports may omit empty or excluded catalogs.
- Binary content is not interpreted or executed.
- Script and calculation runtime results, effective access after partial actions, and live endpoint behavior are outside static proof.

For a DDR comparison, obtain paired exports from the same file/version and add a distinct adapter and fixtures only if measured coverage shows value.
