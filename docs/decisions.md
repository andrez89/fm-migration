# Analysis decisions

- The analyzer is offline and rule-based; it does not invoke AI, FileMaker, SQL, plugins, URLs, JavaScript, or scripts.
- Save-a-Copy XML source actions remain separate. The tool does not merge partial ModifyAction records into a guessed final state.
- Structured UUID references are preferred. A unique cross-file UUID match is a `heuristic_candidate` until file identity and external-source relationships are confirmed.
- Calculation text matches are heuristic. The analyzer does not parse or evaluate FileMaker formulas and does not resolve table-occurrence aliases from bare qualified names.
- Workflow reports describe reachable static candidates, not confirmed user journeys or business requirements.
- Permission reports transcribe source-action rules; they do not claim effective runtime authorization.
- Credentials are redacted by default. Account names and assigned roles are retained as requested.
- JSON output schema versions describe the generated contract; source export versions remain separate metadata.
- HTML must consume the stable bundle and keep extracted web content inert.
