use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File},
    io::{self, BufRead, BufReader, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
mod analysis;
mod components;
use analysis::{
    AnalysisEdge, WorkflowCandidate, analyze_calculation_features, analyze_component_references,
    analyze_scripts, discover_workflows,
};
use components::{Component, XmlNode, extract_data_model};
use quick_xml::{Reader, XmlVersion, encoding::DecodingReader, events::Event};
use serde::{Deserialize, Serialize};

const KNOWN_CATALOGS: &[&str] = &[
    "AccountsCatalog",
    "BaseDirectoryCatalog",
    "BaseTableCatalog",
    "CustomFunctionsCatalog",
    "CustomMenuCatalog",
    "CustomMenuSetCatalog",
    "ExtendedPrivilegesCatalog",
    "ExternalDataSourceCatalog",
    "FieldCatalog",
    "LayoutCatalog",
    "LibraryCatalog",
    "PrivilegeSetsCatalog",
    "RelationshipCatalog",
    "ScriptCatalog",
    "TableOccurrenceCatalog",
    "ThemeCatalog",
    "ValueListCatalog",
];

#[derive(Parser)]
#[command(
    name = "fmblueprint",
    about = "Analyze Claris FileMaker Save a Copy as XML exports"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Analyze {
        #[arg(value_name = "INPUT", num_args = 0..)]
        input: Vec<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Validate {
        #[arg(value_name = "BUNDLE")]
        bundle: PathBuf,
    },
    Context {
        #[arg(value_name = "BUNDLE")]
        bundle: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 250)]
        max_objects: usize,
    },
}

#[derive(Debug, Serialize)]
struct Inventory {
    schema_version: u32,
    source_format: String,
    applications: Vec<Application>,
    catalogs: Vec<Catalog>,
    known_catalogs_not_observed: Vec<String>,
    diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Application {
    file_name: Option<String>,
    uuid: Option<String>,
    export_version: Option<String>,
    filemaker_version: Option<String>,
    locale: Option<String>,
    input_encoding: String,
    ddr_info_present: bool,
}

#[derive(Debug, Default, Serialize)]
struct Catalog {
    source_file: String,
    action: String,
    name: String,
    occurrences: u64,
    declared_member_count: Option<u64>,
    observed_entries: u64,
}

struct CatalogScope {
    source_file: String,
    action: String,
    name: String,
    declared_member_count: Option<u64>,
    observed_entries: u64,
}

#[derive(Default)]
struct ParsedXml {
    application: Option<Application>,
    catalogs: BTreeMap<(String, String, String), Catalog>,
    ddr_info_present: bool,
    unknown_catalogs: Vec<String>,
}

#[derive(Serialize)]
struct ComponentExport<'a> {
    schema_version: u32,
    components: &'a [Component],
}

#[derive(Serialize)]
struct EdgeExport<'a> {
    schema_version: u32,
    edges: &'a [AnalysisEdge],
}

#[derive(Serialize)]
struct WorkflowExport<'a> {
    schema_version: u32,
    workflows: &'a [WorkflowCandidate],
}

#[derive(Deserialize, Serialize)]
struct ComponentPageManifest {
    schema_version: u32,
    pages: Vec<String>,
}

type GraphSelector = (&'static str, fn(&AnalysisEdge) -> bool);

struct ComponentIndexes<'a> {
    filenames: BTreeMap<&'a str, String>,
    by_id: BTreeMap<&'a str, &'a Component>,
    added_by_id: BTreeMap<&'a str, &'a Component>,
    fields_by_table_id: BTreeMap<&'a str, Vec<&'a Component>>,
    children_by_parent: BTreeMap<(&'a str, &'a str), Vec<&'a Component>>,
    edges_by_object_id: BTreeMap<&'a str, Vec<&'a AnalysisEdge>>,
}

impl<'a> ComponentIndexes<'a> {
    fn new(components: &'a [Component], edges: &'a [AnalysisEdge]) -> Self {
        let mut indexes = Self {
            filenames: BTreeMap::new(),
            by_id: BTreeMap::new(),
            added_by_id: BTreeMap::new(),
            fields_by_table_id: BTreeMap::new(),
            children_by_parent: BTreeMap::new(),
            edges_by_object_id: BTreeMap::new(),
        };
        for (index, component) in components.iter().enumerate() {
            indexes.filenames.insert(
                component.record_id.as_str(),
                format!(
                    "{}/{:05}-{}-{}.md",
                    component.kind,
                    index + 1,
                    component.kind,
                    slug(&component.name)
                ),
            );
            indexes
                .by_id
                .entry(component.object_id.as_str())
                .or_insert(component);
            if component.source_action == "AddAction" {
                indexes
                    .added_by_id
                    .entry(component.object_id.as_str())
                    .or_insert(component);
            }
            if component.kind == "field"
                && let Some(table_id) = component.table_id.as_deref()
            {
                indexes
                    .fields_by_table_id
                    .entry(table_id)
                    .or_default()
                    .push(component);
            }
            if component.kind == "layout_object"
                && let (Some(parent_id), Some(parent_record_id)) = (
                    component.parent_object_id.as_deref(),
                    component.parent_record_id.as_deref(),
                )
            {
                indexes
                    .children_by_parent
                    .entry((parent_id, parent_record_id))
                    .or_default()
                    .push(component);
            }
        }
        for edge in edges {
            if let Some(id) = edge.from_object_id.as_deref() {
                indexes.edges_by_object_id.entry(id).or_default().push(edge);
            }
            if let Some(id) = edge.to_object_id.as_deref()
                && edge.from_object_id.as_deref() != Some(id)
            {
                indexes.edges_by_object_id.entry(id).or_default().push(edge);
            }
        }
        indexes
    }

    fn page_path(&self, component: &Component) -> &str {
        self.filenames
            .get(component.record_id.as_str())
            .map(String::as_str)
            .unwrap_or("")
    }

    fn object(&self, id: &str) -> Option<&'a Component> {
        self.added_by_id
            .get(id)
            .copied()
            .or_else(|| self.by_id.get(id).copied())
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Analyze { input, out }) => run_analysis(input, out),
        Some(Commands::Validate { bundle }) => validate_bundle(&bundle),
        Some(Commands::Context {
            bundle,
            id,
            depth,
            max_objects,
        }) => write_context_package(&bundle, &id, depth, max_objects),
        None if io::stdin().is_terminal() => run_analysis(Vec::new(), None),
        None => {
            let mut command = Cli::command();
            command.print_help()?;
            println!();
            bail!(
                "use `fmblueprint analyze <xml-or-directory>... [--out <directory>]` outside an interactive terminal"
            )
        }
    }
}

fn validate_bundle(bundle: &Path) -> Result<()> {
    if !bundle.is_dir() {
        bail!(
            "documentation bundle directory does not exist: {}",
            bundle.display()
        );
    }
    let root = fs::canonicalize(bundle)?;
    let manifest_path = root.join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("missing {}", manifest_path.display()))?,
    )?;
    if manifest["schema_version"].as_u64() != Some(1) {
        bail!("unsupported or missing bundle manifest schema version");
    }
    let files = manifest["files"]
        .as_array()
        .context("manifest files must be an array")?;
    let mut paths = BTreeSet::new();
    for item in files {
        let relative = item
            .as_str()
            .context("manifest file path must be a string")?;
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            bail!("unsafe path in bundle manifest: {relative}");
        }
        if !paths.insert(relative.to_string()) {
            bail!("duplicate path in bundle manifest: {relative}");
        }
        let path = root.join(relative_path);
        if !path.is_file() {
            bail!("manifest file is missing: {relative}");
        }
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => {
                let _: serde::de::IgnoredAny =
                    serde_json::from_reader(BufReader::new(File::open(&path)?))
                        .with_context(|| format!("invalid JSON at {relative}"))?;
            }
            Some("jsonl") => {
                for (line_number, line) in BufReader::new(File::open(&path)?).lines().enumerate() {
                    let line = line?;
                    serde_json::from_str::<serde_json::Value>(&line).with_context(|| {
                        format!("invalid JSONL at {relative}:{}", line_number + 1)
                    })?;
                }
            }
            _ => {}
        }
    }
    validate_markdown_links(&root, &paths)?;
    let mut object_ids = BTreeSet::new();
    let mut record_ids = BTreeSet::new();
    for line in BufReader::new(File::open(root.join("objects.jsonl"))?).lines() {
        let component: serde_json::Value = serde_json::from_str(&line?)?;
        if let Some(id) = component["object_id"].as_str() {
            object_ids.insert(id.to_string());
        }
        if let Some(id) = component["record_id"].as_str() {
            record_ids.insert(id.to_string());
        }
        for record in component["attached_records"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = record["record_id"].as_str() {
                record_ids.insert(id.to_string());
            }
        }
    }
    for line in BufReader::new(File::open(root.join("edges.jsonl"))?).lines() {
        let edge: serde_json::Value = serde_json::from_str(&line?)?;
        for field in ["from_object_id", "to_object_id"] {
            if let Some(id) = edge[field].as_str()
                && !object_ids.contains(id)
            {
                bail!("edge references missing object ID {id}");
            }
        }
        for field in ["from_record_id", "to_record_id"] {
            if let Some(id) = edge[field].as_str()
                && !record_ids.contains(id)
            {
                bail!("edge references missing record ID {id}");
            }
        }
    }
    Ok(())
}

fn validate_markdown_links(root: &Path, paths: &BTreeSet<String>) -> Result<()> {
    for relative in paths.iter().filter(|path| path.ends_with(".md")) {
        let source = root.join(relative);
        let markdown = fs::read_to_string(&source)?;
        for (offset, _) in markdown.match_indices("](") {
            let escape_count = markdown.as_bytes()[..offset]
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count();
            if escape_count % 2 == 1 {
                continue;
            }
            let target = markdown[offset + 2..]
                .split(')')
                .next()
                .unwrap_or_default()
                .trim();
            if target.is_empty()
                || target.starts_with('#')
                || target.contains("://")
                || target.starts_with("mailto:")
            {
                continue;
            }
            let local = target.split('#').next().unwrap_or_default();
            if local.is_empty() {
                continue;
            }
            let resolved = fs::canonicalize(source.parent().unwrap_or(root).join(local))
                .with_context(|| format!("broken Markdown link in {relative}: {target}"))?;
            if !resolved.starts_with(root) {
                bail!("Markdown link escapes bundle from {relative}: {target}");
            }
        }
    }
    Ok(())
}

fn write_context_package(bundle: &Path, id: &str, depth: usize, max_objects: usize) -> Result<()> {
    if max_objects == 0 {
        bail!("--max-objects must be greater than zero");
    }
    let objects: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("objects.json"))?)?;
    let edges: serde_json::Value = serde_json::from_slice(&fs::read(bundle.join("edges.json"))?)?;
    let workflows: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("workflows/workflows.json"))?)?;
    let components = objects["components"]
        .as_array()
        .context("objects.components must be an array")?;
    let object_ids: BTreeSet<_> = components
        .iter()
        .filter_map(|component| component["object_id"].as_str())
        .collect();
    let workflow = workflows["workflows"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|workflow| workflow["id"].as_str() == Some(id));
    let mut roots = BTreeSet::new();
    if let Some(workflow) = workflow {
        for key in [
            "entry_object_id",
            "script_ids",
            "layout_ids",
            "field_read_ids",
            "field_write_ids",
            "table_occurrence_ids",
            "external_source_ids",
        ] {
            if let Some(single) = workflow[key].as_str() {
                roots.insert(single);
            }
            for related in workflow[key].as_array().into_iter().flatten() {
                if let Some(related) = related.as_str() {
                    roots.insert(related);
                }
            }
        }
    } else if object_ids.contains(id) {
        roots.insert(id);
    } else {
        bail!("no object or workflow with ID {id} was found in the bundle");
    }

    let mut included = BTreeSet::new();
    let mut continuation = BTreeSet::new();
    let mut pending: VecDeque<_> = roots.iter().map(|object_id| (*object_id, 0usize)).collect();
    while let Some((object_id, current_depth)) = pending.pop_front() {
        if !object_ids.contains(object_id) {
            continue;
        }
        if included.contains(object_id) {
            continue;
        }
        if included.len() >= max_objects {
            continuation.insert(object_id.to_string());
            continue;
        }
        included.insert(object_id.to_string());
        if current_depth >= depth {
            continue;
        }
        for edge in edges["edges"].as_array().into_iter().flatten() {
            let from = edge["from_object_id"].as_str();
            let to = edge["to_object_id"].as_str();
            if from == Some(object_id)
                && let Some(to) = to
                && object_ids.contains(to)
            {
                pending.push_back((to, current_depth + 1));
            }
            if to == Some(object_id)
                && let Some(from) = from
                && object_ids.contains(from)
            {
                pending.push_back((from, current_depth + 1));
            }
        }
    }
    for (object_id, _) in pending {
        continuation.insert(object_id.to_string());
    }
    let selected_components: Vec<_> = components
        .iter()
        .filter(|component| {
            component["object_id"]
                .as_str()
                .is_some_and(|object_id| included.contains(object_id))
        })
        .collect();
    let selected_edges: Vec<_> = edges["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|edge| {
            edge["from_object_id"]
                .as_str()
                .is_some_and(|object_id| included.contains(object_id))
                || edge["to_object_id"]
                    .as_str()
                    .is_some_and(|object_id| included.contains(object_id))
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "root_id": id,
            "root_kind": if workflow.is_some() { "workflow" } else { "object" },
            "depth": depth,
            "max_objects": max_objects,
            "truncated": !continuation.is_empty(),
            "continuation_ids": continuation,
            "workflow": workflow,
            "objects": selected_components,
            "edges": selected_edges,
        }))?
    );
    Ok(())
}

fn run_analysis(inputs: Vec<PathBuf>, output: Option<PathBuf>) -> Result<()> {
    if inputs.is_empty() && !io::stdin().is_terminal() {
        bail!(
            "XML input is required outside an interactive terminal; use `fmblueprint analyze <xml-or-directory>... [--out <directory>]`"
        )
    }
    let inputs = if inputs.is_empty() {
        vec![prompt_path("FileMaker XML input path: ", None)?]
    } else {
        inputs
    };
    let inputs = expand_xml_inputs(&inputs)?;
    let default_output = default_output_dir(&inputs);
    let output = match output {
        Some(output) => output,
        None if io::stdin().is_terminal() => {
            prompt_path("Output directory", Some(&default_output))?
        }
        None => default_output,
    };
    let mut applications = Vec::new();
    let mut catalogs: BTreeMap<(String, String, String), Catalog> = BTreeMap::new();
    let mut known_catalogs_not_observed = Vec::new();
    let mut diagnostics = Vec::new();
    let mut components = Vec::new();
    for input in &inputs {
        let encoding = detect_encoding(input)?;
        let file = File::open(input)
            .with_context(|| format!("cannot open XML input {}", input.display()))?;
        let parsed = parse_xml(file)?;
        let inventory = finish_inventory(parsed, encoding)?;
        applications.extend(inventory.applications);
        for catalog in inventory.catalogs {
            let key = (
                catalog.source_file.clone(),
                catalog.action.clone(),
                catalog.name.clone(),
            );
            let total = catalogs.entry(key).or_insert_with(|| Catalog {
                source_file: catalog.source_file,
                action: catalog.action,
                name: catalog.name,
                ..Catalog::default()
            });
            total.occurrences += catalog.occurrences;
            total.observed_entries += catalog.observed_entries;
            if let Some(count) = catalog.declared_member_count {
                *total.declared_member_count.get_or_insert(0) += count;
            }
        }
        known_catalogs_not_observed.extend(inventory.known_catalogs_not_observed);
        diagnostics.extend(inventory.diagnostics);
        let component_source = File::open(input)
            .with_context(|| format!("cannot reopen XML input {}", input.display()))?;
        components.extend(extract_data_model(component_source)?);
    }
    applications.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    let catalogs = catalogs.into_values().collect();
    known_catalogs_not_observed.sort();
    diagnostics.sort();
    let inventory = Inventory {
        schema_version: 2,
        source_format: "FMSaveAsXML".to_string(),
        applications,
        catalogs,
        known_catalogs_not_observed,
        diagnostics,
    };
    analyze_calculation_features(&mut components);
    let mut edges = analyze_scripts(&mut components);
    edges.extend(analyze_component_references(&components));
    edges.sort_by(|left, right| {
        left.from_record_id
            .cmp(&right.from_record_id)
            .then(left.kind.cmp(&right.kind))
            .then(left.to_object_id.cmp(&right.to_object_id))
            .then(left.to_record_id.cmp(&right.to_record_id))
            .then(left.evidence.cmp(&right.evidence))
    });
    let workflows = discover_workflows(&components, &edges);
    let indexes = ComponentIndexes::new(&components, &edges);

    fs::create_dir_all(&output)
        .with_context(|| format!("cannot create output directory {}", output.display()))?;
    let json_path = output.join("inventory.json");
    let markdown_path = output.join("overview.md");
    fs::write(&json_path, serde_json::to_vec_pretty(&inventory)?)
        .with_context(|| format!("cannot write {}", json_path.display()))?;
    fs::write(&markdown_path, render_markdown(&inventory))
        .with_context(|| format!("cannot write {}", markdown_path.display()))?;
    fs::write(
        output.join("objects.json"),
        serde_json::to_vec_pretty(&ComponentExport {
            schema_version: 2,
            components: &components,
        })?,
    )?;
    fs::write(
        output.join("edges.json"),
        serde_json::to_vec_pretty(&EdgeExport {
            schema_version: 2,
            edges: &edges,
        })?,
    )?;
    write_component_docs(&output, &components, &indexes)?;
    write_permission_matrix(&output, &components, &indexes)?;
    write_integration_docs(&output, &components, &workflows, &indexes)?;
    write_pre_html_bundle(
        &output,
        &inventory,
        &components,
        &edges,
        &workflows,
        &indexes,
    )?;
    println!("Inventory written to {}", output.display());
    Ok(())
}

fn write_integration_docs(
    output: &Path,
    components: &[Component],
    workflows: &[WorkflowCandidate],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    let integrations = output.join("integrations");
    fs::create_dir_all(&integrations)?;
    let mut markdown = String::from(
        "# Integration inventory\n\nThis report lists external data-source records and explicit references found in the export. Script effects are static step classifications; endpoint behavior and successful runtime access are not established.\n\n## External data sources\n\n",
    );
    let sources: Vec<_> = components
        .iter()
        .filter(|component| component.kind == "external_data_source")
        .collect();
    if sources.is_empty() {
        markdown.push_str("No external data-source records were present.\n");
    }
    for source in sources {
        let references: Vec<_> = indexes
            .edges_by_object_id
            .get(source.object_id.as_str())
            .into_iter()
            .flatten()
            .filter(|edge| {
                edge.kind == "accesses_external_source"
                    && edge.to_object_id.as_deref() == Some(source.object_id.as_str())
            })
            .collect();
        markdown.push_str(&format!(
            "- **{}** (`{}`; action `{}`): {} explicit reference(s). Source record: `objects/{}`.\n",
            md(&source.name), md(&source.object_id), md(&source.source_action), references.len(), indexes.page_path(source)
        ));
    }
    markdown.push_str("\n## Script effects associated with integrations\n\n");
    let mut found_effect = false;
    for script in components
        .iter()
        .filter(|component| component.kind == "script")
    {
        let effects: Vec<_> = script
            .findings
            .iter()
            .filter(|finding| {
                finding.kind == "step_effect"
                    && [
                        "opening a URL",
                        "fetching data from a URL",
                        "email sending",
                        "record import",
                        "record export",
                        "calling a server-side script",
                        "executing JavaScript in a web viewer",
                        "changing web-viewer content",
                    ]
                    .iter()
                    .any(|effect| finding.summary.contains(effect))
            })
            .collect();
        if effects.is_empty() {
            continue;
        }
        found_effect = true;
        markdown.push_str(&format!(
            "- **{}** (`{}`): {}. Evidence:\n",
            md(&script.name),
            md(&script.object_id),
            effects
                .iter()
                .map(|effect| md(&effect.summary))
                .collect::<Vec<_>>()
                .join("; ")
        ));
        for evidence in effects.iter().flat_map(|effect| &effect.evidence) {
            markdown.push_str(&format!("  - `{}`\n", md(evidence)));
        }
    }
    if !found_effect {
        markdown.push_str("No classified URL, email, import, or export steps were found.\n");
    }
    markdown.push_str("\nCredentials are redacted in component records. Literal paths or URLs may be present in source properties; this summary does not execute or validate them.\n");
    fs::write(integrations.join("overview.md"), markdown)?;

    let workflow_dir = output.join("workflows");
    fs::create_dir_all(&workflow_dir)?;
    fs::write(
        workflow_dir.join("workflows.json"),
        serde_json::to_vec_pretty(&WorkflowExport {
            schema_version: 2,
            workflows,
        })?,
    )?;
    let mut index = String::from(
        "# Workflow candidates\n\nCandidates are derived from explicit UI-to-script or UI-navigation references and reachable script edges. They do not establish runtime order, branch outcomes, or business intent.\n\n",
    );
    if workflows.is_empty() {
        index.push_str("No structurally connected workflow candidates were found.\n");
    }
    for (number, workflow) in workflows.iter().enumerate() {
        let filename = format!("{:04}-{}.md", number + 1, slug(&workflow.entry_name));
        let entry_link = indexes
            .object(&workflow.entry_object_id)
            .map(|component| {
                format!(
                    "[{}](../objects/{})",
                    md(&component_display_name(component)),
                    indexes.page_path(component)
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "{} (`{}`)",
                    md(&workflow.entry_kind),
                    md(&workflow.entry_object_id)
                )
            });
        index.push_str(&format!(
            "- [{}]({filename}) — entry {entry_link}; {} reachable script(s), status `{}`\n",
            md(&workflow.entry_name),
            workflow.script_ids.len(),
            workflow.status
        ));
        let mut page = format!(
            "# {}\n\n- Status: `{}`\n- Entry: {entry_link}\n- Source: `{}`\n",
            md(&workflow.entry_name),
            workflow.status,
            md(&workflow.source_file)
        );
        for (label, ids) in [
            ("Scripts", &workflow.script_ids),
            ("Layouts", &workflow.layout_ids),
            ("Fields read", &workflow.field_read_ids),
            ("Fields written", &workflow.field_write_ids),
            ("Table occurrences", &workflow.table_occurrence_ids),
            ("External sources", &workflow.external_source_ids),
        ] {
            page.push_str(&format!("\n## {label}\n\n"));
            if ids.is_empty() {
                page.push_str("None resolved.\n");
            } else {
                for id in ids {
                    let object = indexes.object(id);
                    if let Some(object) = object {
                        page.push_str(&format!(
                            "- [{}](../objects/{}) (`{}`)\n",
                            md(&component_display_name(object)),
                            indexes.page_path(object),
                            md(id)
                        ));
                    } else {
                        page.push_str(&format!("- `{}`\n", md(id)));
                    }
                }
            }
        }
        page.push_str("\n## Effects\n\n");
        if workflow.effects.is_empty() {
            page.push_str("No classified effects.\n");
        } else {
            for effect in &workflow.effects {
                page.push_str(&format!("- {}\n", md(effect)));
            }
        }
        page.push_str("\n## Evidence\n\n");
        if workflow.evidence.is_empty() {
            page.push_str("No source paths attached.\n");
        } else {
            for evidence in &workflow.evidence {
                page.push_str(&format!("- `{}`\n", md(evidence)));
            }
        }
        page.push_str("\n## Limitations\n\n");
        for limitation in &workflow.limitations {
            page.push_str(&format!("- {}\n", md(limitation)));
        }
        fs::write(workflow_dir.join(filename), page)?;
    }
    fs::write(workflow_dir.join("index.md"), index)?;
    Ok(())
}

fn write_pre_html_bundle(
    output: &Path,
    inventory: &Inventory,
    components: &[Component],
    edges: &[AnalysisEdge],
    workflows: &[WorkflowCandidate],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    for directory in [
        "schemas",
        "architecture",
        "data",
        "graphs",
        "migration",
        "agent",
        "reports",
    ] {
        fs::create_dir_all(output.join(directory))?;
    }
    for (name, schema) in [
        (
            "application.schema.json",
            include_str!("../schemas/application.schema.json"),
        ),
        (
            "objects.schema.json",
            include_str!("../schemas/objects.schema.json"),
        ),
        (
            "edges.schema.json",
            include_str!("../schemas/edges.schema.json"),
        ),
        (
            "graph.schema.json",
            include_str!("../schemas/graph.schema.json"),
        ),
        (
            "workflows.schema.json",
            include_str!("../schemas/workflows.schema.json"),
        ),
        (
            "object.schema.json",
            include_str!("../schemas/object.schema.json"),
        ),
        (
            "edge.schema.json",
            include_str!("../schemas/edge.schema.json"),
        ),
        (
            "workflow.schema.json",
            include_str!("../schemas/workflow.schema.json"),
        ),
        (
            "coverage.schema.json",
            include_str!("../schemas/coverage.schema.json"),
        ),
        (
            "diagnostics.schema.json",
            include_str!("../schemas/diagnostics.schema.json"),
        ),
        (
            "diagnostic.schema.json",
            include_str!("../schemas/diagnostic.schema.json"),
        ),
        (
            "requirements.schema.json",
            include_str!("../schemas/requirements.schema.json"),
        ),
        (
            "context-map.schema.json",
            include_str!("../schemas/context-map.schema.json"),
        ),
        (
            "context-package.schema.json",
            include_str!("../schemas/context-package.schema.json"),
        ),
        (
            "task-units.schema.json",
            include_str!("../schemas/task-units.schema.json"),
        ),
        (
            "testing-scenario.schema.json",
            include_str!("../schemas/testing-scenario.schema.json"),
        ),
    ] {
        fs::write(output.join("schemas").join(name), schema)?;
    }
    fs::write(
        output.join("schemas/compatibility.md"),
        include_str!("../docs/schema-compatibility.md"),
    )?;

    fs::write(
        output.join("application.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": inventory.schema_version,
            "source_format": inventory.source_format,
            "applications": &inventory.applications,
            "catalogs": &inventory.catalogs,
            "known_catalogs_not_observed": &inventory.known_catalogs_not_observed,
            "diagnostics": &inventory.diagnostics,
        }))?,
    )?;
    write_jsonl(output.join("objects.jsonl").as_path(), components)?;
    write_jsonl(output.join("edges.jsonl").as_path(), edges)?;

    let component_counts = count_labels(components.iter().map(|component| component.kind.as_str()));
    let edge_counts = count_labels(edges.iter().map(|edge| edge.kind.as_str()));
    let finding_counts = count_labels(components.iter().flat_map(|component| {
        component
            .findings
            .iter()
            .map(|finding| finding.kind.as_str())
    }));
    let mut xml_element_counts = BTreeMap::new();
    for component in components {
        collect_xml_element_counts(&component.properties, &mut xml_element_counts);
        for record in &component.attached_records {
            collect_xml_element_counts(&record.properties, &mut xml_element_counts);
        }
    }
    let limitations = vec![
        serde_json::json!({"feature":"calculation language","status":"partial","description":"Only structured chunks and conservative qualified-name heuristics are analyzed; formulas are not evaluated."}),
        serde_json::json!({"feature":"script runtime","status":"partial","description":"Control flow and call graph are static; dynamic destinations, state, timing, and runtime outcomes are unresolved."}),
        serde_json::json!({"feature":"workflow reconstruction","status":"partial","description":"Candidates are reachability summaries, not verified user journeys or business processes."}),
        serde_json::json!({"feature":"effective authorization","status":"uncertain","description":"Exported privilege actions are transcribed separately; partial changes are not merged into an effective runtime policy."}),
        serde_json::json!({"feature":"external integrations","status":"partial","description":"Static URLs and external-source references do not establish endpoint validity or successful access."}),
        serde_json::json!({"feature":"runtime records","status":"unsupported","description":"Save-a-Copy-as-XML describes design metadata and does not contain application record data."}),
    ];
    let coverage = serde_json::json!({
        "schema_version": 1,
        "catalogs": {"observed": &inventory.catalogs, "not_observed": &inventory.known_catalogs_not_observed, "unknown_catalog_items": component_counts.get("unknown_catalog_item").copied().unwrap_or(0)},
        "component_counts": component_counts,
        "edge_counts": edge_counts,
        "edge_status_counts": count_labels(edges.iter().map(|edge| edge.status.as_str())),
        "finding_counts": finding_counts,
        "preserved_xml_element_counts": xml_element_counts,
        "limitations": limitations,
    });
    fs::write(
        output.join("coverage.json"),
        serde_json::to_vec_pretty(&coverage)?,
    )?;

    let mut diagnostics = Vec::new();
    for diagnostic in &inventory.diagnostics {
        diagnostics.push(serde_json::json!({"kind":"inventory_diagnostic","status":"unresolved","summary":diagnostic,"source_object_id":null,"target_object_id":null,"evidence":[],"limitations":[]}));
    }
    for component in components {
        for diagnostic in &component.diagnostics {
            diagnostics.push(serde_json::json!({"kind":"component_diagnostic","status":"unresolved","summary":diagnostic,"source_object_id":component.object_id,"target_object_id":null,"evidence":[component.source_path],"limitations":[]}));
        }
        for finding in &component.findings {
            if matches!(
                finding.status.as_str(),
                "heuristic_candidate" | "unresolved"
            ) {
                diagnostics.push(serde_json::json!({"kind":finding.kind,"status":finding.status,"summary":finding.summary,"source_object_id":component.object_id,"target_object_id":null,"evidence":finding.evidence,"limitations":finding.limitations}));
            }
        }
    }
    for edge in edges
        .iter()
        .filter(|edge| matches!(edge.status.as_str(), "heuristic_candidate" | "unresolved"))
    {
        diagnostics.push(serde_json::json!({"kind":edge.kind,"status":edge.status,"summary":format!("{} reference is {}", edge.kind, edge.status),"source_object_id":edge.from_object_id,"target_object_id":edge.to_object_id,"evidence":edge.evidence,"limitations":edge.limitations}));
    }
    diagnostics.sort_by_key(|diagnostic| serde_json::to_string(diagnostic).unwrap_or_default());
    for (index, diagnostic) in diagnostics.iter_mut().enumerate() {
        diagnostic["id"] = serde_json::json!(format!("diag:{:06}", index + 1));
    }
    fs::write(
        output.join("diagnostics.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"schema_version":1,"diagnostics":diagnostics}),
        )?,
    )?;
    write_analysis_graphs(output, edges, workflows)?;
    write_architecture_docs(output, inventory, components, edges, workflows, indexes)?;
    write_migration_docs(output, components, workflows, indexes)?;
    write_agent_bundle(output, components, workflows, &diagnostics, indexes)?;

    let reports = output.join("reports");
    let edge_status = count_labels(edges.iter().map(|edge| edge.status.as_str()));
    let mut quality = format!(
        "# Analysis quality\n\n- Applications: {}\n- Components: {}\n- Relationships: {}\n- Workflows: {}\n- Unresolved edges: {}\n- Heuristic candidate edges: {}\n\nCounts describe source records and static findings, not final runtime state. See [coverage.json](../coverage.json) and [diagnostics.json](../diagnostics.json).\n",
        inventory.applications.len(),
        components.len(),
        edges.iter().filter(|edge| edge.kind == "joins_to").count(),
        workflows.len(),
        edge_status.get("unresolved").copied().unwrap_or(0),
        edge_status.get("heuristic_candidate").copied().unwrap_or(0)
    );
    quality.push_str("\n## Known interpretation limits\n\nCalculations are not evaluated. Workflow and call reachability do not establish execution. Privilege settings remain separated by exported action. Missing catalogs can mean excluded or empty content.\n");
    fs::write(reports.join("analysis-quality.md"), quality)?;
    let mut unsupported = String::from(
        "# Unsupported and incomplete analysis\n\n| Area | Status | Limitation |\n|---|---|---|\n",
    );
    for limitation in coverage["limitations"].as_array().into_iter().flatten() {
        unsupported.push_str(&format!(
            "| {} | `{}` | {} |\n",
            md(limitation["feature"].as_str().unwrap_or("unknown")),
            limitation["status"].as_str().unwrap_or("uncertain"),
            md(limitation["description"].as_str().unwrap_or(""))
        ));
    }
    fs::write(reports.join("unsupported-features.md"), unsupported)?;
    let mut unresolved = String::from(
        "# Unresolved and heuristic references\n\nReferences retain uncertainty and are not promoted to confirmed links.\n\n| Status | Kind | From | To | Evidence | Limitation |\n|---|---|---|---|---|---|\n",
    );
    for edge in edges
        .iter()
        .filter(|edge| matches!(edge.status.as_str(), "unresolved" | "heuristic_candidate"))
    {
        unresolved.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` | {} | {} |\n",
            edge.status,
            md(&edge.kind),
            md(edge.from_object_id.as_deref().unwrap_or("—")),
            md(edge.to_object_id.as_deref().unwrap_or("—")),
            md(&edge.evidence.join("; ")),
            md(&edge.limitations.join("; "))
        ));
    }
    fs::write(reports.join("unresolved-references.md"), unresolved)?;

    let mut readme = String::from(
        "# FileMaker migration blueprint\n\nThis bundle is generated offline from design XML. It does not contain application records and does not prove runtime behavior. Start with the coverage report, then follow evidence links into component pages.\n\n## Main reports\n\n- [Architecture overview](architecture/overview.md)\n- [Data dictionary](data/data-dictionary.md)\n- [Security matrix](security/permission-matrix.md)\n- [Integration inventory](integrations/overview.md)\n- [Workflow candidates](workflows/index.md)\n- [Migration notes](migration/README.md)\n- [Coverage and uncertainty](coverage.json)\n- [Diagnostics](diagnostics.json)\n- [Agent guide](agent/AGENTS.md)\n\nValidate this bundle with `fmblueprint validate <bundle>`. Export bounded context with `fmblueprint context <bundle> --id <object-or-workflow-id> --depth 2 --max-objects 250`.\n",
    );
    readme.push_str("\nSchemas are versioned per artifact family in `schemas/`; see the repository compatibility policy before consuming them.\n");
    fs::write(output.join("README.md"), readme)?;
    write_manifest(output, inventory)?;
    Ok(())
}

fn count_labels<'a>(labels: impl Iterator<Item = &'a str>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for label in labels {
        *counts.entry(label.to_string()).or_insert(0) += 1;
    }
    counts
}

fn collect_xml_element_counts(node: &XmlNode, counts: &mut BTreeMap<String, usize>) {
    *counts.entry(node.name.clone()).or_insert(0) += 1;
    for child in &node.children {
        collect_xml_element_counts(child, counts);
    }
}

fn write_jsonl<T: Serialize>(path: &Path, rows: impl IntoIterator<Item = T>) -> Result<()> {
    let mut file = File::create(path)?;
    for row in rows {
        serde_json::to_writer(&mut file, &row)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn write_analysis_graphs(
    output: &Path,
    edges: &[AnalysisEdge],
    workflows: &[WorkflowCandidate],
) -> Result<()> {
    let directory = output.join("graphs");
    let graphs: [GraphSelector; 4] = [
        ("relationships.json", |edge| {
            matches!(
                edge.kind.as_str(),
                "joins_to" | "references_table" | "uses_table_occurrence"
            )
        }),
        ("script-calls.json", |edge| edge.kind == "calls_script"),
        ("navigation.json", |edge| {
            matches!(
                edge.kind.as_str(),
                "navigates_to" | "triggers_script" | "is_triggered_by"
            )
        }),
        ("dependencies.json", |_| true),
    ];
    for (name, include) in graphs {
        let selected: Vec<_> = edges.iter().filter(|edge| include(edge)).collect();
        fs::write(
            directory.join(name),
            serde_json::to_vec_pretty(&serde_json::json!({"schema_version":1,"edges":selected}))?,
        )?;
    }
    fs::write(
        directory.join("workflows.json"),
        serde_json::to_vec_pretty(&WorkflowExport {
            schema_version: 2,
            workflows,
        })?,
    )?;
    Ok(())
}

fn write_architecture_docs(
    output: &Path,
    inventory: &Inventory,
    components: &[Component],
    edges: &[AnalysisEdge],
    workflows: &[WorkflowCandidate],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    let directory = output.join("architecture");
    let mut overview = String::from(
        "# Architecture overview\n\nThe export contains design metadata only. Base tables and table occurrences are represented separately; relationship edges preserve occurrence context. Static analysis does not execute scripts or calculations.\n\n## Source files\n\n| File | UUID | FileMaker | Export | DDR_INFO |\n|---|---|---|---|---|\n",
    );
    for app in &inventory.applications {
        overview.push_str(&format!(
            "| {} | `{}` | {} | {} | {} |\n",
            md(app.file_name.as_deref().unwrap_or("unknown")),
            md(app.uuid.as_deref().unwrap_or("unknown")),
            md(app.filemaker_version.as_deref().unwrap_or("unknown")),
            md(app.export_version.as_deref().unwrap_or("unknown")),
            app.ddr_info_present
        ));
    }
    overview.push_str(&format!("\n{} components and {} typed edges were extracted. See [dependency map](dependency-map.md), [data flows](data-flows.md), [entry points](entry-points.md), and [coverage](../coverage.json).\n", components.len(), edges.len()));
    fs::write(directory.join("overview.md"), overview)?;
    let mut entries = String::from(
        "# Candidate entry points\n\nThese are static UI trigger/navigation candidates. They are not confirmed user journeys.\n\n",
    );
    for (index, workflow) in workflows.iter().enumerate() {
        let filename = format!("{:04}-{}.md", index + 1, slug(&workflow.entry_name));
        entries.push_str(&format!(
            "- [{}](../workflows/{filename}): {} (`{}`)\n",
            md(&workflow.entry_name),
            workflow.entry_kind,
            workflow.status
        ));
    }
    fs::write(directory.join("entry-points.md"), entries)?;
    let mut dependencies = String::from("# Dependency map\n\n| Edge kind | Count |\n|---|---:|\n");
    for (kind, count) in count_labels(edges.iter().map(|edge| edge.kind.as_str())) {
        dependencies.push_str(&format!("| `{kind}` | {count} |\n"));
    }
    fs::write(directory.join("dependency-map.md"), dependencies)?;
    let mut flows = String::from(
        "# Data flows\n\n| Kind | Status | From | To | Evidence |\n|---|---|---|---|---|\n",
    );
    for edge in edges.iter().filter(|edge| {
        matches!(
            edge.kind.as_str(),
            "reads_field"
                | "writes_field"
                | "uses_table_occurrence"
                | "joins_to"
                | "accesses_external_source"
        )
    }) {
        flows.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` | `{}` |\n",
            edge.kind,
            edge.status,
            md(edge.from_object_id.as_deref().unwrap_or("—")),
            md(edge.to_object_id.as_deref().unwrap_or("—")),
            md(&edge.evidence.join("; "))
        ));
    }
    fs::write(directory.join("data-flows.md"), flows)?;

    let mut dictionary = String::from(
        "# Data dictionary\n\nThis is a literal FileMaker schema index; names do not establish domain meaning.\n\n| Table | Field | XML type | Source action | Component page |\n|---|---|---|---|---|\n",
    );
    for field in components
        .iter()
        .filter(|component| component.kind == "field")
    {
        let xml_type = first_named_property(&field.properties, "Field")
            .and_then(|node| {
                node.attributes
                    .get("datatype")
                    .or_else(|| node.attributes.get("dataType"))
            })
            .map(String::as_str)
            .unwrap_or("not specified");
        dictionary.push_str(&format!(
            "| {} | {} | {} | `{}` | [details](../objects/{}) |\n",
            md(field.table_name.as_deref().unwrap_or("unresolved table")),
            md(&field.name),
            md(xml_type),
            md(&field.source_action),
            indexes.page_path(field)
        ));
    }
    fs::write(output.join("data").join("data-dictionary.md"), dictionary)?;
    Ok(())
}

fn first_named_property<'a>(node: &'a XmlNode, name: &str) -> Option<&'a XmlNode> {
    if node.name == name {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| first_named_property(child, name))
}

fn write_migration_docs(
    output: &Path,
    components: &[Component],
    workflows: &[WorkflowCandidate],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    let directory = output.join("migration");
    let readme = String::from(
        "# Migration blueprint (framework-neutral)\n\nThese documents describe observed design facts and candidate implementation slices. They are not a target-framework design or verified business requirements.\n\n- [Feature inventory](feature-inventory.md)\n- [Requirements and static findings](requirements-derived.md)\n- [Screens](screens.md)\n- [Data contracts](data-contracts.md)\n- [Security contracts](security-contracts.md)\n- [Integration contracts](integration-contracts.md)\n- [Candidate test scenarios](testing-scenarios.md)\n- [Suggested implementation order](implementation-order.md)\n- [Limitations and decisions](limitations-and-decisions.md)\n",
    );
    fs::write(directory.join("README.md"), readme)?;
    fs::write(
        directory.join("feature-inventory.md"),
        "# Feature inventory\n\nCounts and observed feature families are in [coverage.json](../coverage.json). Unsupported and uncertain behavior is listed in [reports/unsupported-features.md](../reports/unsupported-features.md).\n",
    )?;
    fs::write(
        directory.join("requirements-derived.md"),
        "# Derived findings\n\n`agent/requirements.jsonl` contains static findings, not business requirements. Each entry retains source object IDs, evidence paths, epistemic status, and limitations. Human discovery is required before treating any finding as a product requirement.\n",
    )?;
    let mut screens = String::from(
        "# Screen inventory\n\nLayouts are literal source records. Object coordinates and trigger behavior remain attached to their component pages.\n\n| Layout | Action | Source |\n|---|---|---|\n",
    );
    for layout in components
        .iter()
        .filter(|component| component.kind == "layout")
    {
        screens.push_str(&format!(
            "| [{}](../objects/{}) | `{}` | `{}` |\n",
            md(&component_display_name(layout)),
            indexes.page_path(layout),
            layout.source_action,
            md(&layout.source_path)
        ));
    }
    fs::write(directory.join("screens.md"), screens)?;
    let data = String::from(
        "# Data contracts\n\nThe XML defines fields and table context, not record payloads or API contracts. Treat this list as a schema inventory pending application-level validation. See [data dictionary](../data/data-dictionary.md).\n",
    );
    fs::write(directory.join("data-contracts.md"), data)?;
    fs::write(
        directory.join("security-contracts.md"),
        "# Security contracts\n\nSee [exported access matrix](../security/permission-matrix.md). The matrix preserves privilege source actions and explicit table/object rules; it does not compute effective runtime access or evaluate data-dependent calculations.\n",
    )?;
    fs::write(
        directory.join("integration-contracts.md"),
        "# Integration contracts\n\nSee [integration inventory](../integrations/overview.md). Endpoint literals and external-source paths remain inert evidence and are not validated or executed.\n",
    )?;
    fs::write(
        directory.join("implementation-order.md"),
        "# Suggested implementation order\n\n1. Confirm source-file identity, table occurrences, relationships, and field definitions.\n2. Validate account assignments and privilege rules with application owners.\n3. Reconstruct screen and workflow behavior from candidate entries and runtime scenarios.\n4. Implement data mutations and integrations only after their unresolved behavior is confirmed.\n\nThis ordering is a planning hint derived from static coverage, not a target architecture.\n",
    )?;
    fs::write(
        directory.join("limitations-and-decisions.md"),
        "# Limitations and decisions\n\n- Preserve Add/Modify/Remove source actions separately; no final-state merge is inferred.\n- Treat name-matched calculations and cross-file UUID matches as heuristic candidates.\n- Do not execute calculations, scripts, web-viewer content, SQL, or plug-ins.\n- Obtain paired Save-a-Copy XML with `DDR_INFO` and DDR XML only if a coverage comparison is needed; DDR is not required for this output contract.\n",
    )?;
    let mut scenarios = String::from(
        "# Candidate validation scenarios\n\nThese seeds validate observable behavior later; static effects are not expected runtime outcomes.\n\n",
    );
    for workflow in workflows {
        scenarios.push_str(&format!("## {}\n\n- Entry object: `{}`\n- Candidate static effects: {}\n- Questions to verify: {}\n\n", md(&workflow.entry_name), md(&workflow.entry_object_id), if workflow.effects.is_empty() { "none classified".to_string() } else { workflow.effects.iter().map(|effect| md(effect)).collect::<Vec<_>>().join("; ") }, workflow.limitations.iter().map(|limitation| md(limitation)).collect::<Vec<_>>().join("; ")));
    }
    fs::write(directory.join("testing-scenarios.md"), scenarios)?;
    Ok(())
}

fn write_agent_bundle(
    output: &Path,
    components: &[Component],
    workflows: &[WorkflowCandidate],
    diagnostics: &[serde_json::Value],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    let directory = output.join("agent");
    fs::write(
        directory.join("AGENTS.md"),
        "# Agent guide\n\nUse `context-map.json` to locate objects and first-degree dependencies. Read component Markdown for preserved source properties and evidence. Use graph JSON for typed relationships. Run `fmblueprint context <bundle> --id <id> --depth 2 --max-objects 250` to emit a bounded context package; inspect `continuation_ids` if truncated. Interpret `explicit` as present in XML, `derived` as structural analysis, `heuristic_candidate` as a name/UUID-based lead, and `unresolved` as missing or ambiguous. Never promote heuristic findings into verified requirements. No artifact contains runtime records.\n",
    )?;
    fs::write(
        directory.join("INDEX.md"),
        "# Agent artifacts\n\n- `context-map.json`: object IDs, docs, and graph neighbors.\n- `requirements.jsonl`: evidence-backed static findings, not business requirements.\n- `unknowns.jsonl`: unresolved and heuristic analysis questions (`schemas/diagnostic.schema.json`).\n- `task-units.json`: suggested migration work slices.\n- `testing-scenarios.jsonl`: workflow validation seeds.\n- `schemas/context-package.schema.json`: bounded context output contract.\n",
    )?;
    let mut context_objects = Vec::new();
    for component in components {
        let related_edges = indexes.edges_by_object_id.get(component.object_id.as_str());
        let outgoing: BTreeSet<_> = related_edges
            .into_iter()
            .flatten()
            .filter(|edge| edge.from_object_id.as_deref() == Some(component.object_id.as_str()))
            .filter_map(|edge| edge.to_object_id.clone())
            .collect();
        let incoming: BTreeSet<_> = related_edges
            .into_iter()
            .flatten()
            .filter(|edge| edge.to_object_id.as_deref() == Some(component.object_id.as_str()))
            .filter_map(|edge| edge.from_object_id.clone())
            .collect();
        context_objects.push(serde_json::json!({"id":component.object_id,"record_id":component.record_id,"kind":component.kind,"name":component.name,"source_file":component.source_file,"status":if component.findings.iter().any(|finding| finding.status == "unresolved") { "partially_unresolved" } else { "documented" },"document":format!("../objects/{}",indexes.page_path(component)),"outgoing":outgoing,"incoming":incoming,"source_path":component.source_path}));
    }
    fs::write(
        directory.join("context-map.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"schema_version":1,"depth_hint":1,"objects":context_objects}),
        )?,
    )?;
    let mut requirements = Vec::new();
    for component in components {
        for (index, finding) in component.findings.iter().enumerate() {
            requirements.push(serde_json::json!({"id":format!("finding:{}:{}:{:04}",component.record_id,finding.kind,index+1),"kind":"static_finding","statement":finding.summary,"status":finding.status,"source_objects":[component.object_id],"evidence":finding.evidence,"dependencies":[],"unknowns":finding.limitations}));
        }
    }
    write_jsonl(directory.join("requirements.jsonl").as_path(), requirements)?;
    let unknowns: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic["status"].as_str(),
                Some("unresolved" | "heuristic_candidate")
            )
        })
        .cloned()
        .collect();
    write_jsonl(directory.join("unknowns.jsonl").as_path(), unknowns)?;
    let tasks = [
        (
            "data-model",
            "Validate tables, fields, occurrences and join predicates",
            "data",
        ),
        (
            "security",
            "Review accounts and privilege rules",
            "security",
        ),
        (
            "screens",
            "Validate layout objects and navigation candidates",
            "ui",
        ),
        (
            "integrations",
            "Confirm external sources and script effects",
            "integration",
        ),
        (
            "workflows",
            "Validate candidate paths and runtime conditions",
            "workflow",
        ),
    ];
    let task_units: Vec<_> = tasks.iter().map(|(id, title, kind)| serde_json::json!({"id":format!("task:{id}"),"title":title,"kind":kind,"status":"planning_hint","source_object_ids":components.iter().filter(|component| match *kind { "data" => matches!(component.kind.as_str(),"table"|"field"|"table_occurrence"|"relationship"),"security" => matches!(component.kind.as_str(),"account"|"privilege_set"),"ui" => matches!(component.kind.as_str(),"layout"|"layout_object"|"custom_menu"),"integration" => component.kind == "external_data_source","workflow" => component.kind == "script", _ => false }).map(|component| component.object_id.clone()).collect::<BTreeSet<_>>(),"depends_on":if *id == "data-model" { Vec::<String>::new() } else { vec!["task:data-model".to_string()] }})).collect();
    fs::write(
        directory.join("task-units.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"schema_version":1,"tasks":task_units}))?,
    )?;
    let scenarios: Vec<_> = workflows.iter().map(|workflow| serde_json::json!({"id":format!("scenario:{}",workflow.id),"kind":"workflow_validation_seed","status":workflow.status,"entry_object_id":workflow.entry_object_id,"actions":[format!("Exercise UI entry {} in a controlled FileMaker runtime",workflow.entry_object_id)],"observed_static_effects":workflow.effects,"open_questions":workflow.limitations,"evidence":workflow.evidence})).collect();
    write_jsonl(
        directory.join("testing-scenarios.jsonl").as_path(),
        scenarios,
    )?;
    Ok(())
}

fn collect_output_files(directory: &Path, root: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_output_files(&path, root, files)?;
        } else if entry.file_type()?.is_file() {
            files.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn write_manifest(output: &Path, inventory: &Inventory) -> Result<()> {
    let mut files = Vec::new();
    collect_output_files(output, output, &mut files)?;
    files.push("manifest.json".to_string());
    files.sort();
    files.dedup();
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"schema_version":1,"generator":"fmblueprint","generator_version":env!("CARGO_PKG_VERSION"),"source_format":inventory.source_format,"inputs":inventory.applications.iter().map(|app| serde_json::json!({"file_name":app.file_name,"uuid":app.uuid,"filemaker_version":app.filemaker_version,"export_version":app.export_version})).collect::<Vec<_>>(),"artifact_schemas":{"application":2,"object":2,"objects_export":2,"edge":2,"edges_export":2,"graph":1,"workflow":2,"workflows_export":2,"coverage":1,"diagnostic":1,"diagnostics":1,"agent_context":1,"context_package":1,"agent_tasks":1,"agent_requirements":1,"agent_unknown":1,"agent_scenario":1},"files":files}),
        )?,
    )?;
    Ok(())
}

fn component_display_name(component: &Component) -> String {
    if component.name.trim().is_empty() {
        format!(
            "{} {}",
            component.kind,
            component.xml_id.as_deref().unwrap_or("unknown")
        )
    } else {
        component.name.clone()
    }
}

fn prompt_path(prompt: &str, default: Option<&Path>) -> Result<PathBuf> {
    match default {
        Some(default) => print!("{prompt} [{}]: ", default.display()),
        None => print!("{prompt}"),
    }
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        return default.map(Path::to_path_buf).context("a path is required");
    }
    Ok(PathBuf::from(value))
}

fn expand_xml_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for input in inputs {
        if input.is_dir() {
            for entry in fs::read_dir(input)
                .with_context(|| format!("cannot read input directory {}", input.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                if entry.file_type()?.is_file()
                    && path.extension().is_some_and(|extension| {
                        extension.to_string_lossy().eq_ignore_ascii_case("xml")
                    })
                {
                    paths.push(path);
                }
            }
        } else {
            paths.push(input.clone());
        }
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        bail!("no XML files found in the supplied input")
    }
    Ok(paths)
}

fn default_output_dir(inputs: &[PathBuf]) -> PathBuf {
    if let [input] = inputs {
        let name = input
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("filemaker-app");
        return PathBuf::from("output").join(name);
    }
    let common_parent = inputs
        .first()
        .and_then(|first| first.parent())
        .filter(|parent| inputs.iter().all(|input| input.parent() == Some(*parent)));
    let name = common_parent
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("filemaker-app");
    PathBuf::from("output").join(name)
}

fn detect_encoding(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("cannot open XML input {}", path.display()))?;
    let mut prefix = [0; 3];
    let read = file.read(&mut prefix)?;
    Ok(match &prefix[..read] {
        [0xff, 0xfe, ..] => "UTF-16 LE (BOM)",
        [0xfe, 0xff, ..] => "UTF-16 BE (BOM)",
        [0xef, 0xbb, 0xbf] => "UTF-8 (BOM)",
        _ => "XML declared or UTF-8",
    }
    .to_string())
}

fn parse_xml<R: Read>(source: R) -> Result<ParsedXml> {
    let mut reader = Reader::from_reader(DecodingReader::new(BufReader::new(source)));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut catalogs: Vec<CatalogScope> = Vec::new();
    let mut parsed = ParsedXml::default();
    let mut source_file = String::from("unknown-file");
    let mut root_seen = false;

    loop {
        match reader.read_event_into(&mut buffer).context("invalid XML")? {
            Event::Start(event) => {
                let name = event.local_name().as_ref().to_string();
                let attrs = attributes(&event)?;
                if !root_seen {
                    if name != "FMSaveAsXML" {
                        bail!("unsupported XML root <{name}>; expected <FMSaveAsXML>")
                    }
                    root_seen = true;
                    parsed.application = Some(application(&attrs));
                    source_file = parsed
                        .application
                        .as_ref()
                        .and_then(|application| application.file_name.clone())
                        .unwrap_or_else(|| "unknown-file".to_string());
                }
                if name == "DDR_INFO" {
                    parsed.ddr_info_present = true;
                }
                observe_entry(&name, &attrs, &stack, &mut catalogs);
                if name.ends_with("Catalog") {
                    let action = stack
                        .iter()
                        .rev()
                        .find(|item| {
                            matches!(item.as_str(), "AddAction" | "ModifyAction" | "RemoveAction")
                        })
                        .cloned()
                        .unwrap_or_else(|| "Unscoped".to_string());
                    if !KNOWN_CATALOGS.contains(&name.as_str()) {
                        parsed.unknown_catalogs.push(name.clone());
                    }
                    catalogs.push(CatalogScope {
                        source_file: source_file.clone(),
                        action,
                        name: name.clone(),
                        declared_member_count: attrs
                            .get("membercount")
                            .and_then(|v| v.parse().ok()),
                        observed_entries: 0,
                    });
                }
                stack.push(name);
            }
            Event::Empty(event) => {
                let name = event.local_name().as_ref().to_string();
                let attrs = attributes(&event)?;
                if !root_seen {
                    if name != "FMSaveAsXML" {
                        bail!("unsupported XML root <{name}>; expected <FMSaveAsXML>")
                    }
                    root_seen = true;
                    parsed.application = Some(application(&attrs));
                    source_file = parsed
                        .application
                        .as_ref()
                        .and_then(|application| application.file_name.clone())
                        .unwrap_or_else(|| "unknown-file".to_string());
                }
                if name == "DDR_INFO" {
                    parsed.ddr_info_present = true;
                }
                observe_entry(&name, &attrs, &stack, &mut catalogs);
                if name.ends_with("Catalog") {
                    let action = stack
                        .iter()
                        .rev()
                        .find(|item| {
                            matches!(item.as_str(), "AddAction" | "ModifyAction" | "RemoveAction")
                        })
                        .cloned()
                        .unwrap_or_else(|| "Unscoped".to_string());
                    if !KNOWN_CATALOGS.contains(&name.as_str()) {
                        parsed.unknown_catalogs.push(name.clone());
                    }
                    merge_catalog(
                        &mut parsed.catalogs,
                        CatalogScope {
                            source_file: source_file.clone(),
                            action,
                            name,
                            declared_member_count: attrs
                                .get("membercount")
                                .and_then(|v| v.parse().ok()),
                            observed_entries: 0,
                        },
                    );
                }
            }
            Event::End(event) => {
                let name = event.local_name().as_ref().to_string();
                if name.ends_with("Catalog")
                    && let Some(scope) = catalogs.pop()
                {
                    if scope.name == name {
                        merge_catalog(&mut parsed.catalogs, scope);
                    } else {
                        bail!("malformed catalog nesting near </{name}>")
                    }
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !root_seen {
        bail!("empty XML input; expected <FMSaveAsXML>")
    }
    Ok(parsed)
}

fn attributes(event: &quick_xml::events::BytesStart<'_>) -> Result<BTreeMap<String, String>> {
    event
        .attributes()
        .map(|attribute| {
            let attribute = attribute?;
            let key = attribute.key.local_name().as_ref().to_string();
            let value = attribute
                .normalized_value(XmlVersion::default())?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn application(attrs: &BTreeMap<String, String>) -> Application {
    let file_name = attrs.get("File").and_then(|value| {
        value
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .map(str::to_string)
    });
    Application {
        file_name,
        uuid: attrs.get("UUID").cloned(),
        export_version: attrs.get("version").cloned(),
        filemaker_version: attrs.get("Source").cloned(),
        locale: attrs.get("locale").cloned(),
        input_encoding: String::new(),
        ddr_info_present: false,
    }
}

fn observe_entry(
    name: &str,
    attrs: &BTreeMap<String, String>,
    stack: &[String],
    catalogs: &mut [CatalogScope],
) {
    let Some(scope) = catalogs.last_mut() else {
        return;
    };
    let parent = stack.last().map(String::as_str);
    let under_object_list =
        parent == Some("ObjectList") && stack.len() >= 2 && stack[stack.len() - 2] == scope.name;
    let expected = catalog_entry_name(&scope.name);
    let is_expected = expected == Some(name);
    let unknown_catalog_member = expected.is_none() && attrs.contains_key("id");
    if (parent == Some(scope.name.as_str()) || under_object_list)
        && (is_expected || unknown_catalog_member)
    {
        scope.observed_entries += 1;
    }
    if name == "ObjectList"
        && parent == Some(scope.name.as_str())
        && let Some(count) = attrs
            .get("membercount")
            .and_then(|value| value.parse::<u64>().ok())
    {
        scope.declared_member_count.get_or_insert(count);
    }
}

fn catalog_entry_name(catalog: &str) -> Option<&'static str> {
    Some(match catalog {
        "AccountsCatalog" => "Account",
        "BaseDirectoryCatalog" => "BaseDirectory",
        "BaseTableCatalog" => "BaseTable",
        "CustomFunctionsCatalog" => "CustomFunction",
        "CustomMenuCatalog" => "CustomMenu",
        "CustomMenuSetCatalog" => "CustomMenuSet",
        "ExtendedPrivilegesCatalog" => "ExtendedPrivilege",
        "ExternalDataSourceCatalog" => "ExternalDataSource",
        "FieldCatalog" => "Field",
        "LayoutCatalog" => "Layout",
        "LibraryCatalog" => "BinaryData",
        "PrivilegeSetsCatalog" => "PrivilegeSet",
        "RelationshipCatalog" => "Relationship",
        "ScriptCatalog" => "Script",
        "TableOccurrenceCatalog" => "TableOccurrence",
        "ThemeCatalog" => "Theme",
        "ValueListCatalog" => "ValueList",
        _ => return None,
    })
}

fn merge_catalog(catalogs: &mut BTreeMap<(String, String, String), Catalog>, scope: CatalogScope) {
    let key = (
        scope.source_file.clone(),
        scope.action.clone(),
        scope.name.clone(),
    );
    let catalog = catalogs.entry(key).or_insert_with(|| Catalog {
        source_file: scope.source_file,
        action: scope.action,
        name: scope.name,
        ..Catalog::default()
    });
    catalog.occurrences += 1;
    catalog.observed_entries += scope.observed_entries;
    if let Some(count) = scope.declared_member_count {
        *catalog.declared_member_count.get_or_insert(0) += count;
    }
}

fn finish_inventory(mut parsed: ParsedXml, input_encoding: String) -> Result<Inventory> {
    let mut application = parsed
        .application
        .take()
        .context("missing FileMaker file metadata")?;
    application.input_encoding = input_encoding;
    application.ddr_info_present = parsed.ddr_info_present;
    let source_file = application.file_name.as_deref().unwrap_or("unknown-file");
    let present: Vec<&str> = parsed
        .catalogs
        .values()
        .map(|catalog| catalog.name.as_str())
        .collect();
    let mut not_observed: Vec<String> = KNOWN_CATALOGS
        .iter()
        .filter(|name| !present.contains(name))
        .map(|name| format!("{source_file}: {name}"))
        .collect();
    not_observed.sort();
    parsed.unknown_catalogs.sort();
    parsed.unknown_catalogs.dedup();
    let mut diagnostics: Vec<String> = parsed
        .unknown_catalogs
        .iter()
        .map(|name| {
            format!("{source_file}: unrecognized catalog <{name}> was recorded; entry counts use recognized entry elements when available.")
        })
        .collect();
    if !not_observed.is_empty() {
        diagnostics.push(format!(
            "Known catalogs not observed: {}. This may mean empty or excluded catalogs, not necessarily an incomplete export.",
            not_observed.join(", ")
        ));
    }
    Ok(Inventory {
        schema_version: 2,
        source_format: "FMSaveAsXML".to_string(),
        applications: vec![application],
        catalogs: parsed.catalogs.into_values().collect(),
        known_catalogs_not_observed: not_observed,
        diagnostics,
    })
}

fn render_markdown(inventory: &Inventory) -> String {
    let title = inventory
        .applications
        .first()
        .and_then(|app| app.file_name.as_deref())
        .unwrap_or("FileMaker application");
    let mut markdown = format!(
        "# FileMaker inventory: {}\n\n- Source format: {}\n- Input files: {}\n\n",
        md(title),
        inventory.source_format,
        inventory.applications.len()
    );
    markdown.push_str("## Applications\n\n| File | UUID | Export version | FileMaker version | Locale | Encoding | DDR_INFO |\n|---|---|---|---|---|---|---|\n");
    for app in &inventory.applications {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            md_or_dash(app.file_name.as_deref()),
            md_or_dash(app.uuid.as_deref()),
            md_or_dash(app.export_version.as_deref()),
            md_or_dash(app.filemaker_version.as_deref()),
            md_or_dash(app.locale.as_deref()),
            md(&app.input_encoding),
            if app.ddr_info_present {
                "present"
            } else {
                "not present"
            },
        ));
    }
    markdown.push_str("\n## Catalog inventory\n\n| File | Action | Catalog | Occurrences | Declared members | Observed entries |\n|---|---|---|---:|---:|---:|\n");
    for catalog in &inventory.catalogs {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            md(&catalog.source_file),
            md(&catalog.action),
            md(&catalog.name),
            catalog.occurrences,
            catalog
                .declared_member_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "—".to_string()),
            catalog.observed_entries,
        ));
    }
    markdown.push_str("\nCatalog counts describe entries present in the export by action; additions and modifications are not presented as total application contents.\n\n");
    if !inventory.known_catalogs_not_observed.is_empty() {
        markdown.push_str("## Known catalogs not observed\n\n");
        for catalog in &inventory.known_catalogs_not_observed {
            markdown.push_str(&format!("- `{}`\n", md(catalog)));
        }
        markdown.push_str("\nAbsence can mean an empty catalog or one excluded from export.\n\n");
    }
    if !inventory.diagnostics.is_empty() {
        markdown.push_str("## Diagnostics\n\n");
        for diagnostic in &inventory.diagnostics {
            markdown.push_str(&format!("- {}\n", md(diagnostic)));
        }
        markdown.push('\n');
    }
    markdown.push_str("## Coverage\n\nCatalog inventory reports source entries by file and action. Component pages preserve available XML properties for extracted catalogs; account names and privilege-set roles are included, while passwords and credentials are redacted. Explicit references are resolved where the target is present and unambiguous; runtime behavior is not inferred.\n");
    markdown
        .push_str("\n[Component pages](objects/index.md) · [Typed relationships](edges.json) · [Permission matrix](security/permission-matrix.md) · [Integrations](integrations/overview.md) · [Workflow candidates](workflows/index.md) · [Coverage](coverage.json) · [Migration blueprint](migration/README.md) · [Agent guide](agent/AGENTS.md) · [Bundle README](README.md)\n");
    markdown
}

fn write_component_docs(
    output: &Path,
    components: &[Component],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    let directory = output.join("objects");
    fs::create_dir_all(&directory).with_context(|| {
        format!(
            "cannot create component documentation directory {}",
            directory.display()
        )
    })?;
    let current_pages: std::collections::BTreeSet<String> =
        indexes.filenames.values().cloned().collect();
    let tables: BTreeMap<&str, &Component> = components
        .iter()
        .filter(|component| component.kind == "table" && component.source_action == "AddAction")
        .filter_map(|component| component.xml_id.as_deref().map(|id| (id, component)))
        .collect();
    let mut index = String::from("# Extracted components\n\n");
    for component in components {
        let filename = indexes.page_path(component);
        index.push_str(&format!(
            "- [{}](./{}): {} — `{}` (`{}`)\n",
            md(&component.name),
            filename,
            component.kind,
            md(&component.object_id),
            md(&component.source_action),
        ));
        let page_path = directory.join(filename);
        if let Some(parent) = page_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            page_path,
            render_component_page(component, &tables, indexes),
        )
        .with_context(|| format!("cannot write component page {filename}"))?;
    }
    fs::write(directory.join("index.md"), index)?;
    remove_obsolete_component_pages(&directory, &current_pages)?;
    let manifest_path = directory.join("pages-manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&ComponentPageManifest {
            schema_version: 2,
            pages: current_pages.iter().cloned().collect(),
        })?,
    )?;
    Ok(())
}

fn write_permission_matrix(
    output: &Path,
    components: &[Component],
    indexes: &ComponentIndexes<'_>,
) -> Result<()> {
    let directory = output.join("security");
    fs::create_dir_all(&directory)?;
    let mut markdown = String::from(
        "# Exported access matrix\n\nThis report transcribes access settings present in each privilege-set source record. It does not merge Add/Modify actions or prove effective runtime access. `Calculation` means access is conditional; the calculation is preserved on the linked privilege-set page.\n\n## General access settings\n\n| File | Action | Privilege set | Account names | Category | Settings | Evidence |\n|---|---|---|---|---|---|---|\n",
    );
    let privilege_sets: Vec<_> = components
        .iter()
        .filter(|component| component.kind == "privilege_set")
        .collect();
    for privilege_set in &privilege_sets {
        let accounts = accounts_for_privilege_set(privilege_set, components);
        let mut access_nodes = Vec::new();
        collect_named_nodes(&privilege_set.properties, "access", &mut access_nodes);
        for access in &access_nodes {
            for section in &access.children {
                markdown.push_str(&format!(
                    "| {} | {} | [{}](../objects/{}) | {} | {} | {} | `{}` |\n",
                    md(&privilege_set.source_file),
                    md(&privilege_set.source_action),
                    md(&privilege_set.name),
                    indexes.page_path(privilege_set),
                    md(if accounts.is_empty() {
                        "—"
                    } else {
                        accounts.as_str()
                    }),
                    md(&section.name),
                    md(&format_access_section(section)),
                    md(&privilege_set.source_path),
                ));
            }
        }
    }
    if privilege_sets.is_empty() {
        markdown.push_str("\nNo privilege-set records were present in the supplied exports.\n");
    } else {
        markdown.push_str("\n## Table record rules\n\n| File | Action | Privilege set | Account names | Table | View | Edit | Create | Delete | Fields | Evidence |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        for privilege_set in &privilege_sets {
            let mut access_nodes = Vec::new();
            collect_named_nodes(&privilege_set.properties, "access", &mut access_nodes);
            let accounts = accounts_for_privilege_set(privilege_set, components);
            let mut table_nodes = Vec::new();
            for access in &access_nodes {
                collect_named_nodes(access, "Table", &mut table_nodes);
            }
            for table in table_nodes {
                let reference = first_named_node(table, "BaseTableReference")
                    .or_else(|| first_named_node(table, "TableOccurrenceReference"));
                let target = reference
                    .and_then(|reference| reference.attributes.get("name"))
                    .cloned()
                    .or_else(|| {
                        reference.and_then(|reference| {
                            reference
                                .attributes
                                .get("id")
                                .map(|id| format!("unresolved id {id}"))
                        })
                    })
                    .unwrap_or_else(|| "unresolved table target".to_string());
                markdown.push_str(&format!(
                    "| {} | {} | [{}](../objects/{}) | {} | {} | {} | {} | {} | {} | {} | `{}` |\n",
                    md(&privilege_set.source_file),
                    md(&privilege_set.source_action),
                    md(&privilege_set.name),
                    indexes.page_path(privilege_set),
                    md(if accounts.is_empty() {
                        "—"
                    } else {
                        accounts.as_str()
                    }),
                    md(&target),
                    md(access_setting(table, "View")),
                    md(access_setting(table, "Edit")),
                    md(access_setting(table, "Create")),
                    md(access_setting(table, "Delete")),
                    md(access_setting(table, "Fields")),
                    md(&privilege_set.source_path),
                ));
            }
        }
        markdown.push_str("\n## Explicit field-access rules\n\n| File | Action | Privilege set | Account names | Table | Field | Access | Evidence |\n|---|---|---|---|---|---|---|---|\n");
        for privilege_set in &privilege_sets {
            let accounts = accounts_for_privilege_set(privilege_set, components);
            let mut tables = Vec::new();
            collect_named_nodes(&privilege_set.properties, "Table", &mut tables);
            for table in tables {
                let table_name = first_named_node(table, "BaseTableReference")
                    .or_else(|| first_named_node(table, "TableOccurrenceReference"))
                    .and_then(|reference| reference.attributes.get("name"))
                    .map(String::as_str)
                    .unwrap_or("unresolved table");
                let mut field_groups = Vec::new();
                collect_named_nodes(table, "Fields", &mut field_groups);
                for group in field_groups {
                    let mut fields = Vec::new();
                    collect_named_nodes(group, "Field", &mut fields);
                    for field in fields {
                        let field_name = first_named_node(field, "FieldReference")
                            .and_then(|reference| reference.attributes.get("name"))
                            .or_else(|| field.attributes.get("name"))
                            .map(String::as_str)
                            .unwrap_or("unresolved field");
                        markdown.push_str(&format!(
                            "| {} | {} | {} | {} | {} | {} | {} | `{}` |\n",
                            md(&privilege_set.source_file),
                            md(&privilege_set.source_action),
                            md(&privilege_set.name),
                            md(if accounts.is_empty() {
                                "—"
                            } else {
                                accounts.as_str()
                            }),
                            md(table_name),
                            md(field_name),
                            md(field
                                .attributes
                                .get("access")
                                .map(String::as_str)
                                .unwrap_or("not specified")),
                            md(&privilege_set.source_path)
                        ));
                    }
                }
            }
        }
        markdown.push_str("\n## Explicit object-level access rules\n\n| File | Action | Privilege set | Account names | Category | Object | Settings | Evidence |\n|---|---|---|---|---|---|---|---|---|\n");
        for privilege_set in &privilege_sets {
            let accounts = accounts_for_privilege_set(privilege_set, components);
            let mut access_nodes = Vec::new();
            collect_named_nodes(&privilege_set.properties, "access", &mut access_nodes);
            for access in access_nodes {
                for category_name in ["Layouts", "Scripts", "ValueLists", "ExtendedPrivileges"] {
                    let mut categories = Vec::new();
                    collect_named_nodes(access, category_name, &mut categories);
                    for category in categories {
                        let object_name = match category_name {
                            "Layouts" => "Layout",
                            "Scripts" => "Script",
                            "ValueLists" => "ValueList",
                            _ => "ExtendedPrivilege",
                        };
                        let mut objects = Vec::new();
                        collect_named_nodes(category, object_name, &mut objects);
                        for object in objects {
                            let reference_name = match object_name {
                                "Layout" => "LayoutReference",
                                "Script" => "ScriptReference",
                                "ValueList" => "ValueListReference",
                                _ => "ExtendedPrivilegeReference",
                            };
                            let name = first_named_node(object, reference_name)
                                .and_then(|reference| reference.attributes.get("name"))
                                .map(String::as_str)
                                .unwrap_or("unresolved object");
                            markdown.push_str(&format!(
                                "| {} | {} | {} | {} | {} | {} | {} | `{}` |\n",
                                md(&privilege_set.source_file),
                                md(&privilege_set.source_action),
                                md(&privilege_set.name),
                                md(if accounts.is_empty() {
                                    "—"
                                } else {
                                    accounts.as_str()
                                }),
                                md(category_name),
                                md(name),
                                md(&format_access_section(object)),
                                md(&privilege_set.source_path)
                            ));
                        }
                    }
                }
            }
        }
        markdown.push_str("\nAccount names and assigned privilege sets are included; passwords and credentials remain redacted in source object records. Settings are transcribed per exported action and are not an effective-access calculation. Field rows appear only where the XML contains explicit per-field entries; table-level `Fields access` settings are reported above and are not expanded into invented field rows.\n");
    }
    fs::write(directory.join("permission-matrix.md"), markdown)?;
    Ok(())
}

fn source_file_identity(component: &Component) -> &str {
    component
        .object_id
        .split(':')
        .next()
        .filter(|identity| !identity.is_empty())
        .unwrap_or(&component.source_file)
}

fn account_uses_privilege_set(account: &Component, privilege_set: &Component) -> bool {
    let mut references = Vec::new();
    collect_named_nodes(
        &account.properties,
        "PrivilegeSetReference",
        &mut references,
    );
    references.into_iter().any(|reference| {
        if let Some(uuid) = reference.attributes.get("UUID") {
            privilege_set.xml_uuid.as_deref() == Some(uuid.as_str())
                && reference
                    .attributes
                    .get("id")
                    .is_none_or(|id| privilege_set.xml_id.as_deref() == Some(id.as_str()))
        } else {
            reference
                .attributes
                .get("id")
                .is_some_and(|id| privilege_set.xml_id.as_deref() == Some(id.as_str()))
        }
    })
}

fn accounts_for_privilege_set(privilege_set: &Component, components: &[Component]) -> String {
    components
        .iter()
        .filter(|component| {
            component.kind == "account"
                && source_file_identity(component) == source_file_identity(privilege_set)
                && account_uses_privilege_set(component, privilege_set)
        })
        .map(|component| format!("{} ({})", component.name, component.source_action))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_access_section(section: &XmlNode) -> String {
    let settings = section
        .attributes
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>();
    if settings.is_empty() {
        "No section attributes; inspect linked source for details".to_string()
    } else {
        settings.join(", ")
    }
}

fn access_setting<'a>(table: &'a XmlNode, name: &str) -> &'a str {
    first_named_node(table, name)
        .and_then(|node| node.attributes.get("access"))
        .map(String::as_str)
        .unwrap_or("not specified")
}

fn first_named_node<'a>(node: &'a XmlNode, name: &str) -> Option<&'a XmlNode> {
    if node.name == name {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| first_named_node(child, name))
}

fn collect_named_nodes<'a>(node: &'a XmlNode, name: &str, output: &mut Vec<&'a XmlNode>) {
    if node.name == name {
        output.push(node);
    }
    for child in &node.children {
        collect_named_nodes(child, name, output);
    }
}

fn remove_obsolete_component_pages(
    directory: &Path,
    current_pages: &std::collections::BTreeSet<String>,
) -> Result<()> {
    let manifest_path = directory.join("pages-manifest.json");
    let old_pages = if manifest_path.exists() {
        let manifest: ComponentPageManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        if !matches!(manifest.schema_version, 1 | 2) {
            bail!(
                "unsupported component page manifest schema version {}",
                manifest.schema_version
            )
        }
        manifest.pages
    } else {
        fs::read_dir(directory)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !is_generated_page_name(&name) {
                    return None;
                }
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).ok()?;
                if !metadata.file_type().is_file() {
                    return None;
                }
                let content = fs::read_to_string(path).ok()?;
                (content.contains("- Object ID: `")
                    && content.contains("Properties preserve the available XML structure."))
                .then_some(name)
            })
            .collect()
    };
    for name in old_pages {
        if !current_pages.contains(&name) && is_generated_page_path(&name) {
            let path = directory.join(name);
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

fn is_generated_page_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".md") else {
        return false;
    };
    let mut parts = stem.splitn(3, '-');
    let (Some(number), Some(kind), Some(slug)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    number.len() == 5
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(
            kind,
            "table"
                | "field"
                | "base_directory"
                | "external_data_source"
                | "table_occurrence"
                | "custom_function"
                | "value_list"
                | "relationship"
                | "custom_menu"
                | "custom_menu_set"
                | "script"
                | "theme"
                | "layout"
                | "resource"
                | "privilege_set"
                | "extended_privilege"
                | "account"
                | "layout_object"
                | "script_step_group"
                | "custom_function_calculation"
                | "value_list_options"
                | "unresolved_script_step_group"
                | "unresolved_custom_function_calculation"
                | "unresolved_value_list_options"
                | "unknown_catalog_item"
        )
        && !slug.is_empty()
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_generated_page_path(name: &str) -> bool {
    let path = Path::new(name);
    let parts: Vec<_> = path.components().collect();
    match parts.as_slice() {
        [std::path::Component::Normal(filename)] => {
            is_generated_page_name(&filename.to_string_lossy())
        }
        [
            std::path::Component::Normal(kind),
            std::path::Component::Normal(filename),
        ] => {
            let filename = filename.to_string_lossy();
            is_generated_page_name(&filename)
                && filename.split('-').nth(1) == Some(kind.to_string_lossy().as_ref())
        }
        _ => false,
    }
}

fn render_component_page(
    component: &Component,
    tables: &BTreeMap<&str, &Component>,
    indexes: &ComponentIndexes<'_>,
) -> String {
    let title = component.kind.replace('_', " ");
    let mut markdown = format!(
        "# {}: {}\n\n- Object ID: `{}`\n- Record ID: `{}`\n- Source: `{}`\n- Action: `{}`\n- XML path: `{}`\n",
        title,
        md(&component.name),
        md(&component.object_id),
        md(&component.record_id),
        md(&component.source_file),
        md(&component.source_action),
        md(&component.source_path),
    );
    if component.kind == "field"
        && let Some(table_id) = component.table_id.as_deref()
    {
        match tables.get(table_id) {
            Some(table) => {
                if let Some(filename) = indexes.filenames.get(table.record_id.as_str()) {
                    markdown.push_str(&format!(
                        "- Table: [{}](../{})\n",
                        md(&table.name),
                        filename
                    ));
                }
            }
            None => markdown.push_str(&format!(
                "- Table reference: unresolved `{}`\n",
                md(table_id)
            )),
        }
    }
    if component.kind == "layout_object"
        && let Some(parent_id) = component.parent_object_id.as_deref()
        && let Some(parent) = indexes.by_id.get(parent_id)
        && let Some(filename) = indexes.filenames.get(parent.record_id.as_str())
    {
        markdown.push_str(&format!(
            "- Parent: [{}](../{})\n",
            md(&parent.name),
            filename
        ));
    }
    if component.kind == "table" {
        markdown.push_str("\n## Fields\n\n");
        if let Some(table_id) = component.xml_id.as_deref() {
            for field in indexes
                .fields_by_table_id
                .get(table_id)
                .into_iter()
                .flatten()
            {
                if let Some(filename) = indexes.filenames.get(field.record_id.as_str()) {
                    markdown.push_str(&format!("- [{}](../{})\n", md(&field.name), filename));
                }
            }
        }
        markdown.push('\n');
    }
    if matches!(component.kind.as_str(), "layout" | "layout_object")
        && indexes
            .children_by_parent
            .contains_key(&(component.object_id.as_str(), component.record_id.as_str()))
    {
        markdown.push_str("\n## Child objects\n\n");
        if let Some(children) = indexes
            .children_by_parent
            .get(&(component.object_id.as_str(), component.record_id.as_str()))
        {
            for object in children {
                if let Some(filename) = indexes.filenames.get(object.record_id.as_str()) {
                    markdown.push_str(&format!("- [{}](../{})\n", md(&object.name), filename));
                }
            }
        }
        markdown.push('\n');
    }
    markdown.push_str("## Properties\n\n| Property | Value |\n|---|---|\n");
    let mut properties = Vec::new();
    components::render_properties(&component.properties, "", &mut properties);
    for (property, value) in properties {
        markdown.push_str(&format!("| {} | {} |\n", md(&property), md(&value)));
    }
    render_attached_records(&mut markdown, component);
    if !component.findings.is_empty() {
        markdown.push_str("\n## Static analysis\n\n");
        for finding in &component.findings {
            markdown.push_str(&format!(
                "- **{}** ({}): {}. Evidence: {}.{}\n",
                md(&finding.kind),
                md(&finding.status),
                md(&finding.summary),
                md(&finding.evidence.join(", ")),
                if finding.limitations.is_empty() {
                    String::new()
                } else {
                    format!(" Limitations: {}", md(&finding.limitations.join("; ")))
                }
            ));
        }
    }
    let related_edges = indexes.edges_by_object_id.get(component.object_id.as_str());
    if related_edges.is_some_and(|edges| !edges.is_empty()) {
        markdown.push_str("\n## Relationships\n\n");
        for edge in related_edges.into_iter().flatten() {
            let source = edge
                .from_record_id
                .as_deref()
                .or(edge.from_object_id.as_deref())
                .unwrap_or("unknown");
            let target = edge
                .to_record_id
                .as_deref()
                .or(edge.to_object_id.as_deref())
                .unwrap_or("unresolved");
            markdown.push_str(&format!(
                "- `{}` → `{}` ({}) — {} — evidence: `{}`{}\n",
                md(source),
                md(target),
                md(&edge.kind),
                md(&edge.status),
                md(&edge.evidence.join(", ")),
                if edge.limitations.is_empty() {
                    String::new()
                } else {
                    format!(" — limitations: {}", md(&edge.limitations.join("; ")))
                }
            ));
        }
    }
    for diagnostic in &component.diagnostics {
        markdown.push_str(&format!("\nDiagnostic: {}\n", md(diagnostic)));
    }
    markdown.push_str("\nProperties preserve the available XML structure. Static findings are evidence-backed and do not execute or claim runtime behavior.\n");
    if component.kind == "account" {
        markdown.push_str("Account names and privilege-set roles are included; passwords and credentials are redacted.\n");
    }
    if component.kind == "resource" {
        markdown.push_str(
            "Embedded binary payloads are omitted; available resource metadata is preserved.\n",
        );
    }
    markdown
}

fn render_attached_records(markdown: &mut String, component: &Component) {
    if component.attached_records.is_empty() {
        if component.kind == "script" {
            markdown.push_str(
                "\n## Script steps\n\nNo step group was present for this script in the export.\n",
            );
        }
        return;
    }
    let title = match component.kind.as_str() {
        "script" => "Script steps",
        "custom_function" => "Calculation body",
        "value_list" => "Value list options",
        _ => "Attached source records",
    };
    markdown.push_str(&format!("\n## {title}\n\n"));
    for record in &component.attached_records {
        if record.kind == "script_step" {
            markdown.push_str(&format!(
                "### {}. {}\n\n- Record ID: `{}`\n- XML path: `{}`\n\n| Property | Value |\n|---|---|\n",
                record.order,
                md(&record.name),
                md(&record.record_id),
                md(&record.source_path),
            ));
        } else {
            markdown.push_str(&format!(
                "### {}\n\n- Type: `{}`\n- Record ID: `{}`\n- XML path: `{}`\n\n| Property | Value |\n|---|---|\n",
                md(&record.name),
                md(&record.kind),
                md(&record.record_id),
                md(&record.source_path),
            ));
        }
        let mut properties = Vec::new();
        components::render_properties(&record.properties, "", &mut properties);
        for (property, value) in properties {
            markdown.push_str(&format!("| {} | {} |\n", md(&property), md(&value)));
        }
    }
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
        "unnamed".to_string()
    } else {
        slug.to_string()
    }
}

fn md(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('`', "&#96;")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
}

fn md_or_dash(value: Option<&str>) -> String {
    value.map(md).unwrap_or_else(|| "—".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const FIXTURE: &str = r#"<?xml version="1.0"?>
<FMSaveAsXML version="2.2.2.0" Source="21.0.1" File="../Example.fmp12" UUID="file-uuid" locale="English" userName="PRIVATE PERSON">
  <Structure><AddAction><BaseTableCatalog membercount="1"><UUID>change-id</UUID><BaseTable id="1" name="Customers"/></BaseTableCatalog>
  <FieldsForTables><FieldCatalog><ObjectList membercount="2"><Field id="1" name="Name"/><Field id="2" name="Email"/></ObjectList></FieldCatalog></FieldsForTables>
  <FutureCatalog membercount="1"><FutureThing id="1"/></FutureCatalog></AddAction>
  <ModifyAction><LayoutCatalog membercount="1"><Layout name="Welcome"/></LayoutCatalog>
  <CustomMenuSetCatalog membercount="1"><CustomMenuSetReference id="1"/><ObjectList><CustomMenuSet id="1"/></ObjectList></CustomMenuSetCatalog></ModifyAction>
  </Structure>
  <DDR_INFO/>
</FMSaveAsXML>"#;

    #[test]
    fn inventories_catalogs_and_redacts_unselected_identity_attributes() {
        let parsed = parse_xml(Cursor::new(FIXTURE)).unwrap();
        let inventory = finish_inventory(parsed, "UTF-8".to_string()).unwrap();
        let serialized = serde_json::to_string(&inventory).unwrap();
        assert_eq!(
            inventory.applications[0].file_name.as_deref(),
            Some("Example.fmp12")
        );
        assert_eq!(
            inventory
                .catalogs
                .iter()
                .find(|item| item.name == "FieldCatalog")
                .unwrap()
                .observed_entries,
            2
        );
        assert_eq!(
            inventory
                .catalogs
                .iter()
                .find(|item| item.name == "LayoutCatalog")
                .unwrap()
                .observed_entries,
            1
        );
        assert_eq!(
            inventory
                .catalogs
                .iter()
                .find(|item| item.name == "CustomMenuSetCatalog")
                .unwrap()
                .observed_entries,
            1
        );
        assert!(inventory.applications[0].ddr_info_present);
        assert!(serialized.contains("FutureCatalog"));
        assert!(!serialized.contains("PRIVATE PERSON"));
        assert!(!serialized.contains("../Example.fmp12"));
    }

    #[test]
    fn rejects_malformed_xml_and_non_filemaker_formats() {
        assert!(parse_xml(Cursor::new("<FMSaveAsXML><broken></FMSaveAsXML>")).is_err());
        assert!(parse_xml(Cursor::new("<DatabaseDesignReport/>")).is_err());
    }

    #[test]
    fn decodes_utf16_xml_with_a_bom() {
        let mut xml = vec![0xff, 0xfe];
        for unit in FIXTURE.encode_utf16() {
            xml.extend_from_slice(&unit.to_le_bytes());
        }
        let parsed = parse_xml(Cursor::new(xml)).unwrap();
        assert_eq!(
            parsed.application.unwrap().file_name.as_deref(),
            Some("Example.fmp12")
        );
    }

    #[test]
    fn outputs_are_deterministic() {
        let first = finish_inventory(
            parse_xml(Cursor::new(FIXTURE)).unwrap(),
            "UTF-8".to_string(),
        )
        .unwrap();
        let second = finish_inventory(
            parse_xml(Cursor::new(FIXTURE)).unwrap(),
            "UTF-8".to_string(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(render_markdown(&first), render_markdown(&second));
    }

    #[test]
    fn generates_deterministic_pre_html_bundle_and_machine_artifacts() {
        let directory = std::env::temp_dir().join(format!(
            "fmblueprint-bundle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let input = directory.join("input.xml");
        let output = directory.join("bundle");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&input, FIXTURE).unwrap();

        run_analysis(vec![input.clone()], Some(output.clone())).unwrap();
        validate_bundle(&output).unwrap();
        let artifacts = [
            "manifest.json",
            "coverage.json",
            "diagnostics.json",
            "graphs/relationships.json",
            "migration/README.md",
            "agent/context-map.json",
            "agent/requirements.jsonl",
        ];
        let first: Vec<_> = artifacts
            .iter()
            .map(|name| fs::read(output.join(name)).unwrap())
            .collect();
        assert!(output.join("schemas/workflow.schema.json").is_file());
        assert!(output.join("schemas/diagnostic.schema.json").is_file());
        assert!(output.join("reports/unresolved-references.md").is_file());
        for line in fs::read_to_string(output.join("objects.jsonl"))
            .unwrap()
            .lines()
        {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }

        run_analysis(vec![input], Some(output.clone())).unwrap();
        let second: Vec<_> = artifacts
            .iter()
            .map(|name| fs::read(output.join(name)).unwrap())
            .collect();
        assert_eq!(first, second);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn removes_only_obsolete_generated_component_pages() {
        let directory = std::env::temp_dir().join(format!(
            "fmblueprint-pages-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let stale = directory.join("00001-table-obsolete.md");
        let user_file = directory.join("notes.md");
        fs::write(
            &stale,
            "# Table: old\n- Object ID: `old`\nProperties preserve the available XML structure.\n",
        )
        .unwrap();
        fs::write(&user_file, "Keep this file\n").unwrap();

        remove_obsolete_component_pages(
            &directory,
            &std::collections::BTreeSet::from(["00002-field-current.md".to_string()]),
        )
        .unwrap();

        assert!(!stale.exists());
        assert_eq!(fs::read_to_string(&user_file).unwrap(), "Keep this file\n");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn defaults_output_folder_to_input_filename_stem() {
        assert_eq!(
            default_output_dir(&[PathBuf::from("samples/sample-small.xml")]),
            PathBuf::from("output/sample-small")
        );
    }
}
