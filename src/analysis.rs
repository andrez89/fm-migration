use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::components::{AnalysisFinding, AttachedRecord, Component, XmlNode};

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisEdge {
    pub kind: String,
    pub status: String,
    pub from_object_id: Option<String>,
    pub to_object_id: Option<String>,
    pub from_record_id: Option<String>,
    pub to_record_id: Option<String>,
    pub evidence: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone)]
struct ObjectSymbol {
    object_id: String,
    record_id: String,
    xml_id: Option<String>,
}

struct Symbols {
    by_uuid: BTreeMap<(String, String, String), Vec<ObjectSymbol>>,
    by_id: BTreeMap<(String, String, String), Vec<ObjectSymbol>>,
}

struct ControlBlocks {
    if_ends: BTreeMap<usize, usize>,
    alternatives: BTreeMap<usize, usize>,
    alternative_parents: BTreeMap<usize, usize>,
    loop_ends: BTreeMap<usize, usize>,
    loop_starts: BTreeMap<usize, usize>,
}

pub fn analyze_scripts(components: &mut [Component]) -> Vec<AnalysisEdge> {
    let mut edges = Vec::new();
    let symbols = build_symbols(components);
    let script_indices: Vec<_> = components
        .iter()
        .enumerate()
        .filter(|(_, component)| component.kind == "script")
        .map(|(index, _)| index)
        .collect();
    for script_index in script_indices {
        let script = &mut components[script_index];
        let steps: Vec<_> = script
            .attached_records
            .iter()
            .filter(|record| record.kind == "script_step")
            .cloned()
            .collect();
        for step in &steps {
            if !step_is_enabled(step) {
                script.findings.push(AnalysisFinding {
                    kind: "disabled_step".to_string(),
                    status: "explicit".to_string(),
                    summary: format!("Step {} ({}) is disabled", step.order, step.name),
                    evidence: vec![step.source_path.clone()],
                    limitations: Vec::new(),
                });
                continue;
            }
            if let Some(effect) = effect_for_step(&step.name) {
                script.findings.push(AnalysisFinding {
                    kind: "step_effect".to_string(),
                    status: "explicit".to_string(),
                    summary: format!("Step {} ({}) indicates {effect}", step.order, step.name),
                    evidence: vec![step.source_path.clone()],
                    limitations: vec![
                        "This is a static classification of the step name and arguments; runtime success is not established.".to_string(),
                    ],
                });
            }
            collect_variable_findings(script, step);
            collect_reference_edges(script, step, &symbols, &mut edges);
        }
        let active_steps: Vec<_> = steps
            .iter()
            .filter(|step| step_is_enabled(step))
            .cloned()
            .collect();
        build_control_flow(script, &active_steps, &mut edges);
        script.findings.sort_by(|left, right| {
            left.evidence
                .cmp(&right.evidence)
                .then(left.kind.cmp(&right.kind))
                .then(left.summary.cmp(&right.summary))
        });
    }
    edges.sort_by(|left, right| {
        left.from_record_id
            .cmp(&right.from_record_id)
            .then(left.kind.cmp(&right.kind))
            .then(left.to_object_id.cmp(&right.to_object_id))
            .then(left.to_record_id.cmp(&right.to_record_id))
            .then(left.evidence.cmp(&right.evidence))
    });
    edges
}

fn collect_reference_edges(
    script: &mut Component,
    step: &AttachedRecord,
    symbols: &Symbols,
    edges: &mut Vec<AnalysisEdge>,
) {
    let mut references = Vec::new();
    collect_references(&step.properties, &mut references);
    let step_type = step_reference_kind(&step.name);
    let has_script_reference = references
        .iter()
        .any(|(node, _)| node.name == "ScriptReference");
    for (node, reference_kind) in references.iter().copied() {
        let kind = if node.name == "FieldReference" && step_type == Some("writes_field") {
            step_type
        } else {
            reference_kind
        };
        let target_kind = reference_component_kind(node.name.as_str());
        let target = target_kind.and_then(|target_kind| {
            resolve_reference(symbols, target_kind, &script.source_file, node)
        });
        let status = if target.is_some() {
            "explicit"
        } else {
            "unresolved"
        };
        let evidence = format!("{} ({})", step.source_path, node.name);
        if target.is_none() {
            script.diagnostics.push(format!(
                "unresolved {} in script step {} ({})",
                node.name, step.order, step.name
            ));
        }
        edges.push(AnalysisEdge {
            kind: kind.unwrap_or("references_object").to_string(),
            status: status.to_string(),
            from_object_id: Some(script.object_id.clone()),
            to_object_id: target.as_ref().map(|symbol| symbol.object_id.clone()),
            from_record_id: Some(step.record_id.clone()),
            to_record_id: target.as_ref().map(|symbol| symbol.record_id.clone()),
            evidence: vec![evidence],
            limitations: if target.is_some() {
                Vec::new()
            } else {
                vec!["The reference target is absent or ambiguous in this source file.".to_string()]
            },
        });
    }
    if step.name == "Perform Script" && !has_script_reference {
        script.findings.push(AnalysisFinding {
            kind: "dynamic_script_call".to_string(),
            status: "unresolved".to_string(),
            summary: format!("Step {} may call a script dynamically", step.order),
            evidence: vec![step.source_path.clone()],
            limitations: vec![
                "No explicit ScriptReference was present; the destination was not evaluated."
                    .to_string(),
            ],
        });
        edges.push(AnalysisEdge {
            kind: "calls_script".to_string(),
            status: "unresolved".to_string(),
            from_object_id: Some(script.object_id.clone()),
            to_object_id: None,
            from_record_id: Some(step.record_id.clone()),
            to_record_id: None,
            evidence: vec![step.source_path.clone()],
            limitations: vec!["Dynamic script targets are not evaluated.".to_string()],
        });
    }
}

fn collect_references<'a>(
    node: &'a XmlNode,
    references: &mut Vec<(&'a XmlNode, Option<&'static str>)>,
) {
    if let Some(kind) = reference_edge_kind(&node.name) {
        references.push((node, Some(kind)));
    }
    for child in &node.children {
        collect_references(child, references);
    }
}

fn reference_edge_kind(name: &str) -> Option<&'static str> {
    match name {
        "ScriptReference" => Some("calls_script"),
        "FieldReference" => Some("reads_field"),
        "TableOccurrenceReference" => Some("uses_table_occurrence"),
        "ValueListReference" => Some("uses_value_list"),
        "CustomFunctionReference" => Some("uses_custom_function"),
        "LayoutReference" => Some("navigates_to"),
        _ => None,
    }
}

fn reference_component_kind(name: &str) -> Option<&'static str> {
    match name {
        "ScriptReference" => Some("script"),
        "FieldReference" => Some("field"),
        "TableOccurrenceReference" => Some("table_occurrence"),
        "ValueListReference" => Some("value_list"),
        "CustomFunctionReference" => Some("custom_function"),
        "LayoutReference" => Some("layout"),
        _ => None,
    }
}

fn step_reference_kind(step_name: &str) -> Option<&'static str> {
    match step_name {
        "Set Field" | "Set Field By Name" => Some("writes_field"),
        _ => None,
    }
}

fn build_symbols(components: &[Component]) -> Symbols {
    let mut symbols = Symbols {
        by_uuid: BTreeMap::new(),
        by_id: BTreeMap::new(),
    };
    for component in components {
        let symbol = ObjectSymbol {
            object_id: component.object_id.clone(),
            record_id: component.record_id.clone(),
            xml_id: component.xml_id.clone(),
        };
        if let Some(uuid) = &component.xml_uuid {
            insert_symbol(
                &mut symbols.by_uuid,
                (
                    component.source_file.clone(),
                    component.kind.clone(),
                    uuid.clone(),
                ),
                symbol.clone(),
                component.source_action == "AddAction",
            );
        }
        if let Some(id) = &component.xml_id {
            insert_symbol(
                &mut symbols.by_id,
                (
                    component.source_file.clone(),
                    component.kind.clone(),
                    id.clone(),
                ),
                symbol,
                component.source_action == "AddAction",
            );
        }
    }
    symbols
}

fn insert_symbol(
    symbols: &mut BTreeMap<(String, String, String), Vec<ObjectSymbol>>,
    key: (String, String, String),
    symbol: ObjectSymbol,
    prefer: bool,
) {
    let entries = symbols.entry(key).or_default();
    if let Some(existing) = entries
        .iter_mut()
        .find(|entry| entry.object_id == symbol.object_id)
    {
        if prefer {
            *existing = symbol;
        }
    } else {
        entries.push(symbol);
    }
}

fn resolve_reference(
    symbols: &Symbols,
    kind: &str,
    source_file: &str,
    reference: &XmlNode,
) -> Option<ObjectSymbol> {
    let uuid = reference.attributes.get("UUID");
    let xml_id = reference.attributes.get("id");
    let matching = if let Some(uuid) = uuid {
        symbols
            .by_uuid
            .get(&(source_file.to_string(), kind.to_string(), uuid.clone()))?
    } else {
        let xml_id = xml_id?;
        symbols
            .by_id
            .get(&(source_file.to_string(), kind.to_string(), xml_id.clone()))?
    };
    if uuid.is_some()
        && (matching.len() != 1
            || xml_id.is_some_and(|id| matching[0].xml_id.as_deref() != Some(id.as_str())))
    {
        return None;
    }
    match matching.as_slice() {
        [symbol] => Some(symbol.clone()),
        _ => None,
    }
}

fn effect_for_step(name: &str) -> Option<&'static str> {
    match name {
        "Set Field" | "Set Field By Name" => Some("a field write"),
        "Set Variable" => Some("a variable assignment"),
        "New Record/Request" => Some("record creation"),
        "Delete Record/Request" => Some("record deletion"),
        "Go to Layout" => Some("layout navigation"),
        "Perform Script" => Some("a script call"),
        "Enter Find Mode" | "Perform Find" | "Constrain Found Set" | "Extend Found Set" => {
            Some("a found-set operation")
        }
        "Sort Records" => Some("record sorting"),
        "Import Records" => Some("record import"),
        "Export Records" => Some("record export"),
        "Send Mail" => Some("email sending"),
        "Open URL" => Some("opening a URL"),
        "Commit Records/Requests" => Some("committing records"),
        "Exit Script" | "Halt Script" => Some("script termination"),
        _ => None,
    }
}

fn collect_variable_findings(script: &mut Component, step: &AttachedRecord) {
    let mut targets = BTreeSet::new();
    collect_variable_targets(&step.properties, &mut targets);
    if step.name == "Set Variable" {
        for variable in targets {
            script
                .findings
                .push(variable_finding(step, &variable, "variable_write", "sets"));
        }
    }
    let mut reads = BTreeSet::new();
    collect_calculation_variables(&step.properties, false, &mut reads);
    for variable in reads {
        script
            .findings
            .push(variable_finding(step, &variable, "variable_read", "reads"));
    }
}

fn variable_finding(
    step: &AttachedRecord,
    variable: &str,
    kind: &str,
    action: &str,
) -> AnalysisFinding {
    AnalysisFinding {
        kind: kind.to_string(),
        status: "explicit".to_string(),
        summary: format!("Step {} {action} variable {variable}", step.order),
        evidence: vec![step.source_path.clone()],
        limitations: vec!["Variable values and runtime flow are not evaluated.".to_string()],
    }
}

fn step_is_enabled(step: &AttachedRecord) -> bool {
    !step
        .properties
        .attributes
        .get("enable")
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "false" | "0"))
}

fn collect_variable_targets(node: &XmlNode, output: &mut BTreeSet<String>) {
    if node.name == "Name"
        && let Some(value) = node.attributes.get("value")
        && value.starts_with('$')
    {
        output.insert(value.clone());
    }
    for child in &node.children {
        collect_variable_targets(child, output);
    }
}

fn collect_calculation_variables(
    node: &XmlNode,
    in_calculation: bool,
    output: &mut BTreeSet<String>,
) {
    let in_calculation = in_calculation || node.name == "Calculation";
    if in_calculation && node.name == "Text" {
        output.extend(variables_in_text(&node.text));
    }
    for child in &node.children {
        collect_calculation_variables(child, in_calculation, output);
    }
}

fn variables_in_text(text: &str) -> BTreeSet<String> {
    let characters: Vec<_> = text.chars().collect();
    let mut variables = BTreeSet::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '$' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        if characters.get(index) == Some(&'$') {
            index += 1;
        }
        let name_start = index;
        while characters
            .get(index)
            .is_some_and(|character| character.is_alphanumeric() || *character == '_')
        {
            index += 1;
        }
        if index > name_start {
            variables.insert(characters[start..index].iter().collect());
        }
    }
    variables
}

fn build_control_flow(
    script: &mut Component,
    steps: &[AttachedRecord],
    edges: &mut Vec<AnalysisEdge>,
) {
    if steps.is_empty() {
        return;
    }
    let names: Vec<_> = steps.iter().map(|step| step.name.as_str()).collect();
    let blocks = match_control_blocks(&names);
    let mut adjacency: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate() {
        let next = (index + 1 < steps.len()).then_some(index + 1);
        let mut targets = Vec::new();
        let mut add = |target: Option<usize>, kind: &str| {
            if let Some(target) = target {
                targets.push(target);
                edges.push(AnalysisEdge {
                    kind: kind.to_string(),
                    status: "derived".to_string(),
                    from_object_id: Some(script.object_id.clone()),
                    to_object_id: Some(script.object_id.clone()),
                    from_record_id: Some(step.record_id.clone()),
                    to_record_id: Some(steps[target].record_id.clone()),
                    evidence: vec![step.source_path.clone()],
                    limitations: vec![
                        "Control flow is derived from recognized step names and order.".to_string(),
                    ],
                });
            }
        };
        match step.name.as_str() {
            "Exit Script" | "Halt Script" => {}
            "If" | "Else If" => {
                add(next, "control_true");
                let alternative = blocks.alternatives.get(&index).copied();
                let false_target = alternative.or_else(|| {
                    let owner = if step.name == "If" {
                        Some(index)
                    } else {
                        blocks.alternative_parents.get(&index).copied()
                    }?;
                    blocks
                        .if_ends
                        .get(&owner)
                        .and_then(|end| (*end + 1 < steps.len()).then_some(*end + 1))
                });
                add(false_target, "control_false");
            }
            "Else" => add(next, "control_next"),
            "End If" => add(next, "control_next"),
            "Loop" => add(next, "control_loop_body"),
            "Exit Loop If" => {
                add(next, "control_false");
                if let Some(end) = blocks.loop_ends.get(&index) {
                    add((*end + 1 < steps.len()).then_some(*end + 1), "control_true");
                }
            }
            "End Loop" => {
                if let Some(start) = blocks.loop_starts.get(&index) {
                    add(Some(*start), "control_loop_back");
                }
            }
            _ => {
                let target = next
                    .filter(|next_index| !matches!(names[*next_index], "Else" | "Else If"))
                    .or_else(|| {
                        next.and_then(|next_index| {
                            let owner = blocks.alternative_parents.get(&next_index)?;
                            blocks
                                .if_ends
                                .get(owner)
                                .and_then(|end| (*end + 1 < steps.len()).then_some(*end + 1))
                        })
                    });
                add(target, "control_next");
            }
        }
        adjacency.insert(index, targets);
    }
    let mut reached = BTreeSet::new();
    let mut pending = VecDeque::from([0]);
    while let Some(index) = pending.pop_front() {
        if reached.insert(index) {
            pending.extend(adjacency.get(&index).into_iter().flatten().copied());
        }
    }
    for (index, step) in steps.iter().enumerate() {
        if !reached.contains(&index) {
            script.findings.push(AnalysisFinding {
                kind: "unreachable_step_candidate".to_string(),
                status: "derived".to_string(),
                summary: format!(
                    "Step {} is unreachable in the recognized control-flow graph",
                    step.order
                ),
                evidence: vec![step.source_path.clone()],
                limitations: vec![
                    "Only recognized control-flow steps and explicit step order are modeled."
                        .to_string(),
                ],
            });
        }
    }
}

fn match_control_blocks(names: &[&str]) -> ControlBlocks {
    let mut if_stack = Vec::new();
    let mut loop_stack = Vec::new();
    let mut if_ends = BTreeMap::new();
    let mut alternatives = BTreeMap::new();
    let mut alternative_parents = BTreeMap::new();
    let mut loop_ends = BTreeMap::new();
    let mut loop_starts = BTreeMap::new();
    for (index, name) in names.iter().enumerate() {
        match *name {
            "If" => if_stack.push((index, index)),
            "Else If" | "Else" => {
                if let Some((root, previous)) = if_stack.last_mut() {
                    alternatives.insert(*previous, index);
                    alternative_parents.insert(index, *root);
                    *previous = index;
                }
            }
            "End If" => {
                if let Some((root, previous)) = if_stack.pop() {
                    if_ends.insert(root, index);
                    if_ends.insert(previous, index);
                }
            }
            "Loop" => loop_stack.push(index),
            "End Loop" => {
                if let Some(start) = loop_stack.pop() {
                    loop_ends.insert(start, index);
                    loop_starts.insert(index, start);
                    for (exit_index, name) in names.iter().enumerate().take(index).skip(start + 1) {
                        if *name == "Exit Loop If" {
                            loop_ends.insert(exit_index, index);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    ControlBlocks {
        if_ends,
        alternatives,
        alternative_parents,
        loop_ends,
        loop_starts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::extract_data_model;
    use std::io::Cursor;

    #[test]
    fn resolves_script_calls_and_builds_evidence_backed_control_flow() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <ScriptCatalog><Script id="1" name="Main"><UUID>main-uuid</UUID></Script><Script id="2" name="Target"><UUID>target-uuid</UUID></Script></ScriptCatalog>
          <StepsForScripts><Script><ScriptReference id="1" name="Main" UUID="main-uuid"/><ObjectList>
            <Step id="68" index="0" name="If" enable="True"><Options>0</Options><ParameterValues/></Step>
            <Step id="1" index="1" name="Perform Script" enable="True"><Options>0</Options><ParameterValues><Parameter><ScriptReference id="2" name="Target" UUID="target-uuid"/></Parameter></ParameterValues></Step>
            <Step id="141" index="2" name="Set Variable" enable="True"><Options>0</Options><ParameterValues><Parameter><Name value="$recordId"/></Parameter></ParameterValues></Step>
            <Step id="70" index="3" name="End If" enable="True"><Options>0</Options><ParameterValues/></Step>
            <Step id="103" index="4" name="Exit Script" enable="True"><Options>0</Options><ParameterValues/></Step>
            <Step id="102" index="5" name="Go to Layout" enable="True"><Options>0</Options><ParameterValues/></Step>
            <Step id="141" index="6" name="Set Variable" enable="False"><Options>0</Options><ParameterValues><Parameter><Name value="$ignored"/></Parameter></ParameterValues></Step>
            <Step id="1" index="7" name="Perform Script" enable="True"><Options>0</Options><ParameterValues><Parameter><Calculation><Text>$scriptName</Text></Calculation></Parameter></ParameterValues></Step>
            <Step id="1" index="8" name="Perform Script" enable="True"><Options>0</Options><ParameterValues><Parameter><ScriptReference id="99" name="Wrong ID" UUID="target-uuid"/></Parameter></ParameterValues></Step>
          </ObjectList></Script></StepsForScripts>
        </AddAction></Structure></FMSaveAsXML>"#;
        let mut components = extract_data_model(Cursor::new(xml)).unwrap();
        let edges = analyze_scripts(&mut components);
        let main = components.iter().find(|item| item.name == "Main").unwrap();
        let target = components
            .iter()
            .find(|item| item.name == "Target")
            .unwrap();
        assert!(edges.iter().any(|edge| {
            edge.kind == "calls_script"
                && edge.status == "explicit"
                && edge.from_object_id.as_deref() == Some(main.object_id.as_str())
                && edge.to_object_id.as_deref() == Some(target.object_id.as_str())
                && !edge.evidence.is_empty()
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "control_false"
                && edge.to_record_id.as_deref()
                    == main
                        .attached_records
                        .iter()
                        .find(|record| record.kind == "script_step" && record.order == 4)
                        .map(|record| record.record_id.as_str())
        }));
        assert!(main.findings.iter().any(
            |finding| finding.kind == "variable_write" && finding.summary.contains("$recordId")
        ));
        assert!(
            main.findings
                .iter()
                .any(|finding| finding.kind == "disabled_step")
        );
        assert!(
            !main
                .findings
                .iter()
                .any(|finding| finding.summary.contains("$ignored"))
        );
        assert!(
            main.findings
                .iter()
                .any(|finding| finding.kind == "dynamic_script_call")
        );
        assert!(edges.iter().any(|edge| {
            edge.kind == "calls_script"
                && edge.status == "unresolved"
                && edge.to_object_id.is_none()
        }));
        assert!(
            main.findings
                .iter()
                .any(|finding| finding.kind == "unreachable_step_candidate")
        );
    }
}
