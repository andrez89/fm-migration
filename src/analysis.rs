use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::components::{AnalysisFinding, AttachedRecord, Component, XmlNode};

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowCandidate {
    pub id: String,
    pub status: String,
    pub source_file: String,
    pub entry_object_id: String,
    pub entry_name: String,
    pub entry_kind: String,
    pub script_ids: Vec<String>,
    pub layout_ids: Vec<String>,
    pub field_read_ids: Vec<String>,
    pub field_write_ids: Vec<String>,
    pub table_occurrence_ids: Vec<String>,
    pub external_source_ids: Vec<String>,
    pub effects: Vec<String>,
    pub evidence: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone)]
struct ObjectSymbol {
    object_id: String,
    record_id: String,
    xml_id: Option<String>,
    source_identity: String,
}

struct Symbols {
    by_uuid: BTreeMap<(String, String, String), Vec<ObjectSymbol>>,
    by_id: BTreeMap<(String, String, String), Vec<ObjectSymbol>>,
    custom_functions_by_name: BTreeMap<(String, String), Vec<ObjectSymbol>>,
    qualified_fields: BTreeMap<(String, String), Vec<ObjectSymbol>>,
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
            collect_script_flow_findings(script, step);
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
    append_script_dataflow_candidates(components, &mut edges);
    append_script_call_summaries(components, &edges);
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

pub fn analyze_calculation_features(components: &mut [Component]) {
    for component in components {
        let mut calculations = Vec::new();
        collect_calculation_texts(&component.properties, &mut calculations);
        for record in &component.attached_records {
            collect_calculation_texts(&record.properties, &mut calculations);
        }
        let mut seen = BTreeSet::new();
        for text in calculations {
            for function in [
                "Evaluate",
                "GetField",
                "GetFieldName",
                "ExecuteSQL",
                "While",
            ] {
                if contains_function_call(&text, function) && seen.insert(function) {
                    component.findings.push(AnalysisFinding {
                        kind: "calculation_analysis_limit".to_string(),
                        status: "heuristic_candidate".to_string(),
                        summary: format!("Calculation uses {function}; its dynamic or nested semantics were not analyzed"),
                        evidence: vec![format!("{}/Calculation/Text", component.source_path)],
                        limitations: vec!["The function may be statically resolvable in context, but this analyzer does not parse or evaluate its arguments.".to_string()],
                    });
                }
            }
        }
        component.findings.sort_by(|left, right| {
            left.evidence
                .cmp(&right.evidence)
                .then(left.kind.cmp(&right.kind))
                .then(left.summary.cmp(&right.summary))
        });
    }
}

fn append_script_dataflow_candidates(components: &[Component], edges: &mut Vec<AnalysisEdge>) {
    let calls: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == "calls_script" && edge.status == "explicit")
        .cloned()
        .collect();
    for call in calls {
        let (Some(caller_id), Some(callee_id), Some(step_record_id)) = (
            call.from_object_id.as_deref(),
            call.to_object_id.as_deref(),
            call.from_record_id.as_deref(),
        ) else {
            continue;
        };
        let caller = components.iter().find(|component| {
            component.object_id == caller_id
                && component
                    .attached_records
                    .iter()
                    .any(|record| record.record_id == step_record_id)
        });
        let Some(caller) = caller else {
            continue;
        };
        let Some(step) = caller
            .attached_records
            .iter()
            .find(|record| record.record_id == step_record_id)
        else {
            continue;
        };
        let mut evidence = call.evidence.clone();
        let parameter_read = components.iter().find_map(|component| {
            (component.object_id == callee_id)
                .then(|| {
                    component
                        .findings
                        .iter()
                        .find(|finding| finding.kind == "script_parameter_read")
                        .map(|finding| (component, finding))
                })
                .flatten()
        });
        if contains_named_node(&step.properties, "Parameter")
            && let Some((callee_record, read)) = parameter_read
        {
            evidence.extend(read.evidence.iter().cloned());
            edges.push(AnalysisEdge {
                kind: "passes_script_parameter_to".to_string(),
                status: "heuristic_candidate".to_string(),
                from_object_id: Some(caller_id.to_string()),
                to_object_id: Some(callee_id.to_string()),
                from_record_id: Some(step_record_id.to_string()),
                to_record_id: Some(callee_record.record_id.clone()),
                evidence: evidence.clone(),
                limitations: vec!["The call has a parameter and the callee reads ScriptParameter, but the expression is not evaluated or proven to supply the value read.".to_string()],
            });
        }
        let result_return = components.iter().find_map(|component| {
            (component.object_id == callee_id)
                .then(|| {
                    component
                        .findings
                        .iter()
                        .find(|finding| finding.kind == "script_result_return")
                        .map(|finding| (component, finding))
                })
                .flatten()
        });
        let result_read = components.iter().find_map(|component| {
            (component.object_id == caller_id)
                .then(|| {
                    component
                        .findings
                        .iter()
                        .find(|finding| finding.kind == "script_result_read")
                        .map(|finding| (component, finding))
                })
                .flatten()
        });
        if let (Some((callee_record, returned)), Some((_, read))) = (result_return, result_read) {
            evidence.extend(returned.evidence.iter().cloned());
            evidence.extend(read.evidence.iter().cloned());
            edges.push(AnalysisEdge {
                kind: "may_receive_script_result_from".to_string(),
                status: "heuristic_candidate".to_string(),
                from_object_id: Some(caller_id.to_string()),
                to_object_id: Some(callee_id.to_string()),
                from_record_id: Some(step_record_id.to_string()),
                to_record_id: Some(callee_record.record_id.clone()),
                evidence,
                limitations: vec!["A caller reads ScriptResult and this callee returns a value, but the read is not tied to this call by execution order.".to_string()],
            });
        }
    }
}

fn collect_script_flow_findings(script: &mut Component, step: &AttachedRecord) {
    let mut calculations = Vec::new();
    collect_calculation_texts(&step.properties, &mut calculations);
    for text in calculations {
        let normalized: String = text
            .chars()
            .filter(|character| !character.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        for (needle, kind, summary) in [
            (
                "get(scriptparameter)",
                "script_parameter_read",
                "reads the calling script parameter",
            ),
            (
                "get(scriptresult)",
                "script_result_read",
                "reads a called script result",
            ),
        ] {
            if normalized.contains(needle) {
                script.findings.push(AnalysisFinding {
                    kind: kind.to_string(),
                    status: "explicit".to_string(),
                    summary: format!("Step {} {summary}", step.order),
                    evidence: vec![step.source_path.clone()],
                    limitations: vec![
                        "The expression and runtime value are not evaluated.".to_string(),
                    ],
                });
            }
        }
    }
    let (kind, summary) = match step.name.as_str() {
        "Perform Script" if contains_named_node(&step.properties, "Parameter") => (
            "script_parameter_pass",
            "passes a parameter to the called script",
        ),
        "Exit Script" if contains_named_node(&step.properties, "Calculation") => {
            ("script_result_return", "returns a calculated script result")
        }
        _ => return,
    };
    script.findings.push(AnalysisFinding {
        kind: kind.to_string(),
        status: "explicit".to_string(),
        summary: format!("Step {} {summary}", step.order),
        evidence: vec![step.source_path.clone()],
        limitations: vec!["The parameter/result expression is preserved in source data but is not evaluated or matched across caller and callee.".to_string()],
    });
}

fn contains_named_node(node: &XmlNode, name: &str) -> bool {
    node.name == name
        || node
            .children
            .iter()
            .any(|child| contains_named_node(child, name))
}

fn collect_calculation_texts(node: &XmlNode, output: &mut Vec<String>) {
    if node.name == "Calculation" {
        collect_text_nodes(node, output);
    } else {
        for child in &node.children {
            collect_calculation_texts(child, output);
        }
    }
}

fn collect_text_nodes(node: &XmlNode, output: &mut Vec<String>) {
    if node.name == "Text" && !node.text.trim().is_empty() {
        output.push(node.text.clone());
    }
    for child in &node.children {
        collect_text_nodes(child, output);
    }
}

fn append_script_call_summaries(components: &mut [Component], edges: &[AnalysisEdge]) {
    let calls: BTreeMap<&str, BTreeSet<&str>> = edges
        .iter()
        .filter(|edge| edge.kind == "calls_script" && edge.status == "explicit")
        .filter_map(|edge| {
            Some((
                edge.from_object_id.as_deref()?,
                edge.to_object_id.as_deref()?,
            ))
        })
        .fold(BTreeMap::new(), |mut calls, (from, to)| {
            calls.entry(from).or_default().insert(to);
            calls
        });
    for script in components
        .iter_mut()
        .filter(|component| component.kind == "script")
    {
        let direct = calls
            .get(script.object_id.as_str())
            .cloned()
            .unwrap_or_default();
        let mut reachable = BTreeSet::new();
        let mut pending: VecDeque<_> = direct.iter().copied().collect();
        let mut recursive = false;
        while let Some(target) = pending.pop_front() {
            if target == script.object_id {
                recursive = true;
            }
            if reachable.insert(target) {
                pending.extend(calls.get(target).into_iter().flatten().copied());
            }
        }
        let evidence: Vec<_> = edges
            .iter()
            .filter(|edge| {
                edge.kind == "calls_script"
                    && edge.status == "explicit"
                    && edge.from_object_id.as_deref() == Some(script.object_id.as_str())
            })
            .flat_map(|edge| edge.evidence.iter().cloned())
            .collect();
        if !direct.is_empty() {
            script.findings.push(AnalysisFinding {
                kind: "script_call_summary".to_string(),
                status: "derived".to_string(),
                summary: format!("Calls {} distinct script(s) directly and reaches {} script(s) transitively", direct.len(), reachable.len()),
                evidence: evidence.clone(),
                limitations: vec!["Only resolved explicit script references are included; dynamic targets and execution conditions are excluded.".to_string()],
            });
        }
        if recursive {
            script.findings.push(AnalysisFinding {
                kind: "recursive_script_call_candidate".to_string(),
                status: "derived".to_string(),
                summary: "The resolved script-call graph contains a path back to this script"
                    .to_string(),
                evidence,
                limitations: vec![
                    "Graph reachability does not prove that the recursive path executes."
                        .to_string(),
                ],
            });
        }
        script.findings.sort_by(|left, right| {
            left.evidence
                .cmp(&right.evidence)
                .then(left.kind.cmp(&right.kind))
                .then(left.summary.cmp(&right.summary))
        });
    }
}

pub fn analyze_component_references(components: &[Component]) -> Vec<AnalysisEdge> {
    let symbols = build_symbols(components);
    let mut edges = Vec::new();
    for component in components
        .iter()
        .filter(|component| component.kind != "script")
    {
        append_tree_edges(
            component,
            &component.properties,
            &component.source_path,
            &component.record_id,
            None,
            &symbols,
            &mut edges,
        );
        for record in &component.attached_records {
            append_tree_edges(
                component,
                &record.properties,
                &record.source_path,
                &record.record_id,
                Some(&record.kind),
                &symbols,
                &mut edges,
            );
        }
        if component.kind == "relationship" {
            append_relationship_edges(component, &symbols, &mut edges);
        }
    }
    edges
}

fn append_relationship_edges(
    relationship: &Component,
    symbols: &Symbols,
    edges: &mut Vec<AnalysisEdge>,
) {
    let mut predicates = Vec::new();
    collect_named_nodes(&relationship.properties, "JoinPredicate", &mut predicates);
    for (index, predicate) in predicates.into_iter().enumerate() {
        let left_field = first_node(predicate, "FieldReference");
        let right_field = predicate
            .children
            .iter()
            .find(|node| node.name == "RightField")
            .and_then(|node| first_node(node, "FieldReference"));
        let left_occurrence = left_field
            .and_then(|field| first_node(field, "TableOccurrenceReference"))
            .or_else(|| {
                relationship
                    .properties
                    .children
                    .iter()
                    .find(|node| node.name == "LeftTable")
                    .and_then(|node| first_node(node, "TableOccurrenceReference"))
            });
        let right_occurrence = right_field
            .and_then(|field| first_node(field, "TableOccurrenceReference"))
            .or_else(|| {
                relationship
                    .properties
                    .children
                    .iter()
                    .find(|node| node.name == "RightTable")
                    .and_then(|node| first_node(node, "TableOccurrenceReference"))
            });
        let left = left_occurrence.and_then(|reference| {
            resolve_reference(
                symbols,
                "table_occurrence",
                source_identity(relationship),
                reference,
            )
        });
        let right = right_occurrence.and_then(|reference| {
            resolve_reference(
                symbols,
                "table_occurrence",
                source_identity(relationship),
                reference,
            )
        });
        let Some(left) = left else {
            continue;
        };
        let operator = predicate
            .attributes
            .get("type")
            .map(String::as_str)
            .unwrap_or("unknown");
        let left_name = left_field
            .and_then(|field| field.attributes.get("name"))
            .map(String::as_str)
            .unwrap_or("unknown");
        let right_name = right_field
            .and_then(|field| field.attributes.get("name"))
            .map(String::as_str)
            .unwrap_or("unknown");
        edges.push(AnalysisEdge {
            kind: "joins_to".to_string(),
            status: if right.is_some() { "explicit" } else { "unresolved" }.to_string(),
            from_object_id: Some(left.object_id.clone()),
            to_object_id: right.as_ref().map(|symbol| symbol.object_id.clone()),
            from_record_id: Some(relationship.record_id.clone()),
            to_record_id: right.as_ref().map(|symbol| symbol.record_id.clone()),
            evidence: vec![format!("{}/JoinPredicate[{index}] {left_name} {operator} {right_name}", relationship.source_path)],
            limitations: if right.is_some() { Vec::new() } else { vec!["The right table occurrence could not be uniquely resolved in this source file.".to_string()] },
        });
    }
}

fn collect_named_nodes<'a>(node: &'a XmlNode, name: &str, output: &mut Vec<&'a XmlNode>) {
    if node.name == name {
        output.push(node);
    }
    for child in &node.children {
        collect_named_nodes(child, name, output);
    }
}

fn first_node<'a>(node: &'a XmlNode, name: &str) -> Option<&'a XmlNode> {
    if node.name == name {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| first_node(child, name))
}

pub fn discover_workflows(
    components: &[Component],
    edges: &[AnalysisEdge],
) -> Vec<WorkflowCandidate> {
    let mut objects = BTreeMap::new();
    let mut outgoing_edges: BTreeMap<&str, Vec<&AnalysisEdge>> = BTreeMap::new();
    for component in components {
        if component.source_action == "AddAction"
            || !objects.contains_key(component.object_id.as_str())
        {
            objects.insert(component.object_id.as_str(), component);
        }
    }
    for edge in edges {
        if let Some(source_id) = edge.from_object_id.as_deref() {
            outgoing_edges.entry(source_id).or_default().push(edge);
        }
    }
    let mut workflows = Vec::new();
    for entry_edge in edges.iter().filter(|edge| {
        matches!(
            edge.kind.as_str(),
            "triggers_script" | "is_triggered_by" | "navigates_to"
        )
    }) {
        let (Some(entry_id), Some(entry)) = (
            entry_edge.from_object_id.as_deref(),
            entry_edge
                .from_object_id
                .as_deref()
                .and_then(|object_id| objects.get(object_id).copied()),
        ) else {
            continue;
        };
        if !matches!(
            entry.kind.as_str(),
            "layout" | "layout_object" | "custom_menu"
        ) {
            continue;
        }
        let mut scripts = BTreeSet::new();
        let mut layouts = BTreeSet::new();
        let mut field_reads = BTreeSet::new();
        let mut field_writes = BTreeSet::new();
        let mut table_occurrences = BTreeSet::new();
        let mut external_sources = BTreeSet::new();
        let mut effects = BTreeSet::new();
        let mut evidence: BTreeSet<String> = entry_edge.evidence.iter().cloned().collect();
        let mut limitations = BTreeSet::from([
            "This is a reachability summary; execution order, branch outcomes, and runtime context are not evaluated.".to_string(),
        ]);
        let mut unresolved = entry_edge.status == "unresolved";
        let mut heuristic = entry_edge.status == "heuristic_candidate";
        if let Some(parent_id) = entry.parent_object_id.as_deref() {
            layouts.insert(parent_id.to_string());
        }
        if entry.kind == "layout" {
            layouts.insert(entry_id.to_string());
        }

        let mut pending_scripts = VecDeque::new();
        if entry_edge.kind != "navigates_to"
            && let Some(script_id) = entry_edge.to_object_id.as_deref()
        {
            pending_scripts.push_back(script_id.to_string());
        }
        if entry_edge.kind == "navigates_to"
            && let Some(layout_id) = entry_edge.to_object_id.as_deref()
        {
            layouts.insert(layout_id.to_string());
        }

        while let Some(script_id) = pending_scripts.pop_front() {
            if !scripts.insert(script_id.clone()) {
                continue;
            }
            if let Some(script) = objects.get(script_id.as_str()) {
                for finding in script
                    .findings
                    .iter()
                    .filter(|finding| finding.kind == "step_effect")
                {
                    effects.insert(finding.summary.clone());
                }
            }
            if let Some(script_edges) = outgoing_edges.get(script_id.as_str()) {
                for edge in script_edges.iter().copied().filter(|edge| {
                    matches!(
                        edge.kind.as_str(),
                        "calls_script"
                            | "navigates_to"
                            | "reads_field"
                            | "writes_field"
                            | "uses_table_occurrence"
                            | "accesses_external_source"
                    )
                }) {
                    evidence.extend(edge.evidence.iter().cloned());
                    if edge.status == "unresolved" {
                        unresolved = true;
                        limitations.insert(format!(
                            "Unresolved {} reference remains in reachable script {}.",
                            edge.kind, script_id
                        ));
                    }
                    heuristic |= edge.status == "heuristic_candidate";
                    if edge.kind == "calls_script"
                        && let Some(target) = edge.to_object_id.as_deref()
                    {
                        pending_scripts.push_back(target.to_string());
                    } else if edge.kind == "navigates_to"
                        && let Some(target) = edge.to_object_id.as_deref()
                    {
                        layouts.insert(target.to_string());
                    } else if edge.kind == "reads_field"
                        && let Some(target) = edge.to_object_id.as_deref()
                    {
                        field_reads.insert(target.to_string());
                    } else if edge.kind == "writes_field"
                        && let Some(target) = edge.to_object_id.as_deref()
                    {
                        field_writes.insert(target.to_string());
                    } else if edge.kind == "uses_table_occurrence"
                        && let Some(target) = edge.to_object_id.as_deref()
                    {
                        table_occurrences.insert(target.to_string());
                    } else if edge.kind == "accesses_external_source"
                        && let Some(target) = edge.to_object_id.as_deref()
                    {
                        external_sources.insert(target.to_string());
                    }
                }
            }
        }
        let target_id = entry_edge.to_object_id.as_deref().unwrap_or("unresolved");
        workflows.push(WorkflowCandidate {
            id: format!("workflow:{entry_id}:{}:{target_id}", entry_edge.kind),
            status: if unresolved {
                "partially_unresolved".to_string()
            } else if heuristic {
                "heuristic_candidate".to_string()
            } else {
                "structurally_derived".to_string()
            },
            source_file: entry.source_file.clone(),
            entry_object_id: entry_id.to_string(),
            entry_name: if entry.name.trim().is_empty() {
                format!(
                    "{} {}",
                    entry.kind,
                    entry.xml_id.as_deref().unwrap_or(&entry.object_id)
                )
            } else {
                entry.name.clone()
            },
            entry_kind: entry.kind.clone(),
            script_ids: scripts.into_iter().collect(),
            layout_ids: layouts.into_iter().collect(),
            field_read_ids: field_reads.into_iter().collect(),
            field_write_ids: field_writes.into_iter().collect(),
            table_occurrence_ids: table_occurrences.into_iter().collect(),
            external_source_ids: external_sources.into_iter().collect(),
            effects: effects.into_iter().collect(),
            evidence: evidence.into_iter().collect(),
            limitations: limitations.into_iter().collect(),
        });
    }
    workflows.sort_by(|left, right| left.id.cmp(&right.id));
    workflows.dedup_by(|left, right| left.id == right.id);
    workflows
}

fn append_tree_edges(
    component: &Component,
    root: &XmlNode,
    source_path: &str,
    source_record_id: &str,
    attached_kind: Option<&str>,
    symbols: &Symbols,
    edges: &mut Vec<AnalysisEdge>,
) {
    let mut references = Vec::new();
    collect_reference_paths(root, &mut Vec::new(), &mut references);
    for (node, path) in references {
        if component.kind == "layout" && path.iter().any(|part| part == "LayoutObject") {
            continue;
        }
        if matches!(
            (attached_kind, node.name.as_str()),
            (
                Some("custom_function_calculation"),
                "CustomFunctionReference"
            ) | (Some("value_list_options"), "ValueListReference")
        ) {
            continue;
        }
        let Some(target_kind) = reference_component_kind(&node.name) else {
            continue;
        };
        let target = resolve_reference(symbols, target_kind, source_identity(component), node);
        let kind = component_reference_kind(component, &node.name, &path);
        let evidence = format!("{source_path}/{}", path.join("/"));
        edges.push(AnalysisEdge {
            kind: kind.to_string(),
            status: resolution_status(target.as_ref(), source_identity(component), "explicit"),
            from_object_id: Some(component.object_id.clone()),
            to_object_id: target.as_ref().map(|symbol| symbol.object_id.clone()),
            from_record_id: Some(source_record_id.to_string()),
            to_record_id: target.as_ref().map(|symbol| symbol.record_id.clone()),
            evidence: vec![evidence],
            limitations: match target.as_ref() {
                Some(target) if target.source_identity != source_identity(component) => vec!["Resolved by a unique UUID match across supplied files; confirm that the files represent linked FileMaker sources.".to_string()],
                Some(_) => Vec::new(),
                None => vec!["The reference target is absent or ambiguous in the supplied files.".to_string()],
            },
        });
    }
    append_custom_function_edges(
        component,
        root,
        source_path,
        source_record_id,
        symbols,
        edges,
    );
    append_textual_calculation_edges(
        component,
        root,
        source_path,
        source_record_id,
        symbols,
        edges,
    );
}

fn append_textual_calculation_edges(
    component: &Component,
    root: &XmlNode,
    source_path: &str,
    source_record_id: &str,
    symbols: &Symbols,
    edges: &mut Vec<AnalysisEdge>,
) {
    let mut calculations = Vec::new();
    collect_calculation_texts(root, &mut calculations);
    for text in calculations {
        let text = text.to_lowercase();
        if text.contains("::") {
            for ((identity, qualified_name), targets) in &symbols.qualified_fields {
                if identity != source_identity(component)
                    || !contains_identifier_folded(&text, qualified_name)
                {
                    continue;
                }
                let target = match targets.as_slice() {
                    [target] => Some(target),
                    _ => None,
                };
                edges.push(AnalysisEdge {
                    kind: "reads_field".to_string(),
                    status: if target.is_some() { "heuristic_candidate" } else { "unresolved" }.to_string(),
                    from_object_id: Some(component.object_id.clone()),
                    to_object_id: target.map(|target| target.object_id.clone()),
                    from_record_id: Some(source_record_id.to_string()),
                    to_record_id: target.map(|target| target.record_id.clone()),
                    evidence: vec![format!("{source_path}/Calculation/Text (name match: {qualified_name})")],
                    limitations: vec!["This is a qualified-name match in calculation text; it may not resolve table-occurrence aliases or quoted identifiers. Prefer structured FieldReference chunks when available.".to_string()],
                });
            }
        }
        if text.contains('(') {
            for ((identity, name), targets) in &symbols.custom_functions_by_name {
                if identity != source_identity(component)
                    || !contains_function_call_folded(&text, name)
                {
                    continue;
                }
                let target = match targets.as_slice() {
                    [target] => Some(target),
                    _ => None,
                };
                edges.push(AnalysisEdge {
                    kind: "uses_custom_function".to_string(),
                    status: if target.is_some() { "heuristic_candidate" } else { "unresolved" }.to_string(),
                    from_object_id: Some(component.object_id.clone()),
                    to_object_id: target.map(|target| target.object_id.clone()),
                    from_record_id: Some(source_record_id.to_string()),
                    to_record_id: target.map(|target| target.record_id.clone()),
                    evidence: vec![format!("{source_path}/Calculation/Text (name match: {name})")],
                    limitations: vec!["This is a function-name match in calculation text; nested syntax and quoted function names are not parsed.".to_string()],
                });
            }
        }
    }
}

fn contains_identifier_folded(text: &str, needle: &str) -> bool {
    text.match_indices(&needle).any(|(start, _)| {
        let end = start + needle.len();
        let identifier = |byte: Option<u8>| {
            byte.is_some_and(|byte| byte >= 128 || byte.is_ascii_alphanumeric() || byte == b'_')
        };
        !identifier(
            start
                .checked_sub(1)
                .and_then(|index| text.as_bytes().get(index).copied()),
        ) && !identifier(text.as_bytes().get(end).copied())
    })
}

fn contains_function_call(text: &str, name: &str) -> bool {
    let text = text.to_lowercase();
    contains_function_call_folded(&text, name)
}

fn contains_function_call_folded(text: &str, name: &str) -> bool {
    let name = name.to_lowercase();
    text.match_indices(&name).any(|(start, _)| {
        let end = start + name.len();
        let before = start
            .checked_sub(1)
            .and_then(|index| text.as_bytes().get(index).copied());
        let after = text[end..].trim_start();
        !before.is_some_and(|byte| byte >= 128 || byte.is_ascii_alphanumeric() || byte == b'_')
            && after.starts_with('(')
    })
}

fn append_custom_function_edges(
    component: &Component,
    root: &XmlNode,
    source_path: &str,
    source_record_id: &str,
    symbols: &Symbols,
    edges: &mut Vec<AnalysisEdge>,
) {
    let mut function_chunks = Vec::new();
    collect_custom_function_chunks(root, &mut Vec::new(), &mut function_chunks);
    for (chunk, path) in function_chunks {
        let name = chunk.text.trim();
        let target = symbols
            .custom_functions_by_name
            .get(&(source_identity(component).to_string(), name.to_string()))
            .and_then(|matches| match matches.as_slice() {
                [symbol] => Some(symbol),
                _ => None,
            });
        edges.push(AnalysisEdge {
            kind: "uses_custom_function".to_string(),
            status: resolution_status(target, source_identity(component), "explicit"),
            from_object_id: Some(component.object_id.clone()),
            to_object_id: target.map(|symbol| symbol.object_id.clone()),
            from_record_id: Some(source_record_id.to_string()),
            to_record_id: target.map(|symbol| symbol.record_id.clone()),
            evidence: vec![format!("{source_path}/{}", path.join("/"))],
            limitations: if target.is_some() {
                Vec::new()
            } else {
                vec![
                    "The custom function name is absent or ambiguous in this source file."
                        .to_string(),
                ]
            },
        });
    }
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
            resolve_reference(symbols, target_kind, source_identity(script), node)
        });
        let status = resolution_status(target.as_ref(), source_identity(script), "explicit");
        let evidence = format!("{} ({})", step.source_path, node.name);
        if target.is_none() {
            script.diagnostics.push(format!(
                "unresolved {} in script step {} ({})",
                node.name, step.order, step.name
            ));
        }
        edges.push(AnalysisEdge {
            kind: kind.unwrap_or("references_object").to_string(),
            status,
            from_object_id: Some(script.object_id.clone()),
            to_object_id: target.as_ref().map(|symbol| symbol.object_id.clone()),
            from_record_id: Some(step.record_id.clone()),
            to_record_id: target.as_ref().map(|symbol| symbol.record_id.clone()),
            evidence: vec![evidence],
            limitations: match target.as_ref() {
                Some(target) if target.source_identity != source_identity(script) => vec!["Resolved by a unique UUID match across supplied files; confirm that the files represent linked FileMaker sources.".to_string()],
                Some(_) => Vec::new(),
                None => vec!["The reference target is absent or ambiguous in the supplied files.".to_string()],
            },
        });
    }
    append_custom_function_edges(
        script,
        &step.properties,
        &step.source_path,
        &step.record_id,
        symbols,
        edges,
    );
    append_textual_calculation_edges(
        script,
        &step.properties,
        &step.source_path,
        &step.record_id,
        symbols,
        edges,
    );
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

fn collect_reference_paths<'a>(
    node: &'a XmlNode,
    path: &mut Vec<String>,
    references: &mut Vec<(&'a XmlNode, Vec<String>)>,
) {
    path.push(node.name.clone());
    if reference_component_kind(&node.name).is_some()
        || (node.name == "DataSourceReference"
            && node.attributes.get("type").map(String::as_str)
                == Some("ExternalDataSourceReference"))
    {
        references.push((node, path.clone()));
    }
    for child in &node.children {
        collect_reference_paths(child, path, references);
    }
    path.pop();
}

fn collect_custom_function_chunks<'a>(
    node: &'a XmlNode,
    path: &mut Vec<String>,
    chunks: &mut Vec<(&'a XmlNode, Vec<String>)>,
) {
    path.push(node.name.clone());
    if node.name == "Chunk"
        && node.attributes.get("type").map(String::as_str) == Some("CustomFunctionRef")
    {
        chunks.push((node, path.clone()));
    }
    for child in &node.children {
        collect_custom_function_chunks(child, path, chunks);
    }
    path.pop();
}

fn reference_edge_kind(name: &str) -> Option<&'static str> {
    match name {
        "ScriptReference" => Some("calls_script"),
        "FieldReference" => Some("reads_field"),
        "TableOccurrenceReference" => Some("uses_table_occurrence"),
        "ValueListReference" => Some("uses_value_list"),
        "CustomFunctionReference" => Some("uses_custom_function"),
        "LayoutReference" => Some("navigates_to"),
        "BaseTableReference" => Some("references_table"),
        "RelationshipReference" => Some("references_relationship"),
        "PrivilegeSetReference" => Some("uses_privilege_set"),
        "ExtendedPrivilegeReference" => Some("uses_extended_privilege"),
        "CustomMenuReference" => Some("uses_custom_menu"),
        "CustomMenuSetReference" => Some("uses_custom_menu_set"),
        "ThemeReference" | "LayoutThemeReference" => Some("uses_theme"),
        "ExternalDataSourceReference" => Some("accesses_external_source"),
        "DataSourceReference" => Some("accesses_external_source"),
        "BaseDirectoryReference" => Some("uses_base_directory"),
        "LibraryReference" => Some("uses_resource"),
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
        "BaseTableReference" => Some("table"),
        "RelationshipReference" => Some("relationship"),
        "PrivilegeSetReference" => Some("privilege_set"),
        "ExtendedPrivilegeReference" => Some("extended_privilege"),
        "CustomMenuReference" => Some("custom_menu"),
        "CustomMenuSetReference" => Some("custom_menu_set"),
        "ThemeReference" | "LayoutThemeReference" => Some("theme"),
        "ExternalDataSourceReference" => Some("external_data_source"),
        "DataSourceReference" => Some("external_data_source"),
        "BaseDirectoryReference" => Some("base_directory"),
        "LibraryReference" => Some("resource"),
        _ => None,
    }
}

fn component_reference_kind(
    component: &Component,
    reference: &str,
    path: &[String],
) -> &'static str {
    match (component.kind.as_str(), reference) {
        ("account", "PrivilegeSetReference") => "uses_privilege_set",
        ("layout" | "layout_object", "ScriptReference")
            if path.iter().any(|part| part == "ScriptTriggers") =>
        {
            "is_triggered_by"
        }
        ("layout_object", "ScriptReference")
            if path
                .iter()
                .any(|part| matches!(part.as_str(), "Button" | "ButtonBar" | "action")) =>
        {
            "triggers_script"
        }
        ("custom_menu", "ScriptReference") => "triggers_script",
        (
            "privilege_set",
            "BaseTableReference"
            | "TableOccurrenceReference"
            | "FieldReference"
            | "LayoutReference"
            | "ScriptReference"
            | "ValueListReference",
        ) => "has_access_rule_for",
        (_, "ScriptReference") => "references_script",
        ("layout_object", "LayoutReference") => "navigates_to",
        (_, "FieldReference") => "references_field",
        (_, _) => reference_edge_kind(reference).unwrap_or("references_object"),
    }
}

fn source_identity(component: &Component) -> &str {
    component
        .object_id
        .split(':')
        .next()
        .filter(|identity| !identity.is_empty())
        .unwrap_or(&component.source_file)
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
        custom_functions_by_name: BTreeMap::new(),
        qualified_fields: BTreeMap::new(),
    };
    for component in components {
        let symbol = ObjectSymbol {
            object_id: component.object_id.clone(),
            record_id: component.record_id.clone(),
            xml_id: component.xml_id.clone(),
            source_identity: source_identity(component).to_string(),
        };
        if let Some(uuid) = &component.xml_uuid {
            insert_symbol(
                &mut symbols.by_uuid,
                (
                    source_identity(component).to_string(),
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
                    source_identity(component).to_string(),
                    component.kind.clone(),
                    id.clone(),
                ),
                symbol.clone(),
                component.source_action == "AddAction",
            );
        }
        if component.kind == "custom_function" {
            let entries = symbols
                .custom_functions_by_name
                .entry((
                    source_identity(component).to_string(),
                    component.name.clone(),
                ))
                .or_default();
            if let Some(existing) = entries
                .iter_mut()
                .find(|entry| entry.object_id == symbol.object_id)
            {
                if component.source_action == "AddAction" {
                    *existing = symbol.clone();
                }
            } else {
                entries.push(symbol.clone());
            }
        }
        if component.kind == "field"
            && let Some(table_name) = component.table_name.as_deref()
        {
            insert_symbol(
                &mut symbols.qualified_fields,
                (
                    source_identity(component).to_string(),
                    format!(
                        "{}::{}",
                        table_name.to_lowercase(),
                        component.name.to_lowercase()
                    ),
                ),
                symbol,
                component.source_action == "AddAction",
            );
        }
    }
    symbols
}

fn insert_symbol<K: Ord>(
    symbols: &mut BTreeMap<K, Vec<ObjectSymbol>>,
    key: K,
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
    source_identity: &str,
    reference: &XmlNode,
) -> Option<ObjectSymbol> {
    let uuid = reference.attributes.get("UUID");
    let xml_id = reference.attributes.get("id");
    let mut cross_file = false;
    let matching = if let Some(uuid) = uuid.filter(|uuid| !uuid.is_empty()) {
        if let Some(matching) =
            symbols
                .by_uuid
                .get(&(source_identity.to_string(), kind.to_string(), uuid.clone()))
        {
            matching.as_slice()
        } else {
            cross_file = true;
            let mut matches = symbols
                .by_uuid
                .iter()
                .filter(|((_, candidate_kind, candidate_uuid), _)| {
                    candidate_kind == kind && candidate_uuid == uuid
                })
                .flat_map(|(_, entries)| entries);
            let first = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            std::slice::from_ref(first)
        }
    } else {
        let xml_id = xml_id?;
        symbols
            .by_id
            .get(&(
                source_identity.to_string(),
                kind.to_string(),
                xml_id.clone(),
            ))?
            .as_slice()
    };
    if !cross_file
        && uuid.is_some()
        && (matching.len() != 1
            || xml_id.is_some_and(|id| matching[0].xml_id.as_deref() != Some(id.as_str())))
    {
        return None;
    }
    match matching {
        [symbol] => Some(symbol.clone()),
        _ => None,
    }
}

fn resolution_status(
    target: Option<&ObjectSymbol>,
    source_identity: &str,
    local_status: &str,
) -> String {
    match target {
        Some(target) if target.source_identity != source_identity => {
            "heuristic_candidate".to_string()
        }
        Some(_) => local_status.to_string(),
        None => "unresolved".to_string(),
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
        "Insert from URL" => Some("fetching data from a URL"),
        "Perform Script on Server" => Some("calling a server-side script"),
        "Perform JavaScript in Web Viewer" => Some("executing JavaScript in a web viewer"),
        "Set Web Viewer" => Some("changing web-viewer content"),
        "Open File" => Some("opening a file"),
        "Save Records as PDF" => Some("creating a PDF export"),
        "Open Transaction" => Some("opening a transaction"),
        "Commit Transaction" => Some("committing a transaction"),
        "Revert Transaction" => Some("reverting a transaction"),
        "Set Error Capture" => Some("changing FileMaker error-capture behavior"),
        "Allow User Abort" => Some("changing user-abort behavior"),
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

    #[test]
    fn captures_script_parameter_result_and_call_flow_candidates() {
        let xml = br#"<FMSaveAsXML File="Flow.fmp12" UUID="flow-file"><Structure><AddAction>
          <ScriptCatalog><Script id="1" name="Caller"><UUID>caller</UUID></Script><Script id="2" name="Callee"><UUID>callee</UUID></Script></ScriptCatalog>
          <StepsForScripts>
            <Script><ScriptReference id="1" name="Caller" UUID="caller"/><ObjectList>
              <Step id="1" index="0" name="Perform Script" enable="True"><Options>0</Options><ParameterValues><Parameter><ScriptReference id="2" name="Callee" UUID="callee"/><Calculation><Text>42</Text></Calculation></Parameter></ParameterValues></Step>
              <Step id="141" index="1" name="Set Variable" enable="True"><Options>0</Options><ParameterValues><Parameter><Name value="$result"/><Calculation><Text>Get ( ScriptResult )</Text></Calculation></Parameter></ParameterValues></Step>
            </ObjectList></Script>
            <Script><ScriptReference id="2" name="Callee" UUID="callee"/><ObjectList>
              <Step id="141" index="0" name="Set Variable" enable="True"><Options>0</Options><ParameterValues><Parameter><Name value="$input"/><Calculation><Text>Get ( ScriptParameter )</Text></Calculation></Parameter></ParameterValues></Step>
              <Step id="103" index="1" name="Exit Script" enable="True"><Options>0</Options><ParameterValues><Parameter><Calculation><Text>$input</Text></Calculation></Parameter></ParameterValues></Step>
            </ObjectList></Script>
          </StepsForScripts>
        </AddAction></Structure></FMSaveAsXML>"#;
        let mut components = extract_data_model(Cursor::new(xml)).unwrap();
        let edges = analyze_scripts(&mut components);
        assert!(
            edges
                .iter()
                .any(|edge| edge.kind == "passes_script_parameter_to"
                    && edge.status == "heuristic_candidate")
        );
        assert!(
            edges
                .iter()
                .any(|edge| edge.kind == "may_receive_script_result_from"
                    && edge.status == "heuristic_candidate")
        );
        let callee = components
            .iter()
            .find(|component| component.name == "Callee")
            .unwrap();
        assert!(
            callee
                .findings
                .iter()
                .any(|finding| finding.kind == "script_parameter_read")
        );
        assert!(
            callee
                .findings
                .iter()
                .any(|finding| finding.kind == "script_result_return")
        );
    }

    #[test]
    fn resolves_relationship_join_edges_between_distinct_occurrences() {
        let xml = br#"<FMSaveAsXML File="Join.fmp12" UUID="join-file"><Structure><AddAction>
          <BaseTableCatalog><BaseTable id="1" name="A"><UUID>table-a</UUID></BaseTable><BaseTable id="2" name="B"><UUID>table-b</UUID></BaseTable></BaseTableCatalog>
          <TableOccurrenceCatalog><TableOccurrence id="10" name="A"><UUID>to-a</UUID><BaseTableReference id="1" name="A" UUID="table-a"/></TableOccurrence><TableOccurrence id="11" name="B"><UUID>to-b</UUID><BaseTableReference id="2" name="B" UUID="table-b"/></TableOccurrence></TableOccurrenceCatalog>
          <RelationshipCatalog><Relationship id="3" name="A_to_B"><UUID>rel</UUID><LeftTable><TableOccurrenceReference id="10" name="A" UUID="to-a"/></LeftTable><RightTable><TableOccurrenceReference id="11" name="B" UUID="to-b"/></RightTable><JoinPredicateList><JoinPredicate type="Equal"><LeftField><FieldReference id="1" name="id" UUID="field-a"><TableOccurrenceReference id="10" name="A" UUID="to-a"/></FieldReference></LeftField><RightField><FieldReference id="2" name="a_id" UUID="field-b"><TableOccurrenceReference id="11" name="B" UUID="to-b"/></FieldReference></RightField></JoinPredicate></JoinPredicateList></Relationship></RelationshipCatalog>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        let edges = analyze_component_references(&components);
        let left = components
            .iter()
            .find(|component| component.kind == "table_occurrence" && component.name == "A")
            .unwrap();
        let right = components
            .iter()
            .find(|component| component.kind == "table_occurrence" && component.name == "B")
            .unwrap();
        assert!(edges.iter().any(|edge| edge.kind == "joins_to"
            && edge.status == "explicit"
            && edge.from_object_id.as_deref() == Some(left.object_id.as_str())
            && edge.to_object_id.as_deref() == Some(right.object_id.as_str())
            && edge.evidence.iter().any(|source| source.contains("Equal"))));
    }

    #[test]
    fn extracts_qualified_calculation_dependencies_as_heuristic_edges() {
        let xml = br#"<FMSaveAsXML File="Calc.fmp12" UUID="calc-file"><Structure><AddAction>
          <BaseTableCatalog><BaseTable id="1" name="T"><UUID>table</UUID></BaseTable></BaseTableCatalog>
          <FieldCatalog><BaseTableReference id="1" name="T" UUID="table"/><Field id="1" name="Value"><UUID>value</UUID></Field><Field id="2" name="Computed"><UUID>computed</UUID><Calculation><Text>T::Value + Normalize(1) + GetField($fieldName)</Text></Calculation></Field></FieldCatalog>
          <CustomFunctionsCatalog><CustomFunction id="1" name="Normalize"><UUID>normalize</UUID></CustomFunction></CustomFunctionsCatalog>
        </AddAction></Structure></FMSaveAsXML>"#;
        let mut components = extract_data_model(Cursor::new(xml)).unwrap();
        analyze_calculation_features(&mut components);
        let computed = components
            .iter()
            .find(|component| component.name == "Computed")
            .unwrap();
        let value = components
            .iter()
            .find(|component| component.name == "Value")
            .unwrap();
        let normalize = components
            .iter()
            .find(|component| component.kind == "custom_function")
            .unwrap();
        let edges = analyze_component_references(&components);
        assert!(edges.iter().any(|edge| edge.kind == "reads_field"
            && edge.status == "heuristic_candidate"
            && edge.from_object_id.as_deref() == Some(computed.object_id.as_str())
            && edge.to_object_id.as_deref() == Some(value.object_id.as_str())));
        assert!(edges.iter().any(|edge| edge.kind == "uses_custom_function"
            && edge.status == "heuristic_candidate"
            && edge.from_object_id.as_deref() == Some(computed.object_id.as_str())
            && edge.to_object_id.as_deref() == Some(normalize.object_id.as_str())));
        assert!(
            computed
                .findings
                .iter()
                .any(|finding| finding.kind == "calculation_analysis_limit"
                    && finding.summary.contains("GetField"))
        );
    }

    #[test]
    fn resolves_unique_cross_file_uuid_as_heuristic_candidate_even_if_numeric_ids_differ() {
        let target = br#"<FMSaveAsXML File="Target.fmp12" UUID="target-file"><Structure><AddAction><BaseTableCatalog><BaseTable id="1" name="T"><UUID>table</UUID></BaseTable></BaseTableCatalog><FieldCatalog><BaseTableReference id="1" name="T" UUID="table"/><Field id="1" name="Value"><UUID>global-field</UUID></Field></FieldCatalog></AddAction></Structure></FMSaveAsXML>"#;
        let source = br#"<FMSaveAsXML File="Source.fmp12" UUID="source-file"><Structure><AddAction><BaseTableCatalog><BaseTable id="1" name="S"><UUID>source-table</UUID></BaseTable></BaseTableCatalog><FieldCatalog><BaseTableReference id="1" name="S" UUID="source-table"/><Field id="2" name="Copy"><UUID>copy-field</UUID><Calculation><FieldReference id="999" name="Value" UUID="global-field"/></Calculation></Field></FieldCatalog></AddAction></Structure></FMSaveAsXML>"#;
        let mut components = extract_data_model(Cursor::new(target)).unwrap();
        components.extend(extract_data_model(Cursor::new(source)).unwrap());
        let source_field = components
            .iter()
            .find(|component| component.name == "Copy")
            .unwrap();
        let target_field = components
            .iter()
            .find(|component| component.name == "Value")
            .unwrap();
        let edges = analyze_component_references(&components);
        assert!(edges.iter().any(|edge| edge.kind == "references_field"
            && edge.status == "heuristic_candidate"
            && edge.from_object_id.as_deref() == Some(source_field.object_id.as_str())
            && edge.to_object_id.as_deref() == Some(target_field.object_id.as_str())));
    }
}
