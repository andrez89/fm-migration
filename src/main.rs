use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufReader, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
mod analysis;
mod components;
use analysis::{
    AnalysisEdge, WorkflowCandidate, analyze_component_references, analyze_scripts,
    discover_workflows,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Analyze { input, out }) => run_analysis(input, out),
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
    write_component_docs(&output, &components, &edges)?;
    write_permission_matrix(&output, &components)?;
    write_integration_docs(&output, &components, &edges, &workflows)?;
    println!("Inventory written to {}", output.display());
    Ok(())
}

fn write_integration_docs(
    output: &Path,
    components: &[Component],
    edges: &[AnalysisEdge],
    workflows: &[WorkflowCandidate],
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
        let references: Vec<_> = edges
            .iter()
            .filter(|edge| {
                edge.kind == "accesses_external_source"
                    && edge.to_object_id.as_deref() == Some(source.object_id.as_str())
            })
            .collect();
        markdown.push_str(&format!(
            "- **{}** (`{}`; action `{}`): {} explicit reference(s). Source record: `objects/{}`.\n",
            md(&source.name), md(&source.object_id), md(&source.source_action), references.len(), component_page_relative_path(source, components)
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
        let entry_link = components
            .iter()
            .find(|component| {
                component.object_id == workflow.entry_object_id
                    && component.source_action == "AddAction"
            })
            .or_else(|| {
                components
                    .iter()
                    .find(|component| component.object_id == workflow.entry_object_id)
            })
            .map(|component| {
                format!(
                    "[{}](../objects/{})",
                    md(&component_display_name(component)),
                    component_page_relative_path(component, components)
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
                    let object = components
                        .iter()
                        .find(|component| {
                            component.object_id == *id && component.source_action == "AddAction"
                        })
                        .or_else(|| {
                            components
                                .iter()
                                .find(|component| component.object_id == *id)
                        });
                    if let Some(object) = object {
                        page.push_str(&format!(
                            "- [{}](../objects/{}) (`{}`)\n",
                            md(&component_display_name(object)),
                            component_page_relative_path(object, components),
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
        .push_str("\n[Component pages](objects/index.md) · [Typed relationships](edges.json) · [Permission matrix](security/permission-matrix.md) · [Integrations](integrations/overview.md) · [Workflow candidates](workflows/index.md)\n");
    markdown
}

fn write_component_docs(
    output: &Path,
    components: &[Component],
    edges: &[AnalysisEdge],
) -> Result<()> {
    let directory = output.join("objects");
    fs::create_dir_all(&directory).with_context(|| {
        format!(
            "cannot create component documentation directory {}",
            directory.display()
        )
    })?;
    let filenames: BTreeMap<&str, String> = components
        .iter()
        .enumerate()
        .map(|(index, component)| {
            (
                component.record_id.as_str(),
                format!(
                    "{}/{:05}-{}-{}.md",
                    component.kind,
                    index + 1,
                    component.kind,
                    slug(&component.name)
                ),
            )
        })
        .collect();
    let current_pages: std::collections::BTreeSet<String> = filenames.values().cloned().collect();
    let tables: BTreeMap<&str, &Component> = components
        .iter()
        .filter(|component| component.kind == "table" && component.source_action == "AddAction")
        .filter_map(|component| component.xml_id.as_deref().map(|id| (id, component)))
        .collect();
    let mut index = String::from("# Extracted components\n\n");
    for component in components {
        let filename = &filenames[component.record_id.as_str()];
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
            render_component_page(component, &filenames, &tables, components, edges),
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

fn write_permission_matrix(output: &Path, components: &[Component]) -> Result<()> {
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
                    component_page_relative_path(privilege_set, components),
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
                    component_page_relative_path(privilege_set, components),
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

fn component_page_relative_path(component: &Component, components: &[Component]) -> String {
    let index = components
        .iter()
        .position(|candidate| candidate.record_id == component.record_id)
        .unwrap_or_default()
        + 1;
    format!(
        "{}/{:05}-{}-{}.md",
        component.kind,
        index,
        component.kind,
        slug(&component.name)
    )
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
    filenames: &BTreeMap<&str, String>,
    tables: &BTreeMap<&str, &Component>,
    all: &[Component],
    edges: &[AnalysisEdge],
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
                if let Some(filename) = filenames.get(table.record_id.as_str()) {
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
        && let Some(parent) = all.iter().find(|item| item.object_id == parent_id)
        && let Some(filename) = filenames.get(parent.record_id.as_str())
    {
        markdown.push_str(&format!(
            "- Parent: [{}](../{})\n",
            md(&parent.name),
            filename
        ));
    }
    if component.kind == "table" {
        markdown.push_str("\n## Fields\n\n");
        for field in all.iter().filter(|item| {
            item.kind == "field"
                && component
                    .xml_id
                    .as_deref()
                    .is_some_and(|table_id| item.table_id.as_deref() == Some(table_id))
        }) {
            if let Some(filename) = filenames.get(field.record_id.as_str()) {
                markdown.push_str(&format!("- [{}](../{})\n", md(&field.name), filename));
            }
        }
        markdown.push('\n');
    }
    if matches!(component.kind.as_str(), "layout" | "layout_object")
        && all.iter().any(|item| {
            item.kind == "layout_object"
                && item.parent_object_id.as_deref() == Some(component.object_id.as_str())
                && item.parent_record_id.as_deref() == Some(component.record_id.as_str())
        })
    {
        markdown.push_str("\n## Child objects\n\n");
        for object in all.iter().filter(|item| {
            item.kind == "layout_object"
                && item.parent_object_id.as_deref() == Some(component.object_id.as_str())
                && item.parent_record_id.as_deref() == Some(component.record_id.as_str())
        }) {
            if let Some(filename) = filenames.get(object.record_id.as_str()) {
                markdown.push_str(&format!("- [{}](../{})\n", md(&object.name), filename));
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
    let related_edges: Vec<_> = edges
        .iter()
        .filter(|edge| {
            edge.from_object_id.as_deref() == Some(component.object_id.as_str())
                || edge.to_object_id.as_deref() == Some(component.object_id.as_str())
        })
        .collect();
    if !related_edges.is_empty() {
        markdown.push_str("\n## Relationships\n\n");
        for edge in related_edges {
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
