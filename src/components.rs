use std::{
    collections::BTreeMap,
    io::{BufReader, Read},
};

use anyhow::{Result, bail};
use quick_xml::{Reader, XmlVersion, encoding::DecodingReader, events::Event};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Component {
    pub record_id: String,
    pub object_id: String,
    pub kind: String,
    pub name: String,
    pub source_file: String,
    pub source_action: String,
    pub source_path: String,
    pub xml_id: Option<String>,
    pub xml_uuid: Option<String>,
    pub table_id: Option<String>,
    pub table_name: Option<String>,
    pub parent_object_id: Option<String>,
    pub parent_record_id: Option<String>,
    pub properties: XmlNode,
    pub attached_records: Vec<AttachedRecord>,
    pub findings: Vec<AnalysisFinding>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttachedRecord {
    pub record_id: String,
    pub kind: String,
    pub name: String,
    pub order: usize,
    pub source_path: String,
    pub properties: XmlNode,
    pub findings: Vec<AnalysisFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisFinding {
    pub kind: String,
    pub status: String,
    pub summary: String,
    pub evidence: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct XmlNode {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub text: String,
    pub children: Vec<XmlNode>,
    #[serde(skip)]
    redact_content: bool,
    #[serde(skip)]
    suppress_text: bool,
    #[serde(skip)]
    redact_text_candidate: bool,
}

struct FieldCatalogContext {
    table_id: Option<String>,
    table_name: Option<String>,
}

struct CaptureContext {
    kind: &'static str,
    catalog: &'static str,
    action: String,
    source_file: String,
    table_id: Option<String>,
    table_name: Option<String>,
    ordinal: usize,
}

pub fn extract_data_model<R: Read>(source: R) -> Result<Vec<Component>> {
    let mut reader = Reader::from_reader(DecodingReader::new(BufReader::new(source)));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut field_catalogs: Vec<FieldCatalogContext> = Vec::new();
    let mut capture: Vec<XmlNode> = Vec::new();
    let mut capture_context: Option<CaptureContext> = None;
    let mut components = Vec::new();
    let mut source_file = String::new();
    let mut file_uuid = String::new();
    let mut object_ordinal = 0usize;
    let mut root_seen = false;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                let name = event.local_name().as_ref().to_string();
                let attrs = xml_attributes(&event)?;
                if !root_seen {
                    if name != "FMSaveAsXML" {
                        bail!("unsupported XML root <{name}>; expected <FMSaveAsXML>")
                    }
                    root_seen = true;
                    source_file = attrs
                        .get("File")
                        .and_then(|value| value.rsplit(['/', '\\']).find(|part| !part.is_empty()))
                        .unwrap_or("unknown-file")
                        .to_string();
                    file_uuid = attrs.get("UUID").cloned().unwrap_or_default();
                }

                if name == "FieldCatalog" {
                    field_catalogs.push(FieldCatalogContext {
                        table_id: None,
                        table_name: None,
                    });
                } else if name == "BaseTableReference"
                    && path.last().map(String::as_str) == Some("FieldCatalog")
                    && let Some(context) = field_catalogs.last_mut()
                {
                    context.table_id = attrs.get("id").cloned();
                    context.table_name = attrs.get("name").cloned();
                }

                if capture.is_empty() {
                    if let Some((kind, catalog)) = component_kind(&name, &path) {
                        object_ordinal += 1;
                        let field_context = field_catalogs.last();
                        capture_context = Some(CaptureContext {
                            kind,
                            catalog,
                            action: current_action(&path),
                            source_file: source_file.clone(),
                            table_id: if kind == "field" {
                                field_context.and_then(|context| context.table_id.clone())
                            } else {
                                None
                            },
                            table_name: if kind == "field" {
                                field_context.and_then(|context| context.table_name.clone())
                            } else {
                                None
                            },
                            ordinal: object_ordinal,
                        });
                        capture.push(XmlNode::new(name.clone(), attrs, false, kind == "resource"));
                    }
                } else {
                    let redact = capture
                        .last()
                        .is_some_and(|node| node.redact_content || node.redact_text_candidate)
                        || is_sensitive_content_element(&name, &attrs);
                    let suppress_text = capture.last().is_some_and(|node| node.suppress_text)
                        || is_binary_payload(&name);
                    capture.push(XmlNode::new(name.clone(), attrs, redact, suppress_text));
                }
                path.push(name);
            }
            Event::Empty(event) => {
                let name = event.local_name().as_ref().to_string();
                let attrs = xml_attributes(&event)?;
                if !root_seen {
                    if name != "FMSaveAsXML" {
                        bail!("unsupported XML root <{name}>; expected <FMSaveAsXML>")
                    }
                    root_seen = true;
                    source_file = attrs
                        .get("File")
                        .and_then(|value| value.rsplit(['/', '\\']).find(|part| !part.is_empty()))
                        .unwrap_or("unknown-file")
                        .to_string();
                    file_uuid = attrs.get("UUID").cloned().unwrap_or_default();
                }
                if name == "BaseTableReference"
                    && path.last().map(String::as_str) == Some("FieldCatalog")
                    && let Some(context) = field_catalogs.last_mut()
                {
                    context.table_id = attrs.get("id").cloned();
                    context.table_name = attrs.get("name").cloned();
                }
                if capture.is_empty() {
                    if let Some((kind, catalog)) = component_kind(&name, &path) {
                        object_ordinal += 1;
                        let field_context = field_catalogs.last();
                        let context = CaptureContext {
                            kind,
                            catalog,
                            action: current_action(&path),
                            source_file: source_file.clone(),
                            table_id: if kind == "field" {
                                field_context.and_then(|context| context.table_id.clone())
                            } else {
                                None
                            },
                            table_name: if kind == "field" {
                                field_context.and_then(|context| context.table_name.clone())
                            } else {
                                None
                            },
                            ordinal: object_ordinal,
                        };
                        components.push(component_from_node(
                            XmlNode::new(name, attrs, false, kind == "resource"),
                            context,
                            &file_uuid,
                        ));
                    }
                } else if let Some(parent) = capture.last_mut() {
                    let redact = parent.redact_content
                        || parent.redact_text_candidate
                        || is_sensitive_content_element(&name, &attrs);
                    let suppress_text = parent.suppress_text || is_binary_payload(&name);
                    parent
                        .children
                        .push(XmlNode::new(name, attrs, redact, suppress_text));
                }
            }
            Event::Text(event) => {
                if let Some(node) = capture.last_mut() {
                    let text = event.xml_content(XmlVersion::default());
                    if node.redact_content || node.suppress_text {
                        node.text = "[REDACTED]".to_string();
                    } else if !text.trim().is_empty() {
                        if node.redact_text_candidate {
                            node.text = "[REDACTED]".to_string();
                            node.redact_content = true;
                        } else {
                            node.text.push_str(&text);
                        }
                    }
                }
            }
            Event::CData(event) => {
                if let Some(node) = capture.last_mut() {
                    let text = event.xml_content(XmlVersion::default());
                    if node.redact_content || node.suppress_text {
                        node.text = "[REDACTED]".to_string();
                    } else if !text.trim().is_empty() {
                        if node.redact_text_candidate {
                            node.text = "[REDACTED]".to_string();
                            node.redact_content = true;
                        } else {
                            node.text.push_str(&text);
                        }
                    }
                }
            }
            Event::GeneralRef(event) => {
                if let Some(node) = capture.last_mut() {
                    if node.redact_content || node.suppress_text {
                        node.text = "[REDACTED]".to_string();
                    } else if node.redact_text_candidate {
                        node.text = "[REDACTED]".to_string();
                        node.redact_content = true;
                    } else {
                        let reference = event.xml_content(XmlVersion::default());
                        let value = match event.resolve_char_ref()? {
                            Some(character) => character.to_string(),
                            None => match reference.as_ref() {
                                "amp" => "&".to_string(),
                                "lt" => "<".to_string(),
                                "gt" => ">".to_string(),
                                "apos" => "'".to_string(),
                                "quot" => "\"".to_string(),
                                _ => format!("&{reference};"),
                            },
                        };
                        node.text.push_str(&value);
                    }
                }
            }
            Event::End(event) => {
                let name = event.local_name().as_ref().to_string();
                if !capture.is_empty()
                    && let Some(node) = capture.pop()
                {
                    if let Some(parent) = capture.last_mut() {
                        parent.children.push(node);
                    } else if let Some(context) = capture_context.take() {
                        components.push(component_from_node(node, context, &file_uuid));
                    }
                }
                if name == "FieldCatalog" {
                    field_catalogs.pop();
                }
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !root_seen {
        bail!("empty XML input; expected <FMSaveAsXML>")
    }
    let (detached, mut components): (Vec<_>, Vec<_>) = components
        .into_iter()
        .partition(|component| is_detached_kind(&component.kind));
    let unresolved = attach_detached_records(&mut components, detached);
    components.extend(unresolved);
    let table_records: Vec<_> = components
        .iter()
        .filter(|component| component.kind == "table")
        .filter_map(|component| {
            component.xml_id.as_deref().map(|xml_id| {
                (
                    xml_id.to_string(),
                    component.object_id.clone(),
                    component.record_id.clone(),
                    component.source_action.clone(),
                )
            })
        })
        .collect();
    for component in &mut components {
        if component.kind == "field" {
            let matching_tables: Vec<_> = component
                .table_id
                .as_deref()
                .map(|table_id| {
                    table_records
                        .iter()
                        .filter(|(id, _, _, _)| id == table_id)
                        .collect()
                })
                .unwrap_or_default();
            let table = matching_tables
                .iter()
                .find(|(_, _, _, action)| action == &component.source_action)
                .or_else(|| matching_tables.first());
            component.parent_object_id = table.map(|(_, object_id, _, _)| object_id.clone());
            component.parent_record_id = table.map(|(_, _, record_id, _)| record_id.clone());
        }
    }
    let mut layout_objects = Vec::new();
    for layout in components.iter().filter(|item| item.kind == "layout") {
        let mut ordinal = 0;
        collect_layout_objects(
            layout,
            &layout.properties,
            &mut ordinal,
            &mut layout_objects,
        );
    }
    components.extend(layout_objects);
    components.sort_by(|left, right| {
        left.object_id
            .cmp(&right.object_id)
            .then(left.record_id.cmp(&right.record_id))
    });
    Ok(components)
}

fn is_detached_kind(kind: &str) -> bool {
    matches!(
        kind,
        "script_step_group" | "custom_function_calculation" | "value_list_options"
    )
}

impl XmlNode {
    fn new(
        name: String,
        attributes: BTreeMap<String, String>,
        inherited_redaction: bool,
        inherited_suppression: bool,
    ) -> Self {
        let redact_content =
            inherited_redaction || is_sensitive_content_element(&name, &attributes);
        let redact_text_candidate = !redact_content && is_password_text_element(&name);
        let suppress_text = inherited_suppression || is_binary_payload(&name);
        let attributes = attributes
            .into_iter()
            .map(|(key, value)| {
                let value = if redact_content || is_sensitive_attribute(&key) {
                    "[REDACTED]".to_string()
                } else {
                    value
                };
                (key, value)
            })
            .collect();
        Self {
            name,
            attributes,
            text: if redact_content {
                "[REDACTED]".to_string()
            } else {
                String::new()
            },
            children: Vec::new(),
            redact_content,
            suppress_text,
            redact_text_candidate,
        }
    }

    fn child_text(&self, name: &str) -> Option<&str> {
        self.children
            .iter()
            .find(|child| child.name == name)
            .map(|child| child.text.as_str())
    }
}

fn component_kind(name: &str, path: &[String]) -> Option<(&'static str, &'static str)> {
    if name.ends_with("Reference") && current_action(path) != "RemoveAction" {
        return None;
    }
    if name == "Script" && path.last().map(String::as_str) == Some("StepsForScripts") {
        return Some(("script_step_group", "StepsForScripts/Script"));
    }
    if name == "CustomFunctionCalc"
        && path.last().map(String::as_str) == Some("ObjectList")
        && path.iter().rev().nth(1).map(String::as_str) == Some("CalcsForCustomFunctions")
    {
        return Some((
            "custom_function_calculation",
            "CalcsForCustomFunctions/ObjectList/CustomFunctionCalc",
        ));
    }
    if name == "ValueList" && path.last().map(String::as_str) == Some("OptionsForValueLists") {
        return Some(("value_list_options", "OptionsForValueLists/ValueList"));
    }
    let (catalog, kind) = match name {
        "BaseDirectory" | "BaseDirectoryReference" => ("BaseDirectoryCatalog", "base_directory"),
        "ExternalDataSource" | "ExternalDataSourceReference" => {
            ("ExternalDataSourceCatalog", "external_data_source")
        }
        "BaseTable" | "BaseTableReference" => ("BaseTableCatalog", "table"),
        "TableOccurrence" | "TableOccurrenceReference" => {
            ("TableOccurrenceCatalog", "table_occurrence")
        }
        "CustomFunction" | "CustomFunctionReference" => {
            ("CustomFunctionsCatalog", "custom_function")
        }
        "ValueList" | "ValueListReference" => ("ValueListCatalog", "value_list"),
        "Field" | "FieldReference" => ("FieldCatalog", "field"),
        "Relationship" | "RelationshipReference" => ("RelationshipCatalog", "relationship"),
        "CustomMenu" | "CustomMenuReference" => ("CustomMenuCatalog", "custom_menu"),
        "CustomMenuSet" | "CustomMenuSetReference" => ("CustomMenuSetCatalog", "custom_menu_set"),
        "Script" | "ScriptReference" => ("ScriptCatalog", "script"),
        "Theme" | "ThemeReference" => ("ThemeCatalog", "theme"),
        "Layout" | "LayoutReference" => ("LayoutCatalog", "layout"),
        "BinaryData" => ("LibraryCatalog", "resource"),
        "PrivilegeSet" | "PrivilegeSetReference" => ("PrivilegeSetsCatalog", "privilege_set"),
        "ExtendedPrivilege" | "ExtendedPrivilegeReference" => {
            ("ExtendedPrivilegesCatalog", "extended_privilege")
        }
        "Account" | "AccountReference" => ("AccountsCatalog", "account"),
        _ => return None,
    };
    let parent_catalog = if path.last().map(String::as_str) == Some("ObjectList") {
        path.iter().rev().nth(1).map(String::as_str)
    } else {
        path.last().map(String::as_str)
    };
    if parent_catalog != Some(catalog) {
        return None;
    }
    Some((kind, catalog))
}

fn current_action(path: &[String]) -> String {
    path.iter()
        .rev()
        .find(|item| matches!(item.as_str(), "AddAction" | "ModifyAction" | "RemoveAction"))
        .cloned()
        .unwrap_or_else(|| "Unscoped".to_string())
}

fn component_from_node(properties: XmlNode, context: CaptureContext, file_uuid: &str) -> Component {
    let xml_id = properties.attributes.get("id").cloned();
    let xml_uuid = properties.attributes.get("UUID").cloned().or_else(|| {
        properties
            .child_text("UUID")
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let uuid = xml_uuid.as_deref();
    let identity = uuid
        .map(str::to_string)
        .or_else(|| xml_id.clone())
        .unwrap_or_else(|| format!("entry-{}", context.ordinal));
    let kind_id = if context.kind == "field" {
        context.table_id.as_deref().unwrap_or("unscoped")
    } else {
        "file"
    };
    let object_id = format!("{}:{}:{}:{}", file_uuid, context.kind, kind_id, identity);
    let name = if context.kind == "account" {
        account_name(&properties)
            .map(str::to_string)
            .unwrap_or_else(|| format!("account {identity}"))
    } else {
        properties
            .attributes
            .get("name")
            .cloned()
            .unwrap_or_else(|| format!("{} {}", context.kind, identity))
    };
    let catalog = match context.catalog {
        "StepsForScripts/Script" => "StepsForScripts/Script",
        "CalcsForCustomFunctions/ObjectList/CustomFunctionCalc" => {
            "CalcsForCustomFunctions/ObjectList/CustomFunctionCalc"
        }
        "OptionsForValueLists/ValueList" => "OptionsForValueLists/ValueList",
        "BaseDirectoryCatalog" => "BaseDirectoryCatalog/BaseDirectory",
        "ExternalDataSourceCatalog" => "ExternalDataSourceCatalog/ExternalDataSource",
        "BaseTableCatalog" => "BaseTableCatalog/BaseTable",
        "TableOccurrenceCatalog" => "TableOccurrenceCatalog/TableOccurrence",
        "CustomFunctionsCatalog" => "CustomFunctionsCatalog/ObjectList/CustomFunction",
        "ValueListCatalog" => "ValueListCatalog/ValueList",
        "FieldCatalog" => "FieldsForTables/FieldCatalog/ObjectList/Field",
        "RelationshipCatalog" => "RelationshipCatalog/Relationship",
        "CustomMenuCatalog" => "CustomMenuCatalog/CustomMenu",
        "CustomMenuSetCatalog" => "CustomMenuSetCatalog/ObjectList/CustomMenuSet",
        "ScriptCatalog" => "ScriptCatalog/Script",
        "ThemeCatalog" => "ThemeCatalog/Theme",
        "LayoutCatalog" => "LayoutCatalog/Layout",
        "LibraryCatalog" => "LibraryCatalog/BinaryData",
        "PrivilegeSetsCatalog" => "PrivilegeSetsCatalog/ObjectList/PrivilegeSet",
        "ExtendedPrivilegesCatalog" => "ExtendedPrivilegesCatalog/ObjectList/ExtendedPrivilege",
        _ => "AccountsCatalog/ObjectList/Account",
    };
    let detached_reference = match context.kind {
        "script_step_group" => Some("ScriptReference"),
        "custom_function_calculation" => Some("CustomFunctionReference"),
        "value_list_options" => Some("ValueListReference"),
        _ => None,
    }
    .and_then(|reference_name| {
        properties
            .children
            .iter()
            .find(|child| child.name == reference_name)
            .and_then(|reference| {
                reference
                    .attributes
                    .get("id")
                    .or_else(|| reference.attributes.get("UUID"))
                    .map(|identity| (reference_name, identity))
            })
    });
    let source_path = if let Some((reference_name, identity)) = detached_reference {
        format!(
            "/FMSaveAsXML/Structure/{}/{catalog}[{reference_name}={identity}]",
            context.action
        )
    } else if context.kind == "field" {
        format!(
            "/FMSaveAsXML/Structure/{}/{catalog}[table={}][id={}]",
            context.action,
            context.table_id.as_deref().unwrap_or("unresolved"),
            xml_id.as_deref().unwrap_or(&identity)
        )
    } else {
        format!(
            "/FMSaveAsXML/Structure/{}/{catalog}[id={}]",
            context.action,
            xml_id.as_deref().unwrap_or(&identity)
        )
    };
    let record_id = format!("{object_id}:record:{}:{}", context.action, context.ordinal);
    Component {
        record_id,
        object_id,
        kind: context.kind.to_string(),
        name,
        source_file: context.source_file,
        source_action: context.action,
        source_path,
        xml_id,
        xml_uuid,
        table_id: context.table_id,
        table_name: context.table_name,
        parent_object_id: None,
        parent_record_id: None,
        properties,
        attached_records: Vec::new(),
        findings: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn collect_layout_objects(
    parent: &Component,
    node: &XmlNode,
    ordinal: &mut usize,
    output: &mut Vec<Component>,
) {
    for child in &node.children {
        if child.name == "LayoutObject" {
            *ordinal += 1;
            let xml_id = child.attributes.get("id").cloned();
            let xml_uuid = child.attributes.get("UUID").cloned().or_else(|| {
                child
                    .child_text("UUID")
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
            let identity = xml_uuid
                .clone()
                .or_else(|| xml_id.clone())
                .unwrap_or_else(|| format!("entry-{ordinal}"));
            let name = child
                .attributes
                .get("name")
                .cloned()
                .unwrap_or_else(|| format!("layout object {identity}"));
            let object_id = format!("{}:layout_object:{}", parent.object_id, identity);
            let record_id = format!(
                "{}:layout_object:{}:{}",
                parent.record_id, identity, ordinal
            );
            let component = Component {
                record_id,
                object_id,
                kind: "layout_object".to_string(),
                name,
                source_file: parent.source_file.clone(),
                source_action: parent.source_action.clone(),
                source_path: format!(
                    "{}/LayoutObject[id={}]",
                    parent.source_path,
                    xml_id.as_deref().unwrap_or(&identity)
                ),
                xml_id,
                xml_uuid,
                table_id: None,
                table_name: None,
                parent_object_id: Some(parent.object_id.clone()),
                parent_record_id: Some(parent.record_id.clone()),
                properties: child.clone(),
                attached_records: Vec::new(),
                findings: Vec::new(),
                diagnostics: Vec::new(),
            };
            collect_layout_objects(&component, child, ordinal, output);
            output.push(component);
        } else {
            collect_layout_objects(parent, child, ordinal, output);
        }
    }
}

fn xml_attributes(event: &quick_xml::events::BytesStart<'_>) -> Result<BTreeMap<String, String>> {
    event
        .attributes()
        .map(|attribute| {
            let attribute = attribute?;
            let key = attribute.key.local_name().as_ref().to_string();
            let value = attribute
                .normalized_value(XmlVersion::default())?
                .into_owned();
            if is_sensitive_name(&key) {
                Ok(Some((key, "[REDACTED]".to_string())))
            } else {
                Ok(Some((key, value)))
            }
        })
        .filter_map(|entry: Result<Option<(String, String)>>| match entry {
            Ok(Some(attribute)) => Some(Ok(attribute)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn is_sensitive_name(name: &str) -> bool {
    is_sensitive_attribute(name)
}

fn is_sensitive_attribute(name: &str) -> bool {
    let name = compact_name(name);
    name.contains("credential") || (name.contains("password") && name != "savepassword")
}

fn is_sensitive_content_element(name: &str, attributes: &BTreeMap<String, String>) -> bool {
    let name = compact_name(name);
    if name.contains("credential") {
        return true;
    }
    if name.contains("password") && name != "password" && name != "savepassword" {
        return true;
    }
    name == "password"
        && attributes
            .keys()
            .any(|key| matches!(compact_name(key).as_str(), "value" | "data" | "text"))
}

fn is_password_text_element(name: &str) -> bool {
    compact_name(name) == "password"
}

fn compact_name(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn account_name(properties: &XmlNode) -> Option<&str> {
    properties
        .children
        .iter()
        .find(|node| node.name == "Authentication")
        .and_then(|authentication| {
            authentication
                .children
                .iter()
                .find(|node| node.name == "AccountName")
        })
        .and_then(|account_name| {
            account_name
                .children
                .iter()
                .find(|node| node.name == "INSECURE_TEXT")
        })
        .map(|name| name.text.as_str())
        .filter(|name| !name.is_empty())
}

fn attach_detached_records(
    components: &mut [Component],
    detached: Vec<Component>,
) -> Vec<Component> {
    let mut unresolved = Vec::new();
    for (group_order, mut record) in detached.into_iter().enumerate() {
        let (owner_kind, reference_name) = match record.kind.as_str() {
            "script_step_group" => ("script", "ScriptReference"),
            "custom_function_calculation" => ("custom_function", "CustomFunctionReference"),
            "value_list_options" => ("value_list", "ValueListReference"),
            _ => continue,
        };
        let Some(reference) = record
            .properties
            .children
            .iter()
            .find(|node| node.name == reference_name)
        else {
            record.diagnostics.push(format!(
                "detached {} record has no {reference_name}",
                record.kind
            ));
            record.kind = format!("unresolved_{}", record.kind);
            unresolved.push(record);
            continue;
        };
        let owner_index = resolve_owner(components, owner_kind, &record, reference);
        let Some(owner_index) = owner_index else {
            record.diagnostics.push(format!(
                "could not uniquely resolve {reference_name} to a {owner_kind} object"
            ));
            record.kind = format!("unresolved_{}", record.kind);
            unresolved.push(record);
            continue;
        };
        let owner = &mut components[owner_index];
        if record.kind == "script_step_group" {
            let group_order = owner
                .attached_records
                .iter()
                .filter(|attached| attached.kind == "script_step_group")
                .count();
            let object_list_index = record
                .properties
                .children
                .iter()
                .position(|node| node.name == "ObjectList");
            let steps = object_list_index
                .map(|index| {
                    record.properties.children[index]
                        .children
                        .iter()
                        .filter(|node| node.name == "Step")
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut group_properties = record.properties.clone();
            if let Some(index) = object_list_index {
                group_properties.children[index]
                    .children
                    .retain(|node| node.name != "Step");
            }
            let group_name = reference
                .attributes
                .get("name")
                .cloned()
                .unwrap_or_else(|| owner.name.clone());
            owner.attached_records.push(AttachedRecord {
                record_id: format!("{}:steps-group:{group_order}", owner.record_id),
                kind: "script_step_group".to_string(),
                name: group_name,
                order: group_order,
                source_path: record.source_path.clone(),
                properties: group_properties,
                findings: Vec::new(),
            });
            for (ordinal, step) in steps.iter().enumerate() {
                let order = step
                    .attributes
                    .get("index")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(ordinal);
                owner.attached_records.push(AttachedRecord {
                    record_id: format!("{}:step:{order}:{ordinal}", owner.record_id),
                    kind: "script_step".to_string(),
                    name: step
                        .attributes
                        .get("name")
                        .cloned()
                        .unwrap_or_else(|| "unnamed step".to_string()),
                    order,
                    source_path: format!("{}/ObjectList/Step[index={order}]", record.source_path),
                    properties: step.clone(),
                    findings: Vec::new(),
                });
            }
        } else {
            let name = reference
                .attributes
                .get("name")
                .cloned()
                .unwrap_or_else(|| record.name.clone());
            owner.attached_records.push(AttachedRecord {
                record_id: format!("{}:detail:{group_order}", owner.record_id),
                kind: record.kind.clone(),
                name,
                order: group_order,
                source_path: record.source_path,
                properties: record.properties,
                findings: Vec::new(),
            });
        }
    }
    unresolved
}

fn resolve_owner(
    components: &[Component],
    kind: &str,
    record: &Component,
    reference: &XmlNode,
) -> Option<usize> {
    let uuid = reference.attributes.get("UUID");
    let xml_id = reference.attributes.get("id");
    let candidates: Vec<_> = components
        .iter()
        .enumerate()
        .filter(|(_, component)| {
            component.kind == kind
                && component.source_file == record.source_file
                && component.source_action == record.source_action
        })
        .collect();
    if let Some(uuid) = uuid {
        let matching: Vec<_> = candidates
            .iter()
            .filter(|(_, component)| component.xml_uuid.as_ref() == Some(uuid))
            .collect();
        if let [(index, component)] = matching.as_slice()
            && xml_id.is_none_or(|id| component.xml_id.as_deref() == Some(id.as_str()))
        {
            return Some(*index);
        }
        return None;
    }
    let xml_id = xml_id?;
    let matching: Vec<_> = candidates
        .iter()
        .filter(|(_, component)| component.xml_id.as_deref() == Some(xml_id.as_str()))
        .collect();
    match matching.as_slice() {
        [(index, _)] => Some(*index),
        _ => None,
    }
}

fn is_binary_payload(name: &str) -> bool {
    matches!(name, "BinaryData" | "Base64Data" | "ContainerData")
}

pub fn render_properties(node: &XmlNode, prefix: &str, lines: &mut Vec<(String, String)>) {
    let key = if prefix.is_empty() {
        node.name.clone()
    } else {
        format!("{prefix}.{}", node.name)
    };
    for (name, value) in &node.attributes {
        lines.push((format!("{key}@{name}"), value.clone()));
    }
    if !node.text.trim().is_empty() {
        lines.push((key.clone(), node.text.trim().to_string()));
    }
    for child in &node.children {
        render_properties(child, &key, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::io::Cursor;

    #[test]
    fn extracts_tables_and_fields_with_properties_and_table_context() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <BaseTableCatalog><BaseTable id="7" name="Contacts"><UUID>table-uuid</UUID></BaseTable></BaseTableCatalog>
  <FieldsForTables><FieldCatalog><BaseTableReference id="7" name="Contacts"/><ObjectList>
            <Field id="3" name="Email" datatype="Text"><UUID>field-uuid</UUID><Validation notEmpty="True"/></Field>
          </ObjectList></FieldCatalog></FieldsForTables>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        assert_eq!(components.len(), 2);
        let field = components.iter().find(|item| item.kind == "field").unwrap();
        assert_eq!(field.table_id.as_deref(), Some("7"));
        assert_eq!(field.table_name.as_deref(), Some("Contacts"));
        assert_eq!(field.object_id, "file-id:field:7:field-uuid");
        assert_eq!(field.properties.children[1].attributes["notEmpty"], "True");
    }

    #[test]
    fn preserves_source_metadata_and_redacts_password_credentials() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <BaseTableCatalog><BaseTable id="7" name="Contacts"><UUID userName="PRIVATE PERSON">table-uuid</UUID>
            <Authentication>authentication metadata</Authentication><Password value="private-password"/><Password prohibitModification="True"/><SavePassword keychain="True"/></BaseTable></BaseTableCatalog>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        let json = serde_json::to_string(&components).unwrap();
        assert!(json.contains("PRIVATE PERSON"));
        assert!(json.contains("authentication metadata"));
        assert!(!json.contains("private-password"));
        assert!(json.contains("prohibitModification"));
        assert!(json.contains("keychain"));
        assert!(json.contains("[REDACTED]"));
    }

    #[test]
    fn extracts_account_names_and_roles_while_redacting_passwords() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <RelationshipCatalog><Relationship id="8"><LeftTable id="1"/><RightTable id="2"/></Relationship></RelationshipCatalog>
          <AccountsCatalog><ObjectList><Account id="9"><Authentication><AccountName><INSECURE_TEXT>account@example.test</INSECURE_TEXT></AccountName><INSECURE_PASSWORD>private-password</INSECURE_PASSWORD></Authentication>
            <PrivilegeSetReference id="2" name="Editor"/></Account></ObjectList></AccountsCatalog>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        assert!(components.iter().any(|item| item.kind == "relationship"));
        let account = components
            .iter()
            .find(|item| item.kind == "account")
            .unwrap();
        let json = serde_json::to_string(account).unwrap();
        assert_eq!(account.name, "account@example.test");
        assert!(json.contains("account@example.test"));
        assert!(json.contains("Editor"));
        assert!(!json.contains("private-password"));
        assert!(json.contains("[REDACTED]"));
    }

    #[test]
    fn extracts_layout_objects_as_children_of_layout_pages() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <LayoutCatalog><Layout id="2" name="Overview"><ObjectList>
            <LayoutObject id="10" name="Title"><Text>Heading</Text></LayoutObject>
            <LayoutObject id="11" name="Group"><LayoutObject id="12" name="Button"/></LayoutObject>
          </ObjectList></Layout></LayoutCatalog>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        let objects: Vec<_> = components
            .iter()
            .filter(|component| component.kind == "layout_object")
            .collect();
        assert_eq!(objects.len(), 3);
        let layout = components
            .iter()
            .find(|component| component.kind == "layout")
            .unwrap();
        assert!(objects.iter().any(|object| {
            object.name == "Title" && object.parent_object_id.as_deref() == Some(&layout.object_id)
        }));
        assert!(objects.iter().any(|object| {
            object.name == "Button"
                && objects.iter().any(|parent| {
                    parent.name == "Group"
                        && object.parent_object_id.as_deref() == Some(&parent.object_id)
                })
        }));
    }

    #[test]
    fn attaches_script_steps_custom_function_bodies_and_value_list_options() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <ScriptCatalog><Script id="1" name="Runner"><UUID>script-uuid</UUID></Script><Script id="4" name="Empty"><UUID>empty-script-uuid</UUID></Script></ScriptCatalog>
          <StepsForScripts><Script><ScriptReference id="1" name="Runner" UUID="script-uuid"/><ObjectList membercount="2">
            <Step id="68" index="0" name="If" enable="True"><Options>0</Options><ParameterValues><Parameter type="Calculation"><Calculation><Text>1 = 1</Text></Calculation></Parameter></ParameterValues></Step>
            <Step id="103" index="1" name="Exit Script" enable="True"><Options>0</Options><ParameterValues/></Step>
          </ObjectList></Script><Script><ScriptReference id="4" name="Empty" UUID="empty-script-uuid"/><ObjectList membercount="0"/></Script></StepsForScripts>
          <CustomFunctionsCatalog><ObjectList><CustomFunction id="2" name="Double"><UUID>function-uuid</UUID></CustomFunction></ObjectList></CustomFunctionsCatalog>
          <CalcsForCustomFunctions><ObjectList><CustomFunctionCalc><CustomFunctionReference id="2" name="Double" UUID="function-uuid"/><Calculation><Text>$value * 2</Text></Calculation></CustomFunctionCalc></ObjectList></CalcsForCustomFunctions>
          <ValueListCatalog><ValueList id="3" name="Status"><UUID>value-list-uuid</UUID></ValueList></ValueListCatalog>
          <OptionsForValueLists><ValueList><ValueListReference id="3" name="Status" UUID="value-list-uuid"/><Source value="Custom"/><CustomValues>Open,Closed</CustomValues></ValueList></OptionsForValueLists>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        let script = components
            .iter()
            .find(|item| item.name == "Runner")
            .unwrap();
        let steps: Vec<_> = script
            .attached_records
            .iter()
            .filter(|record| record.kind == "script_step")
            .collect();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].name, "If");
        assert_eq!(steps[0].order, 0);
        assert!(
            script
                .attached_records
                .iter()
                .any(|record| record.kind == "script_step_group")
        );
        let empty_script = components.iter().find(|item| item.name == "Empty").unwrap();
        assert_eq!(empty_script.attached_records.len(), 1);
        assert_eq!(empty_script.attached_records[0].kind, "script_step_group");
        assert_eq!(
            empty_script.attached_records[0].properties.children[1].attributes["membercount"],
            "0"
        );
        assert!(serde_json::to_string(script).unwrap().contains("1 = 1"));
        let function = components
            .iter()
            .find(|item| item.kind == "custom_function")
            .unwrap();
        assert_eq!(
            function.attached_records[0].kind,
            "custom_function_calculation"
        );
        assert!(
            serde_json::to_string(function)
                .unwrap()
                .contains("$value * 2")
        );
        let value_list = components
            .iter()
            .find(|item| item.kind == "value_list")
            .unwrap();
        assert_eq!(value_list.attached_records[0].kind, "value_list_options");
        assert!(
            serde_json::to_string(value_list)
                .unwrap()
                .contains("Open,Closed")
        );
        assert!(
            !components
                .iter()
                .any(|item| item.kind == "script_step_group")
        );
    }

    #[test]
    fn keeps_add_modify_remove_records_separate_with_shared_logical_identity() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure>
          <AddAction><ScriptCatalog><Script id="7" name="Runner"><UUID>script-uuid</UUID></Script></ScriptCatalog></AddAction>
          <ModifyAction><ScriptCatalog><Script id="7" name="Runner"><UUID>script-uuid</UUID><Options runwithfullaccess="True"/></Script></ScriptCatalog></ModifyAction>
          <RemoveAction><ScriptCatalog><ScriptReference id="7" name="Runner" UUID="script-uuid"/></ScriptCatalog></RemoveAction>
        </Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        let records: Vec<_> = components
            .iter()
            .filter(|item| item.kind == "script")
            .collect();
        assert_eq!(records.len(), 3);
        assert!(
            records
                .iter()
                .all(|item| item.object_id == records[0].object_id)
        );
        assert_eq!(
            records
                .iter()
                .map(|item| item.record_id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
        assert_eq!(
            records
                .iter()
                .map(|item| item.source_action.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn preserves_detached_content_when_uuid_and_numeric_id_conflict() {
        let xml = br#"<FMSaveAsXML File="Example.fmp12" UUID="file-id"><Structure><AddAction>
          <ScriptCatalog><Script id="1" name="Runner"><UUID>script-uuid</UUID></Script></ScriptCatalog>
          <StepsForScripts><Script><ScriptReference id="99" name="Runner" UUID="script-uuid"/><ObjectList><Step id="103" index="0" name="Exit Script"/></ObjectList></Script></StepsForScripts>
        </AddAction></Structure></FMSaveAsXML>"#;
        let components = extract_data_model(Cursor::new(xml)).unwrap();
        let unresolved = components
            .iter()
            .find(|item| item.kind == "unresolved_script_step_group")
            .unwrap();
        assert!(unresolved.diagnostics[0].contains("could not uniquely resolve"));
        assert!(
            serde_json::to_string(unresolved)
                .unwrap()
                .contains("Exit Script")
        );
    }
}
