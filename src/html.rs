use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader},
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    analysis::{AnalysisEdge, WorkflowCandidate},
    components::{Component, render_properties},
};

#[derive(Deserialize)]
struct WorkflowExport {
    schema_version: u32,
    workflows: Vec<WorkflowCandidate>,
}

#[derive(Deserialize)]
struct DiagnosticExport {
    schema_version: u32,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Deserialize)]
struct Diagnostic {
    id: String,
    kind: String,
    status: String,
    summary: String,
    source_object_id: Option<String>,
    target_object_id: Option<String>,
    evidence: Vec<String>,
    limitations: Vec<String>,
}

struct Pages<'a> {
    by_record_id: BTreeMap<&'a str, String>,
    by_object_id: BTreeMap<&'a str, &'a Component>,
    edges_by_object_id: BTreeMap<&'a str, Vec<&'a AnalysisEdge>>,
}

impl<'a> Pages<'a> {
    fn new(components: &'a [Component], edges: &'a [AnalysisEdge]) -> Self {
        let mut pages = Self {
            by_record_id: BTreeMap::new(),
            by_object_id: BTreeMap::new(),
            edges_by_object_id: BTreeMap::new(),
        };
        for (index, component) in components.iter().enumerate() {
            let filename = format!(
                "objects/{:05}-{}-{}.html",
                index + 1,
                slug(&component.kind),
                slug(&component.name)
            );
            pages
                .by_record_id
                .insert(component.record_id.as_str(), filename);
            pages
                .by_object_id
                .entry(component.object_id.as_str())
                .or_insert(component);
            if component.source_action == "AddAction" {
                pages
                    .by_object_id
                    .entry(component.object_id.as_str())
                    .or_insert(component);
            }
        }
        for edge in edges {
            if let Some(id) = edge.from_object_id.as_deref() {
                pages.edges_by_object_id.entry(id).or_default().push(edge);
            }
            if let Some(id) = edge.to_object_id.as_deref()
                && edge.from_object_id.as_deref() != Some(id)
            {
                pages.edges_by_object_id.entry(id).or_default().push(edge);
            }
        }
        pages
    }

    fn for_object(&self, id: &str) -> Option<&str> {
        self.by_object_id
            .get(id)
            .and_then(|component| self.by_record_id.get(component.record_id.as_str()))
            .map(String::as_str)
    }
}

pub fn render_bundle(bundle: &Path, output: &Path) -> Result<()> {
    let root = fs::canonicalize(bundle)
        .with_context(|| format!("cannot access bundle {}", bundle.display()))?;
    let applications: Value = read_json(&root.join("application.json"))?;
    let coverage: Value = read_json(&root.join("coverage.json"))?;
    let diagnostic_export: DiagnosticExport = read_json(&root.join("diagnostics.json"))?;
    let workflow_export: WorkflowExport = read_json(&root.join("workflows/workflows.json"))?;
    if workflow_export.schema_version != 2 || diagnostic_export.schema_version != 1 {
        bail!("unsupported workflow or diagnostic schema version");
    }
    let components: Vec<Component> = read_jsonl(&root.join("objects.jsonl"))?;
    let edges: Vec<AnalysisEdge> = read_jsonl(&root.join("edges.jsonl"))?;
    let pages = Pages::new(&components, &edges);

    if output.exists() && fs::symlink_metadata(output)?.file_type().is_symlink() {
        bail!("HTML output directory must not be a symbolic link");
    }
    fs::create_dir_all(output)
        .with_context(|| format!("cannot create HTML output {}", output.display()))?;
    let output = fs::canonicalize(output)?;
    let objects_dir = output.join("objects");
    if objects_dir.exists() && fs::symlink_metadata(&objects_dir)?.file_type().is_symlink() {
        bail!("HTML objects directory must not be a symbolic link");
    }
    fs::create_dir_all(&objects_dir)?;
    fs::create_dir_all(output.join("workflows"))?;
    write_file(&output, "style.css", &format!("{STYLE}{STYLE_DIAGRAM}"))?;
    write_file(&output, "app.js", APP_JS)?;

    for component in &components {
        let filename = &pages.by_record_id[component.record_id.as_str()];
        let related_edges = pages
            .edges_by_object_id
            .get(component.object_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        write_file(
            &output,
            filename,
            &render_component(component, related_edges, &pages),
        )?;
    }
    write_file(
        &output,
        "index.html",
        &render_index(
            &applications,
            &coverage,
            &components,
            &pages,
            workflow_export.workflows.len(),
            diagnostic_export.diagnostics.len(),
        ),
    )?;
    write_file(
        &output,
        "workflows.html",
        &render_workflows(&workflow_export.workflows, &pages),
    )?;
    for (index, workflow) in workflow_export.workflows.iter().enumerate() {
        write_file(
            &output,
            &workflow_filename(index, workflow),
            &render_workflow_detail(workflow, &components, &edges, &pages),
        )?;
    }
    write_file(
        &output,
        "relationships.html",
        &render_relationships(&components, &edges, &pages),
    )?;
    write_file(
        &output,
        "diagnostics.html",
        &render_diagnostics(&diagnostic_export.diagnostics, &pages),
    )?;
    write_file(&output, "coverage.html", &render_coverage(&coverage))?;
    println!(
        "Rendered {} object pages and {} workflow diagrams to {}",
        components.len(),
        workflow_export.workflows.len(),
        output.display()
    );
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(BufReader::new(File::open(path)?))
        .with_context(|| format!("invalid JSON in {}", path.display()))
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let mut records = Vec::new();
    for (line_number, line) in BufReader::new(File::open(path)?).lines().enumerate() {
        let line = line?;
        records.push(
            serde_json::from_str(&line).with_context(|| {
                format!("invalid JSONL at {}:{}", path.display(), line_number + 1)
            })?,
        );
    }
    Ok(records)
}

fn write_file(root: &Path, relative: &str, content: &str) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() && fs::symlink_metadata(&path)?.file_type().is_symlink() {
        bail!("refusing to replace symbolic link {}", path.display());
    }
    fs::write(&path, content).with_context(|| format!("cannot write {}", path.display()))
}

fn render_index(
    applications: &Value,
    coverage: &Value,
    components: &[Component],
    pages: &Pages<'_>,
    workflow_count: usize,
    diagnostic_count: usize,
) -> String {
    let kinds: BTreeSet<_> = components.iter().map(|item| item.kind.as_str()).collect();
    let sources: BTreeSet<_> = components
        .iter()
        .map(|item| item.source_file.as_str())
        .collect();
    let actions: BTreeSet<_> = components
        .iter()
        .map(|item| item.source_action.as_str())
        .collect();
    let edge_count: usize = coverage["edge_counts"]
        .as_object()
        .into_iter()
        .flat_map(|counts| counts.values())
        .filter_map(Value::as_u64)
        .map(|count| count as usize)
        .sum();
    let mut body = String::from(
        "<header class=\"hero\"><div><p class=\"eyebrow\">OFFLINE FILEMAKER BLUEPRINT</p><h1>Design inventory</h1><p>Search source objects, inspect preserved properties, and follow evidence-backed links.</p></div><span class=\"badge\">No network required</span></header>",
    );
    body.push_str(&format!(
        "<section class=\"metrics\" aria-label=\"Bundle summary\"><article><strong>{}</strong><span>objects</span></article><article><strong>{edge_count}</strong><span>typed edges</span></article><article><strong>{workflow_count}</strong><span>workflow candidates</span></article><article><strong>{diagnostic_count}</strong><span>diagnostics</span></article></section>",
        components.len()
    ));
    body.push_str(
        "<section class=\"panel source-panel\"><h2>Source exports</h2><div class=\"source-list\">",
    );
    for app in applications["applications"]
        .as_array()
        .into_iter()
        .flatten()
    {
        body.push_str(&format!(
            "<article><strong>{}</strong><span>FileMaker {}</span><span>Export {}</span><span>Encoding {}</span></article>",
            escape_html(app["file_name"].as_str().unwrap_or("unknown")),
            escape_html(app["filemaker_version"].as_str().unwrap_or("unknown")),
            escape_html(app["export_version"].as_str().unwrap_or("unknown")),
            escape_html(app["input_encoding"].as_str().unwrap_or("unknown"))
        ));
    }
    body.push_str("</div></section><section class=\"panel\"><div class=\"section-heading\"><div><h2>Objects</h2><p id=\"result-count\" aria-live=\"polite\"></p></div><a class=\"button-link\" href=\"coverage.html\">Coverage report</a></div><form class=\"filters\" data-filter-form><label>Search<input type=\"search\" name=\"search\" placeholder=\"Name, ID, source path…\" autocomplete=\"off\"></label>");
    select_options(&mut body, "kind", "Kind", kinds);
    select_options(&mut body, "source", "Source file", sources);
    select_options(&mut body, "action", "Source action", actions);
    body.push_str("<button type=\"reset\" class=\"secondary\">Clear filters</button></form><div class=\"table-wrap\"><table id=\"object-table\"><thead><tr><th>Name</th><th>Kind</th><th>Source</th><th>Action</th><th>Analysis</th></tr></thead><tbody>");
    for component in components {
        let status = component_status(component);
        let href = pages
            .by_record_id
            .get(component.record_id.as_str())
            .map(|path| path.as_str())
            .unwrap_or("#");
        body.push_str(&format!(
            "<tr data-filter-row data-search=\"{}\" data-kind=\"{}\" data-source=\"{}\" data-action=\"{}\"><td><a href=\"{}\">{}</a><small>{}</small></td><td>{}</td><td>{}</td><td>{}</td><td><span class=\"status status-{}\">{}</span></td></tr>",
            escape_html(&format!("{} {} {}", component.name, component.object_id, component.source_path).to_lowercase()),
            escape_html(&component.kind),
            escape_html(&component.source_file),
            escape_html(&component.source_action),
            escape_html(href),
            escape_html(&display_name(component)),
            escape_html(&component.object_id),
            escape_html(&component.kind.replace('_', " ")),
            escape_html(&component.source_file),
            escape_html(&component.source_action),
            escape_html(status),
            escape_html(&status.replace('_', " "))
        ));
    }
    body.push_str("</tbody></table><p class=\"empty-state\" hidden>No objects match these filters.</p></div></section>");
    page("Design inventory", "index", &body)
}

fn select_options<'a>(
    html: &mut String,
    name: &str,
    label: &str,
    options: impl IntoIterator<Item = &'a str>,
) {
    html.push_str(&format!(
        "<label>{}<select name=\"{}\"><option value=\"\">All</option>",
        escape_html(label),
        escape_html(name)
    ));
    for option in options {
        html.push_str(&format!(
            "<option value=\"{}\">{}</option>",
            escape_html(option),
            escape_html(option)
        ));
    }
    html.push_str("</select></label>");
}

fn render_component(component: &Component, edges: &[&AnalysisEdge], pages: &Pages<'_>) -> String {
    let status = component_status(component);
    let mut body = format!(
        "<header class=\"hero\"><div><p class=\"eyebrow\">{}</p><h1>{}</h1><p class=\"mono\">{}</p></div><span class=\"status status-{}\">{}</span></header><section class=\"panel detail-meta\"><dl><dt>Source file</dt><dd>{}</dd><dt>Action</dt><dd>{}</dd><dt>XML path</dt><dd class=\"mono\">{}</dd><dt>Record ID</dt><dd class=\"mono\">{}</dd>",
        escape_html(&component.kind.replace('_', " ")),
        escape_html(&display_name(component)),
        escape_html(&component.object_id),
        escape_html(status),
        escape_html(&status.replace('_', " ")),
        escape_html(&component.source_file),
        escape_html(&component.source_action),
        escape_html(&component.source_path),
        escape_html(&component.record_id)
    );
    if let Some(table_name) = component.table_name.as_deref() {
        body.push_str(&format!(
            "<dt>Table</dt><dd>{}</dd>",
            escape_html(table_name)
        ));
    }
    body.push_str("</dl></section><section class=\"panel\"><h2>Properties</h2>");
    body.push_str(&properties_table(&component.properties));
    body.push_str("</section>");
    if !component.attached_records.is_empty() {
        body.push_str("<section class=\"panel\"><h2>Attached records</h2>");
        for record in &component.attached_records {
            body.push_str(&format!(
                "<details><summary>{} <span class=\"muted\">{} · step {}</span></summary><p class=\"mono\">{}</p>{}</details>",
                escape_html(&record.name),
                escape_html(&record.kind.replace('_', " ")),
                record.order,
                escape_html(&record.source_path),
                properties_table(&record.properties)
            ));
        }
        body.push_str("</section>");
    }
    if !component.findings.is_empty() {
        body.push_str(
            "<section class=\"panel\"><h2>Static analysis</h2><ul class=\"finding-list\">",
        );
        for finding in &component.findings {
            body.push_str(&format!(
                "<li><span class=\"status status-{}\">{}</span><strong>{}</strong><p>{}</p>{}</li>",
                escape_html(&finding.status),
                escape_html(&finding.status.replace('_', " ")),
                escape_html(&finding.kind.replace('_', " ")),
                escape_html(&finding.summary),
                render_string_list(
                    "Evidence and limits",
                    finding.evidence.iter().chain(&finding.limitations)
                )
            ));
        }
        body.push_str("</ul></section>");
    }
    if !component.diagnostics.is_empty() {
        body.push_str("<section class=\"panel\"><h2>Diagnostics</h2><ul>");
        for diagnostic in &component.diagnostics {
            body.push_str(&format!("<li>{}</li>", escape_html(diagnostic)));
        }
        body.push_str("</ul></section>");
    }
    if !edges.is_empty() {
        body.push_str("<section class=\"panel\"><h2>Relationships</h2><div class=\"edge-list\">");
        for edge in edges {
            let from = linked_object(edge.from_object_id.as_deref(), pages, true);
            let to = linked_object(edge.to_object_id.as_deref(), pages, true);
            body.push_str(&format!(
                "<article><div class=\"edge-title\">{} <span class=\"status status-{}\">{}</span></div><p>{} <span aria-hidden=\"true\">→</span> {}</p>{}{}</article>",
                escape_html(&edge.kind.replace('_', " ")),
                escape_html(&edge.status),
                escape_html(&edge.status.replace('_', " ")),
                from,
                to,
                render_string_list("Evidence", &edge.evidence),
                render_string_list("Limitations", &edge.limitations)
            ));
        }
        body.push_str("</div></section>");
    }
    body.push_str("<footer class=\"disclaimer\">Static evidence only. This page does not establish runtime behavior or merge source actions.</footer>");
    page(&display_name(component), "../index", &body)
}

fn properties_table(node: &crate::components::XmlNode) -> String {
    let mut properties = Vec::new();
    render_properties(node, "", &mut properties);
    if properties.is_empty() {
        return "<p class=\"muted\">No properties were present in this export.</p>".into();
    }
    let mut html = String::from(
        "<div class=\"table-wrap\"><table><thead><tr><th>Property</th><th>Value</th></tr></thead><tbody>",
    );
    for (name, value) in properties {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td></tr>",
            escape_html(&name),
            escape_html(&value)
        ));
    }
    html.push_str("</tbody></table></div>");
    html
}

fn render_workflows(workflows: &[WorkflowCandidate], pages: &Pages<'_>) -> String {
    let mut body = String::from(
        "<header class=\"hero\"><div><p class=\"eyebrow\">STATIC CANDIDATES</p><h1>Workflow candidates</h1><p>Reachability summaries from UI entries and script edges; execution order and branch outcomes are not evaluated.</p></div></header><section class=\"panel\"><form class=\"filters\" data-filter-form><label>Search<input type=\"search\" name=\"search\" placeholder=\"Workflow, source, object ID…\"></label>",
    );
    let statuses: BTreeSet<_> = workflows
        .iter()
        .map(|workflow| workflow.status.as_str())
        .collect();
    select_options(&mut body, "status", "Status", statuses);
    body.push_str(
        "</form><p id=\"result-count\" aria-live=\"polite\"></p><div class=\"workflow-list\">",
    );
    for (index, workflow) in workflows.iter().enumerate() {
        let entry = linked_object(Some(&workflow.entry_object_id), pages, false);
        let href = workflow_filename(index, workflow);
        let search = format!(
            "{} {} {} {}",
            workflow.entry_name,
            workflow.entry_object_id,
            workflow.source_file,
            workflow.entry_kind
        )
        .to_lowercase();
        body.push_str(&format!(
            "<article data-filter-row data-search=\"{}\" data-status=\"{}\"><div class=\"section-heading\"><div><h2><a href=\"{}\">{}</a></h2><span class=\"status status-{}\">{}</span></div><span>{}</span></div><p>Entry: {}</p><p>{} reachable script(s) · {} read fields · {} write fields</p>{}{}</article>",
            escape_html(&search),
            escape_html(&workflow.status),
            escape_html(&href),
            escape_html(&workflow.entry_name),
            escape_html(&workflow.status),
            escape_html(&workflow.status.replace('_', " ")),
            escape_html(&workflow.source_file),
            entry,
            workflow.script_ids.len(),
            workflow.field_read_ids.len(),
            workflow.field_write_ids.len(),
            render_string_list("Effects", &workflow.effects),
            render_string_list("Limitations", &workflow.limitations)
        ));
    }
    body.push_str(
        "</div><p class=\"empty-state\" hidden>No workflows match these filters.</p></section>",
    );
    page("Workflow candidates", "index", &body)
}

fn workflow_filename(index: usize, workflow: &WorkflowCandidate) -> String {
    format!(
        "workflows/{:05}-{}.html",
        index + 1,
        slug(&workflow.entry_name)
    )
}

#[derive(Debug)]
struct JoinGroup {
    source_id: String,
    action: String,
    relationship_id: String,
    from_id: String,
    to_id: Option<String>,
    edges: Vec<AnalysisEdge>,
}

type JoinGroupKey = (String, String, String, String, Option<String>);

fn group_join_edges(
    edges: &[AnalysisEdge],
    action_by_record: &BTreeMap<String, String>,
) -> Vec<JoinGroup> {
    let mut grouped: BTreeMap<JoinGroupKey, Vec<AnalysisEdge>> = BTreeMap::new();
    for edge in edges.iter().filter(|edge| edge.kind == "joins_to") {
        let (Some(from), Some(record_id)) = (
            edge.from_object_id.as_deref(),
            edge.from_record_id.as_deref(),
        ) else {
            continue;
        };
        let source_id = from.split(':').next().unwrap_or("unknown").to_string();
        let action = action_by_record
            .get(record_id)
            .cloned()
            .unwrap_or_else(|| "Unknown".into());
        grouped
            .entry((
                source_id,
                action,
                record_id.to_string(),
                from.to_string(),
                edge.to_object_id.clone(),
            ))
            .or_default()
            .push(edge.clone());
    }
    grouped
        .into_iter()
        .map(
            |((source_id, action, relationship_id, from_id, to_id), edges)| JoinGroup {
                source_id,
                action,
                relationship_id,
                from_id,
                to_id,
                edges,
            },
        )
        .collect()
}

fn render_relationships(
    components: &[Component],
    edges: &[AnalysisEdge],
    pages: &Pages<'_>,
) -> String {
    let action_by_record: BTreeMap<_, _> = components
        .iter()
        .map(|component| (component.record_id.clone(), component.source_action.clone()))
        .collect();
    let groups = group_join_edges(edges, &action_by_record);
    let mut nodes_by_source: BTreeMap<String, BTreeMap<String, &Component>> = BTreeMap::new();
    for component in components
        .iter()
        .filter(|component| component.kind == "table_occurrence")
    {
        let source_id = component.object_id.split(':').next().unwrap_or("unknown");
        let nodes = nodes_by_source.entry(source_id.to_string()).or_default();
        if nodes.get(&component.object_id).is_none_or(|current| {
            component.source_action == "AddAction" && current.source_action != "AddAction"
        }) {
            nodes.insert(component.object_id.clone(), component);
        }
    }
    let source_ids: BTreeSet<_> = nodes_by_source
        .keys()
        .chain(groups.iter().map(|group| &group.source_id))
        .cloned()
        .collect();
    let actions: BTreeSet<_> = groups.iter().map(|group| group.action.as_str()).collect();
    let default_action = if actions.contains("AddAction") {
        "AddAction"
    } else {
        ""
    };
    let mut body = String::from(
        "<header class=\"hero\"><div><p class=\"eyebrow\">EXPLICIT JOIN PREDICATES</p><h1>Table relationships</h1><p>Each line represents a recorded <code>joins_to</code> reference. Source actions are kept separate; disconnected table occurrences remain visible.</p></div></header><section class=\"panel\"><form class=\"filters\" data-relationship-filter><label>FileMaker source<select id=\"relationship-source\">",
    );
    for (index, source_id) in source_ids.iter().enumerate() {
        let source_file = nodes_by_source
            .get(source_id)
            .and_then(|nodes| nodes.values().next())
            .map(|component| component.source_file.as_str())
            .unwrap_or("source with unresolved endpoints");
        body.push_str(&format!(
            "<option value=\"{}\"{}>{} · {}</option>",
            escape_html(source_id),
            if index == 0 { " selected" } else { "" },
            escape_html(source_file),
            escape_html(source_id)
        ));
    }
    body.push_str("</select></label><label>Source action<select id=\"relationship-action\"><option value=\"\">All actions</option>");
    for action in actions {
        body.push_str(&format!(
            "<option value=\"{}\"{}>{}</option>",
            escape_html(action),
            if action == default_action {
                " selected"
            } else {
                ""
            },
            escape_html(action)
        ));
    }
    body.push_str(
        "</select></label><span class=\"muted\">Drag to pan · scroll to zoom</span></form>",
    );
    for (index, source_id) in source_ids.iter().enumerate() {
        let nodes = nodes_by_source.get(source_id).cloned().unwrap_or_default();
        let source_groups: Vec<_> = groups
            .iter()
            .filter(|group| group.source_id == *source_id)
            .collect();
        body.push_str(&format!(
            "<div class=\"relationship-source\" data-source-id=\"{}\"{}>",
            escape_html(source_id),
            if index == 0 { "" } else { " hidden" }
        ));
        body.push_str(&render_relationship_svg(
            source_id,
            &nodes,
            &source_groups,
            pages,
        ));
        body.push_str("<h2>Join predicates</h2><div class=\"edge-list\">");
        for group in &source_groups {
            let from = linked_object(Some(&group.from_id), pages, false);
            let to = linked_object(group.to_id.as_deref(), pages, false);
            body.push_str(&format!(
                "<article data-relationship-action=\"{}\"><div class=\"edge-title\">{} <span class=\"status status-{}\">{}</span></div><p>{} → {}</p><p>Relationship record: <span class=\"mono\">{}</span></p>{}{}",
                escape_html(&group.action),
                escape_html(&group.action),
                if group.to_id.is_some() { "explicit" } else { "unresolved" },
                if group.to_id.is_some() { "resolved" } else { "unresolved" },
                from,
                to,
                escape_html(&group.relationship_id),
                render_string_list("Predicates and evidence", group.edges.iter().flat_map(|edge| &edge.evidence)),
                render_string_list("Limitations", group.edges.iter().flat_map(|edge| &edge.limitations))
            ));
            body.push_str("</article>");
        }
        body.push_str("</div></div>");
    }
    if source_ids.is_empty() {
        body.push_str(
            "<p class=\"muted\">No table occurrences or explicit join edges were found.</p>",
        );
    }
    body.push_str("</section>");
    page("Table relationships", "index", &body)
}

fn render_relationship_svg(
    source_id: &str,
    nodes: &BTreeMap<String, &Component>,
    groups: &[&JoinGroup],
    pages: &Pages<'_>,
) -> String {
    let radius = (nodes.len() as f64 * 19.0).max(220.0);
    let width = radius * 2.0 + 240.0;
    let height = width;
    let center = width / 2.0;
    let positions: BTreeMap<_, _> = nodes
        .keys()
        .enumerate()
        .map(|(index, id)| {
            let angle = std::f64::consts::TAU * index as f64 / nodes.len().max(1) as f64;
            (
                id.as_str(),
                (center + radius * angle.cos(), center + radius * angle.sin()),
            )
        })
        .collect();
    let marker = format!("arrow-{}", slug(source_id));
    let mut svg = format!(
        "<div class=\"diagram-scroll\"><svg class=\"diagram-svg\" tabindex=\"0\" role=\"group\" aria-label=\"Table occurrence relationship graph\" viewBox=\"0 0 {width:.0} {height:.0}\" xmlns=\"http://www.w3.org/2000/svg\"><title>Table occurrence relationship graph</title><defs><marker id=\"{marker}\" markerWidth=\"8\" markerHeight=\"8\" refX=\"7\" refY=\"3\" orient=\"auto\"><path d=\"M0,0 L0,6 L8,3 z\" fill=\"#52758a\"/></marker></defs><g class=\"diagram-content\">"
    );
    for group in groups {
        let Some(to_id) = group.to_id.as_deref() else {
            continue;
        };
        let (Some((x1, y1)), Some((x2, y2))) =
            (positions.get(group.from_id.as_str()), positions.get(to_id))
        else {
            continue;
        };
        svg.push_str(&format!(
            "<g class=\"diagram-edge\" data-action=\"{}\"><title>{}</title><line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" marker-end=\"url(#{marker})\"/><text x=\"{:.1}\" y=\"{:.1}\">{} predicate(s)</text></g>",
            escape_html(&group.action),
            escape_html(&group.edges.iter().flat_map(|edge| &edge.evidence).cloned().collect::<Vec<_>>().join("; ")),
            (x1 + x2) / 2.0,
            (y1 + y2) / 2.0,
            group.edges.len()
        ));
    }
    for (id, component) in nodes {
        let Some((x, y)) = positions.get(id.as_str()) else {
            continue;
        };
        let label = display_name(component);
        let base_table = find_xml_node(&component.properties, "BaseTableReference")
            .and_then(|node| node.attributes.get("name"))
            .map(String::as_str)
            .unwrap_or("base table unavailable");
        let href = pages.for_object(id).unwrap_or("#");
        svg.push_str(&format!(
            "<a href=\"{}\"><g class=\"diagram-node\" transform=\"translate({:.1} {:.1})\"><rect x=\"-90\" y=\"-31\" width=\"180\" height=\"62\" rx=\"10\"/><text y=\"-5\">{}</text><text class=\"node-subtitle\" y=\"16\">{}</text><title>{} · {} · {}</title></g></a>",
            escape_html(href), x, y, escape_html(&truncate(&label, 24)),
            escape_html(&truncate(base_table, 26)), escape_html(&label), escape_html(base_table), escape_html(id)
        ));
    }
    svg.push_str("</g></svg></div>");
    svg
}

fn find_xml_node<'a>(
    node: &'a crate::components::XmlNode,
    name: &str,
) -> Option<&'a crate::components::XmlNode> {
    if node.name == name {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_xml_node(child, name))
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let text: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{text}…")
    } else {
        text
    }
}

#[derive(Clone)]
struct WorkflowNode {
    id: String,
    name: String,
    kind: String,
    column: usize,
}

fn render_workflow_detail(
    workflow: &WorkflowCandidate,
    components: &[Component],
    edges: &[AnalysisEdge],
    pages: &Pages<'_>,
) -> String {
    let mut object_map = BTreeMap::new();
    for component in components {
        if object_map
            .get(component.object_id.as_str())
            .is_none_or(|current: &&Component| {
                component.source_action == "AddAction" && current.source_action != "AddAction"
            })
        {
            object_map.insert(component.object_id.as_str(), component);
        }
    }
    let mut nodes = BTreeMap::<String, WorkflowNode>::new();
    let mut unresolved_ids = BTreeSet::new();
    let mut add = |id: &str, kind: &str, column: usize| {
        let Some(component) = object_map.get(id) else {
            unresolved_ids.insert(id.to_string());
            return;
        };
        nodes.entry(id.to_string()).or_insert(WorkflowNode {
            id: id.to_string(),
            name: display_name(component),
            kind: kind.to_string(),
            column,
        });
    };
    add(&workflow.entry_object_id, &workflow.entry_kind, 0);
    for id in &workflow.script_ids {
        add(id, "script", 1);
    }
    for id in &workflow.layout_ids {
        add(id, "layout", 2);
    }
    for id in workflow
        .field_read_ids
        .iter()
        .chain(&workflow.field_write_ids)
    {
        add(id, "field", 3);
    }
    for id in &workflow.table_occurrence_ids {
        add(id, "table occurrence", 4);
    }
    for id in &workflow.external_source_ids {
        add(id, "external source", 5);
    }
    drop(add);
    let workflow_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.kind.as_str(),
                "triggers_script"
                    | "is_triggered_by"
                    | "calls_script"
                    | "navigates_to"
                    | "reads_field"
                    | "writes_field"
                    | "uses_table_occurrence"
                    | "accesses_external_source"
            ) && edge
                .from_object_id
                .as_ref()
                .is_some_and(|id| nodes.contains_key(id))
                && edge
                    .to_object_id
                    .as_ref()
                    .is_some_and(|id| nodes.contains_key(id))
        })
        .collect();
    let max_per_column = (0..=5)
        .map(|column| nodes.values().filter(|node| node.column == column).count())
        .max()
        .unwrap_or(1);
    let height = (max_per_column as f64 * 100.0 + 100.0).max(300.0);
    let width = 6.0 * 230.0 + 220.0;
    let mut positions = BTreeMap::new();
    let mut rows = [0usize; 6];
    for node in nodes.values() {
        let row = rows[node.column];
        rows[node.column] += 1;
        positions.insert(
            node.id.as_str(),
            (
                100.0 + node.column as f64 * 230.0,
                60.0 + row as f64 * 100.0,
            ),
        );
    }
    let mut body = format!(
        "<header class=\"hero\"><div><p class=\"eyebrow\">WORKFLOW CANDIDATE</p><h1>{}</h1><p>{} · Entry {}</p></div><span class=\"status status-{}\">{}</span></header><section class=\"panel\"><p class=\"diagram-note\">This static graph shows typed links among candidate objects. Placement is for readability only; it does not represent runtime sequence or execution.</p>",
        escape_html(&workflow.entry_name),
        escape_html(&workflow.source_file),
        linked_workflow_object(Some(&workflow.entry_object_id), pages),
        escape_html(&workflow.status),
        escape_html(&workflow.status.replace('_', " "))
    );
    body.push_str(&format!("<div class=\"diagram-scroll\"><svg class=\"diagram-svg\" tabindex=\"0\" role=\"group\" aria-label=\"Static workflow candidate graph\" viewBox=\"0 0 {width:.0} {height:.0}\" xmlns=\"http://www.w3.org/2000/svg\"><title>Static workflow candidate graph</title><defs><marker id=\"workflow-arrow\" markerWidth=\"8\" markerHeight=\"8\" refX=\"7\" refY=\"3\" orient=\"auto\"><path d=\"M0,0 L0,6 L8,3 z\" fill=\"#52758a\"/></marker></defs><g class=\"diagram-content\">"));
    for edge in &workflow_edges {
        let (Some(from_id), Some(to_id)) =
            (edge.from_object_id.as_deref(), edge.to_object_id.as_deref())
        else {
            continue;
        };
        let ((x1, y1), (x2, y2)) = (positions[from_id], positions[to_id]);
        body.push_str(&format!("<g class=\"diagram-edge status-{}\"><title>{}</title><line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" marker-end=\"url(#workflow-arrow)\"/><text x=\"{:.1}\" y=\"{:.1}\">{}</text></g>", escape_html(&edge.status), escape_html(&edge.evidence.join("; ")), x1+90.0,y1+28.0,x2-90.0,y2+28.0,(x1+x2)/2.0,(y1+y2)/2.0, escape_html(&edge.kind.replace('_', " "))));
    }
    for node in nodes.values() {
        let (x, y) = positions[node.id.as_str()];
        let href = pages
            .for_object(&node.id)
            .map(|path| format!("../{path}"))
            .unwrap_or_else(|| "#".into());
        body.push_str(&format!("<a href=\"{}\"><g class=\"diagram-node kind-{}\" transform=\"translate({x:.1} {y:.1})\"><rect width=\"180\" height=\"58\" rx=\"9\"/><text x=\"90\" y=\"23\">{}</text><text class=\"node-subtitle\" x=\"90\" y=\"43\">{}</text><title>{} · {}</title></g></a>", escape_html(&href), slug(&node.kind), escape_html(&truncate(&node.name, 23)), escape_html(&node.kind), escape_html(&node.name), escape_html(&node.id)));
    }
    body.push_str("</g></svg></div><h2>Evidence-backed edges</h2><div class=\"edge-list\">");
    for edge in workflow_edges {
        body.push_str(&format!("<article><div class=\"edge-title\">{} <span class=\"status status-{}\">{}</span></div><p>{} → {}</p>{}{}", escape_html(&edge.kind.replace('_', " ")), escape_html(&edge.status), escape_html(&edge.status.replace('_', " ")), linked_workflow_object(edge.from_object_id.as_deref(), pages), linked_workflow_object(edge.to_object_id.as_deref(), pages), render_string_list("Evidence", &edge.evidence), render_string_list("Limitations", &edge.limitations)));
        body.push_str("</article>");
    }
    body.push_str("</div>");
    let unresolved_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.kind.as_str(),
                "triggers_script"
                    | "is_triggered_by"
                    | "calls_script"
                    | "navigates_to"
                    | "reads_field"
                    | "writes_field"
                    | "uses_table_occurrence"
                    | "accesses_external_source"
            ) && edge
                .from_object_id
                .as_ref()
                .is_some_and(|id| nodes.contains_key(id))
                && edge
                    .to_object_id
                    .as_ref()
                    .is_none_or(|id| !object_map.contains_key(id.as_str()))
        })
        .collect();
    if !unresolved_ids.is_empty() || !unresolved_edges.is_empty() {
        body.push_str("<details class=\"unresolved-warning\"><summary>Unresolved workflow references</summary><ul>");
        for id in &unresolved_ids {
            body.push_str(&format!("<li>Candidate object reference has no matching exported object: <span class=\"mono\">{}</span></li>", escape_html(id)));
        }
        for edge in unresolved_edges {
            body.push_str(&format!(
                "<li>{}: target is unresolved<ul>{}</ul></li>",
                escape_html(&edge.kind.replace('_', " ")),
                edge.evidence
                    .iter()
                    .map(|item| format!("<li class=\"mono\">{}</li>", escape_html(item)))
                    .collect::<String>()
            ));
        }
        body.push_str("</ul></details>");
    }
    if !workflow.effects.is_empty() {
        body.push_str(&render_string_list("Candidate effects", &workflow.effects));
    }
    if !workflow.evidence.is_empty() {
        body.push_str(&render_string_list("Workflow evidence", &workflow.evidence));
    }
    if !workflow.limitations.is_empty() {
        body.push_str(&render_string_list(
            "Limitations and unresolved references",
            &workflow.limitations,
        ));
    }
    body.push_str("</section>");
    page(&workflow.entry_name, "../index", &body)
}

fn render_diagnostics(diagnostics: &[Diagnostic], pages: &Pages<'_>) -> String {
    let mut body = String::from(
        "<header class=\"hero\"><div><p class=\"eyebrow\">UNCERTAINTY REGISTER</p><h1>Diagnostics</h1><p>Unresolved references and heuristic findings are leads for review, not verified application behavior.</p></div></header><section class=\"panel\"><form class=\"filters\" data-filter-form><label>Search<input type=\"search\" name=\"search\" placeholder=\"Finding, ID, evidence…\"></label>",
    );
    let statuses: BTreeSet<_> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.status.as_str())
        .collect();
    select_options(&mut body, "status", "Status", statuses);
    body.push_str(
        "</form><p id=\"result-count\" aria-live=\"polite\"></p><div class=\"diagnostic-list\">",
    );
    for diagnostic in diagnostics {
        let search = format!(
            "{} {} {} {}",
            diagnostic.kind,
            diagnostic.summary,
            diagnostic.source_object_id.as_deref().unwrap_or_default(),
            diagnostic.target_object_id.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        let source = linked_object(diagnostic.source_object_id.as_deref(), pages, false);
        let target = linked_object(diagnostic.target_object_id.as_deref(), pages, false);
        body.push_str(&format!(
            "<details data-filter-row data-search=\"{}\" data-status=\"{}\"><summary><span class=\"status status-{}\">{}</span> {} <small>{}</small></summary><p>{}</p><p>Source: {} · Target: {}</p>{}{}</details>",
            escape_html(&search),
            escape_html(&diagnostic.status),
            escape_html(&diagnostic.status),
            escape_html(&diagnostic.status.replace('_', " ")),
            escape_html(&diagnostic.summary),
            escape_html(&diagnostic.id),
            escape_html(&diagnostic.kind.replace('_', " ")),
            source,
            target,
            render_string_list("Evidence", &diagnostic.evidence),
            render_string_list("Limitations", &diagnostic.limitations)
        ));
    }
    body.push_str(
        "</div><p class=\"empty-state\" hidden>No diagnostics match these filters.</p></section>",
    );
    page("Diagnostics", "index", &body)
}

fn render_coverage(coverage: &Value) -> String {
    let mut body = String::from(
        "<header class=\"hero\"><div><p class=\"eyebrow\">OBSERVED SOURCE COVERAGE</p><h1>Coverage and limits</h1><p>Counts represent exported records and static findings, not final application state.</p></div></header><section class=\"panel\"><h2>Components</h2>",
    );
    render_counts(&mut body, &coverage["component_counts"]);
    body.push_str("</section><section class=\"panel\"><h2>Edges</h2>");
    render_counts(&mut body, &coverage["edge_counts"]);
    body.push_str("</section><section class=\"panel\"><h2>Edge statuses</h2>");
    render_counts(&mut body, &coverage["edge_status_counts"]);
    body.push_str("</section><section class=\"panel\"><h2>Known limits</h2><ul>");
    for item in coverage["limitations"].as_array().into_iter().flatten() {
        body.push_str(&format!(
            "<li><span class=\"status status-{}\">{}</span><strong>{}</strong><p>{}</p></li>",
            escape_html(item["status"].as_str().unwrap_or("uncertain")),
            escape_html(item["status"].as_str().unwrap_or("uncertain")),
            escape_html(item["feature"].as_str().unwrap_or("unknown")),
            escape_html(item["description"].as_str().unwrap_or(""))
        ));
    }
    body.push_str("</ul></section>");
    page("Coverage", "index", &body)
}

fn render_counts(html: &mut String, value: &Value) {
    html.push_str("<div class=\"count-grid\">");
    if let Some(counts) = value.as_object() {
        for (name, count) in counts {
            let count = count.as_u64().unwrap_or_default();
            html.push_str(&format!(
                "<article><strong>{count}</strong><span>{}</span></article>",
                escape_html(&name.replace('_', " "))
            ));
        }
    }
    html.push_str("</div>");
}

fn linked_object(id: Option<&str>, pages: &Pages<'_>, from_object_page: bool) -> String {
    let Some(id) = id else {
        return "<span class=\"muted\">unresolved</span>".into();
    };
    let label = pages
        .by_object_id
        .get(id)
        .map(|component| display_name(component))
        .unwrap_or_else(|| id.to_string());
    match pages.for_object(id) {
        Some(path) => {
            let href = if from_object_page {
                path.strip_prefix("objects/").unwrap_or(path)
            } else {
                path
            };
            format!(
                "<a href=\"{}\">{} <small class=\"mono\">{}</small></a>",
                escape_html(href),
                escape_html(&label),
                escape_html(id)
            )
        }
        None => format!(
            "<span>{} <small class=\"mono\">{}</small></span>",
            escape_html(&label),
            escape_html(id)
        ),
    }
}

fn linked_workflow_object(id: Option<&str>, pages: &Pages<'_>) -> String {
    let Some(id) = id else {
        return "<span class=\"muted\">unresolved</span>".into();
    };
    let label = pages
        .by_object_id
        .get(id)
        .map(|component| display_name(component))
        .unwrap_or_else(|| id.to_string());
    match pages.for_object(id) {
        Some(path) => format!(
            "<a href=\"../{}\">{} <small class=\"mono\">{}</small></a>",
            escape_html(path),
            escape_html(&label),
            escape_html(id)
        ),
        None => format!(
            "<span>{} <small class=\"mono\">{}</small></span>",
            escape_html(&label),
            escape_html(id)
        ),
    }
}

fn render_string_list<'a>(title: &str, values: impl IntoIterator<Item = &'a String>) -> String {
    let values: Vec<_> = values.into_iter().collect();
    if values.is_empty() {
        return String::new();
    }
    let mut html = format!(
        "<details class=\"evidence\"><summary>{} ({})</summary><ul>",
        escape_html(title),
        values.len()
    );
    for value in values {
        html.push_str(&format!("<li class=\"mono\">{}</li>", escape_html(value)));
    }
    html.push_str("</ul></details>");
    html
}

fn component_status(component: &Component) -> &'static str {
    if component
        .findings
        .iter()
        .any(|finding| finding.status == "unresolved")
    {
        "unresolved"
    } else if component
        .findings
        .iter()
        .any(|finding| finding.status == "heuristic_candidate")
    {
        "heuristic_candidate"
    } else {
        "documented"
    }
}

fn display_name(component: &Component) -> String {
    if component.name.trim().is_empty() {
        format!(
            "{} {}",
            component.kind.replace('_', " "),
            component.xml_id.as_deref().unwrap_or("unnamed")
        )
    } else {
        component.name.clone()
    }
}

fn page(title: &str, home: &str, body: &str) -> String {
    let parent = if home == "../index" { "../" } else { "" };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'self'; script-src 'self'; img-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'\"><title>{} · FileMaker Blueprint</title><link rel=\"stylesheet\" href=\"{}style.css\"></head><body><nav class=\"topbar\"><a class=\"brand\" href=\"{}index.html\"><span class=\"brand-mark\">F</span> FileMaker Blueprint</a><div><a href=\"{}index.html\">Objects</a><a href=\"{}relationships.html\">Relationships</a><a href=\"{}workflows.html\">Workflows</a><a href=\"{}diagnostics.html\">Diagnostics</a><a href=\"{}coverage.html\">Coverage</a></div></nav><main>{}</main><script src=\"{}app.js\" defer></script></body></html>",
        escape_html(title),
        parent,
        parent,
        parent,
        parent,
        parent,
        parent,
        parent,
        body,
        parent
    )
}

fn slug(value: &str) -> String {
    let mut slug = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "unnamed".into()
    } else {
        slug.into()
    }
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        escaped.push_str(match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => {
                escaped.push(character);
                continue;
            }
        });
    }
    escaped
}

const STYLE: &str = r#"* {
    box-sizing: border-box
}
[hidden] {
    display: none!important
}
body {
    margin: 0;
    background: #f3f6fa;
    color: #172b45;
    font: 15px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif
}
.topbar {
    height: 68px;
    background: #fff;
    border-bottom: 1px solid #dce4ee;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 max(24px,calc((100vw - 1280px)/2));
    position: sticky;
    top: 0;
    z-index: 5
}
.topbar a {
    color: #435a75;
    text-decoration: none;
    margin-left: 20px
}
.topbar .brand {
    margin: 0;
    color: #183859;
    font-weight: 750;
    display: flex;
    gap: 10px;
    align-items: center
}
.brand-mark {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    background: #116d71;
    color: #fff
}
main {
    max-width: 1280px;
    margin: 0 auto;
    padding: 34px 24px 72px
}
.hero {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 30px;
    padding: 10px 0 24px
}
.hero h1 {
    margin: 3px 0;
    font-size: clamp(2rem,4vw,3rem);
    line-height: 1.1;
    letter-spacing: -.04em;
    color: #142f4d
}
.hero p {
    max-width: 760px;
    color: #62758d;
    margin: 8px 0
}
.eyebrow {
    font-size: .72rem;
    letter-spacing: .14em;
    font-weight: 800;
    color: #0b7477!important
}
.badge,.status {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 4px 10px;
    font-size: .76rem;
    font-weight: 700;
    background: #e8eef5;
    color: #485e78;
    white-space: nowrap
}
.status-explicit,.status-documented,.status-structurally-derived {
    background: #e0f3ed;
    color: #176552
}
.status-heuristic_candidate {
    background: #fff1d6;
    color: #825200
}
.status-unresolved,.status-partially_unresolved {
    background: #ffebe5;
    color: #983d27
}
.panel {
    background: #fff;
    border: 1px solid #dfe6ef;
    border-radius: 16px;
    padding: 24px;
    margin: 20px 0;
    box-shadow: 0 8px 24px #1d385009
}
.panel h2 {
    margin: 0 0 14px;
    color: #1d3a5b;
    font-size: 1.2rem
}
.metrics,.count-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit,minmax(150px,1fr));
    gap: 12px;
    margin: 6px 0 22px
}
.metrics article,.count-grid article {
    display: flex;
    flex-direction: column;
    padding: 18px;
    border-radius: 13px;
    background: #eaf2f5
}
.metrics strong,.count-grid strong {
    font-size: 1.7rem;
    color: #115e68
}
.metrics span,.count-grid span {
    color: #61758d;
    text-transform: capitalize
}
.source-list {
    display: grid;
    grid-template-columns: repeat(auto-fit,minmax(220px,1fr));
    gap: 12px
}
.source-list article {
    display: flex;
    flex-direction: column;
    padding: 14px;
    border: 1px solid #e4eaf0;
    border-radius: 10px;
    color: #687b90
}
.source-list strong {
    color: #244663
}
.section-heading {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 18px
}
.section-heading h2 {
    margin-bottom: 2px
}
.section-heading p {
    margin: 4px 0;
    color: #6b7d91
}
.button-link,button {
    border: 0;
    border-radius: 9px;
    background: #116d71;
    color: white;
    text-decoration: none;
    font: inherit;
    font-weight: 700;
    padding: 10px 14px;
    cursor: pointer
}
.filters {
    display: flex;
    align-items: end;
    gap: 12px;
    flex-wrap: wrap;
    margin: 20px 0
}
.filters label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-weight: 700;
    color: #52677f
}
.filters input,.filters select {
    font: inherit;
    color: #183653;
    border: 1px solid #cbd6e2;
    background: #fff;
    border-radius: 8px;
    padding: 10px;
    min-width: 175px
}
.filters input[type=search] {
    width: min(440px,70vw)
}
button[type=reset] {
    background: #edf2f7;
    color: #435a75
}
.table-wrap {
    overflow: auto
}
table {
    border-collapse: collapse;
    width: 100%;
    font-size: .92rem
}
th {
    text-align: left;
    background: #edf3f7;
    color: #47607b;
    font-size: .78rem;
    text-transform: uppercase;
    letter-spacing: .06em
}
td,th {
    padding: 11px 12px;
    border-bottom: 1px solid #e8edf2;
    vertical-align: top
}
td a {
    color: #146c70;
    font-weight: 700;
    text-decoration: none
}
td a:hover,.topbar a:hover {
    text-decoration: underline
}
td small {
    display: block;
    color: #8190a2;
    font: 11px ui-monospace,monospace;
    max-width: 300px;
    overflow-wrap: anywhere
}
.empty-state {
    padding: 24px;
    color: #71839a;
    text-align: center
}
.detail-meta dl {
    display: grid;
    grid-template-columns: 140px 1fr;
    gap: 8px 14px;
    margin: 0
}
.detail-meta dt {
    font-weight: 700;
    color: #58708b
}
.detail-meta dd {
    margin: 0;
    overflow-wrap: anywhere
}
.mono {
    font: 12px/1.55 ui-monospace,SFMono-Regular,Consolas,monospace;
    overflow-wrap: anywhere;
    color: #506984
}
details {
    border-top: 1px solid #e5ebf1;
    padding: 12px 2px
}
summary {
    cursor: pointer;
    color: #294b69;
    font-weight: 700
}
summary:focus-visible,a:focus-visible,button:focus-visible,input:focus-visible,select:focus-visible {
    outline: 3px solid #e6a747;
    outline-offset: 2px
}
.muted {
    color: #8493a4
}
.finding-list,.edge-list,.workflow-list,.diagnostic-list {
    display: grid;
    gap: 12px;
    padding: 0;
    list-style: none
}
.finding-list li,.edge-list article,.workflow-list article {
    border: 1px solid #e2e9f0;
    border-radius: 12px;
    padding: 16px
}
.finding-list strong {
    margin-left: 8px
}
.finding-list p {
    margin: 8px 0;
    color: #52677f
}
.edge-title {
    font-weight: 750;
    color: #234765;
    text-transform: capitalize
}
.edge-list article>p {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap
}
.edge-list a,.diagnostic-list a {
    color: #126d70;
    text-decoration: none;
    font-weight: 700
}
.evidence {
    margin-top: 8px
}
.evidence summary {
    font-size: .84rem
}
.evidence ul {
    padding-left: 20px;
    color: #657990
}
.disclaimer {
    padding: 16px 0;
    color: #71839a;
    border-top: 1px solid #dce4ec;
    margin-top: 28px;
    font-size: .86rem
}
.diagnostic-list details {
    border: 1px solid #e2e9f0;
    border-radius: 11px;
    padding: 14px
}
.diagnostic-list summary small {
    float: right;
    color: #8996a5
}
.diagnostic-list details>p {
    color: #5a7088
}
.source-panel {
    padding-bottom: 20px
}
@media (max-width: 720px) {
    .topbar {
        height: auto;
        align-items: flex-start;
        flex-direction: column;
        padding: 14px 18px;
        gap: 10px
    }
    .topbar div {
        display: flex;
        flex-wrap: wrap;
        gap: 10px
    }
    .topbar a {
        margin: 0 12px 0 0
    }
    main {
        padding: 24px 14px 52px
    }
    .hero {
        align-items: flex-start;
        flex-direction: column
    }
    .panel {
        padding: 17px
    }
    .detail-meta dl {
        grid-template-columns: 1fr;
        gap: 2px
    }
    .detail-meta dd {
        margin-bottom: 10px
    }
    .filters label,.filters input,.filters select {
        width: 100%
    }
    .section-heading {
        align-items: flex-start;
        flex-direction: column
    }
}"#;

const STYLE_DIAGRAM: &str = r#".diagram-scroll {
    overflow: auto;
    border: 1px solid #dfe7ed;
    border-radius: 12px;
    background: #fbfdfe;
    margin: 14px 0 22px
}
.diagram-svg {
    display: block;
    width: max(100%,1100px);
    height: auto;
    touch-action: none;
    cursor: grab
}
.diagram-svg:active {
    cursor: grabbing
}
.diagram-node rect {
    fill: #e8f2f4;
    stroke: #5b8a91;
    stroke-width: 2
}
.diagram-node text {
    fill: #173b56;
    text-anchor: middle;
    font: 700 13px system-ui,sans-serif
}
.diagram-node .node-subtitle {
    fill: #5e7389;
    font: 11px system-ui,sans-serif
}
.diagram-edge line {
    stroke: #52758a;
    stroke-width: 2;
    opacity: .72
}
.diagram-edge text {
    fill: #415d73;
    text-anchor: middle;
    font: 11px system-ui,sans-serif;
    paint-order: stroke;
    stroke: #fbfdfe;
    stroke-width: 4px;
    stroke-linejoin: round
}
.diagram-edge.status-heuristic_candidate line {
    stroke: #b17a18;
    stroke-dasharray: 5 4
}
.diagram-edge.status-unresolved line {
    stroke: #a44630;
    stroke-dasharray: 4 4
}
.diagram-node.kind-script rect {
    fill: #e8f3ec;
    stroke: #6a9b78
}
.diagram-node.kind-field rect {
    fill: #f4f0e5;
    stroke: #a39055
}
.diagram-node.kind-layout rect {
    fill: #e9eef8;
    stroke: #7189b4
}
.diagram-node.kind-table-occurrence rect {
    fill: #e9f3f2;
    stroke: #5b8a91
}
.diagram-note {
    padding: 12px 14px;
    border-left: 4px solid #d3a347;
    background: #fff8e8;
    color: #6b552f
}"#;

const APP_JS: &str = r#"(() => {
  const form = document.querySelector('[data-filter-form]');
  if (form) {
    const rows = [...document.querySelectorAll('[data-filter-row]')];
    const search = form.elements.namedItem('search');
    const kind = form.elements.namedItem('kind');
    const source = form.elements.namedItem('source');
    const action = form.elements.namedItem('action');
    const status = form.elements.namedItem('status');
    const count = document.getElementById('result-count');
    const empty = document.querySelector('.empty-state');
    const update = () => {
      const q = (search?.value || '').trim().toLowerCase();
      let visible = 0;
      for (const row of rows) {
        const match = (!q || row.dataset.search.includes(q)) && (!kind?.value || row.dataset.kind === kind.value) && (!source?.value || row.dataset.source === source.value) && (!action?.value || row.dataset.action === action.value) && (!status?.value || row.dataset.status === status.value);
        row.hidden = !match;
        if (match) visible++;
      }
      if (count) count.textContent = `${visible.toLocaleString()} of ${rows.length.toLocaleString()} shown`;
      if (empty) empty.hidden = visible !== 0;
    };
    form.addEventListener('input', update);
    form.addEventListener('change', update);
    form.addEventListener('reset', () => requestAnimationFrame(update));
    update();
  }

  const source = document.getElementById('relationship-source');
  const action = document.getElementById('relationship-action');
  const updateRelationships = () => {
    for (const graph of document.querySelectorAll('.relationship-source')) {
      graph.hidden = graph.dataset.sourceId !== source?.value;
      for (const edge of graph.querySelectorAll('[data-relationship-action], .diagram-edge[data-action]')) {
        edge.hidden = Boolean(action?.value) && edge.dataset.relationshipAction !== action.value && edge.dataset.action !== action.value;
      }
    }
  };
  source?.addEventListener('change', updateRelationships);
  action?.addEventListener('change', updateRelationships);
  updateRelationships();

  for (const svg of document.querySelectorAll('.diagram-svg')) {
    let drag;
    svg.addEventListener('pointerdown', event => {
      if (event.target.closest('a')) return;
      const box = svg.viewBox.baseVal;
      drag = { x: event.clientX, y: event.clientY, left: box.x, top: box.y, width: box.width, height: box.height };
      svg.setPointerCapture(event.pointerId);
    });
    svg.addEventListener('pointermove', event => {
      if (!drag) return;
      const bounds = svg.getBoundingClientRect();
      const box = svg.viewBox.baseVal;
      box.x = drag.left - (event.clientX - drag.x) * drag.width / bounds.width;
      box.y = drag.top - (event.clientY - drag.y) * drag.height / bounds.height;
    });
    svg.addEventListener('pointerup', () => { drag = undefined; });
    svg.addEventListener('pointercancel', () => { drag = undefined; });
    svg.addEventListener('wheel', event => {
      event.preventDefault();
      const box = svg.viewBox.baseVal;
      const factor = event.deltaY < 0 ? 0.9 : 1.1;
      const width = Math.max(180, Math.min(20000, box.width * factor));
      const height = Math.max(180, Math.min(20000, box.height * factor));
      box.x += (box.width - width) / 2;
      box.y += (box.height - height) / 2;
      box.width = width;
      box.height = height;
    }, { passive: false });
  }
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn join(from: &str, to: Option<&str>, record: &str, evidence: &str) -> AnalysisEdge {
        AnalysisEdge {
            kind: "joins_to".into(),
            status: if to.is_some() {
                "explicit"
            } else {
                "unresolved"
            }
            .into(),
            from_object_id: Some(from.into()),
            to_object_id: to.map(str::to_string),
            from_record_id: Some(record.into()),
            to_record_id: None,
            evidence: vec![evidence.into()],
            limitations: Vec::new(),
        }
    }

    #[test]
    fn join_groups_keep_predicates_actions_and_source_uuids_separate() {
        let edges = vec![
            join(
                "db-a:table_occurrence:a",
                Some("db-a:table_occurrence:b"),
                "rel-add",
                "predicate 1",
            ),
            join(
                "db-a:table_occurrence:a",
                Some("db-a:table_occurrence:b"),
                "rel-add",
                "predicate 2",
            ),
            join(
                "db-a:table_occurrence:a",
                Some("db-a:table_occurrence:b"),
                "rel-modify",
                "modified predicate",
            ),
            join(
                "db-a:table_occurrence:a",
                None,
                "rel-unresolved",
                "unresolved predicate",
            ),
            join(
                "db-b:table_occurrence:a",
                Some("db-b:table_occurrence:b"),
                "rel-add-other",
                "other source",
            ),
            AnalysisEdge {
                kind: "uses_table_occurrence".into(),
                status: "explicit".into(),
                from_object_id: Some("db-a:script:s".into()),
                to_object_id: Some("db-a:table_occurrence:a".into()),
                from_record_id: Some("rel-add".into()),
                to_record_id: None,
                evidence: Vec::new(),
                limitations: Vec::new(),
            },
        ];
        let actions = BTreeMap::from([
            ("rel-add".into(), "AddAction".into()),
            ("rel-modify".into(), "ModifyAction".into()),
            ("rel-unresolved".into(), "RemoveAction".into()),
            ("rel-add-other".into(), "AddAction".into()),
        ]);
        let groups = group_join_edges(&edges, &actions);
        assert_eq!(groups.len(), 4);
        assert!(
            groups
                .iter()
                .any(|group| group.action == "AddAction" && group.edges.len() == 2)
        );
        assert!(
            groups
                .iter()
                .any(|group| group.action == "ModifyAction" && group.edges.len() == 1)
        );
        assert!(groups.iter().any(|group| group.to_id.is_none()));
        assert_eq!(
            groups
                .iter()
                .map(|group| group.source_id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            2
        );
    }

    #[test]
    fn workflow_diagram_preserves_typed_edge_evidence_and_unresolved_warning() {
        let workflow: WorkflowCandidate = serde_json::from_value(serde_json::json!({
            "id":"w1", "status":"partially_unresolved", "source_file":"sample.fmp12",
            "entry_object_id":"db:layout:l", "entry_name":"Dashboard", "entry_kind":"layout",
            "script_ids":["db:script:s", "db:script:missing"], "layout_ids":[],
            "field_read_ids":[], "field_write_ids":[], "table_occurrence_ids":[],
            "external_source_ids":[], "effects":[], "evidence":["layout trigger"], "limitations":["Dynamic branch unresolved"]
        })).unwrap();
        let components: Vec<Component> = serde_json::from_value(serde_json::json!([
            {"record_id":"layout-rec","object_id":"db:layout:l","kind":"layout","name":"Dashboard","source_file":"sample.fmp12","source_action":"AddAction","source_path":"/Layout","xml_id":"1","xml_uuid":null,"table_id":null,"table_name":null,"parent_object_id":null,"parent_record_id":null,"properties":{"name":"Layout","attributes":{},"text":"","children":[]},"attached_records":[],"findings":[],"diagnostics":[]},
            {"record_id":"script-rec","object_id":"db:script:s","kind":"script","name":"Refresh","source_file":"sample.fmp12","source_action":"AddAction","source_path":"/Script","xml_id":"2","xml_uuid":null,"table_id":null,"table_name":null,"parent_object_id":null,"parent_record_id":null,"properties":{"name":"Script","attributes":{},"text":"","children":[]},"attached_records":[],"findings":[],"diagnostics":[]}
        ])).unwrap();
        let edges: Vec<AnalysisEdge> = serde_json::from_value(serde_json::json!([
            {"kind":"triggers_script","status":"heuristic_candidate","from_object_id":"db:layout:l","to_object_id":"db:script:s","from_record_id":null,"to_record_id":null,"evidence":["OnLayoutEnter"],"limitations":["Trigger condition not evaluated"]},
            {"kind":"calls_script","status":"unresolved","from_object_id":"db:script:s","to_object_id":null,"from_record_id":null,"to_record_id":null,"evidence":["target missing"],"limitations":[]}
        ])).unwrap();
        let pages = Pages::new(&components, &edges);
        let html = render_workflow_detail(&workflow, &components, &edges, &pages);
        assert!(html.contains("triggers script"));
        assert!(html.contains("heuristic_candidate"));
        assert!(html.contains("OnLayoutEnter"));
        assert!(html.contains("Unresolved workflow references"));
        assert!(!html.contains("db:script:missing</text>"));
        assert!(html.contains("Dynamic branch unresolved"));
    }
}
