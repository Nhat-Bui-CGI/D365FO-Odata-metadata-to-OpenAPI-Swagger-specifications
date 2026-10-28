//! Streaming parser for EDMX / CSDL v4 `$metadata` documents.
//!
//! D365FO metadata is large (tens of MB), so this reads events with quick-xml
//! instead of building a DOM.

use std::collections::HashMap;
use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::Error;
use crate::edm::*;

pub fn parse_str(xml: &str) -> Result<Metadata, Error> {
    parse_reader(xml.as_bytes())
}

pub fn parse_reader<R: BufRead>(reader: R) -> Result<Metadata, Error> {
    let mut reader = Reader::from_reader(reader);
    reader.config_mut().trim_text(true);

    let mut md = Metadata::default();
    let mut namespace = String::new();
    let mut structured: Option<(StructuredType, bool)> = None; // (type, is_entity)
    let mut enum_type: Option<EnumType> = None;
    let mut operation: Option<Operation> = None;
    let mut saw_edmx = false;
    let mut buf = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::Xml(format!("{e} at byte {}", reader.buffer_position())))?;
        let (e, is_empty) = match &event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(end) => {
                match end.local_name().into_inner() {
                    "EntityType" | "ComplexType" => finish_structured(&mut md, structured.take()),
                    "EnumType" => md.enum_types.extend(enum_type.take()),
                    "Action" | "Function" => md.operations.extend(operation.take()),
                    _ => {}
                }
                buf.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buf.clear();
                continue;
            }
        };
        let a = attrs(e)?;
        let get = |k: &str| a.get(k).cloned();
        let flag = |k: &str| a.get(k).is_some_and(|v| v == "true");

        match e.local_name().into_inner() {
            "Edmx" => saw_edmx = true,
            "Schema" => {
                namespace = get("Namespace").unwrap_or_default();
                if let Some(alias) = get("Alias") {
                    md.aliases.insert(alias, namespace.clone());
                }
            }
            tag @ ("EntityType" | "ComplexType") => {
                let t = StructuredType {
                    namespace: namespace.clone(),
                    name: get("Name").unwrap_or_default(),
                    base_type: get("BaseType"),
                    is_abstract: flag("Abstract"),
                    ..Default::default()
                };
                let pending = Some((t, tag == "EntityType"));
                if is_empty {
                    finish_structured(&mut md, pending);
                } else {
                    structured = pending;
                }
            }
            "PropertyRef" => {
                if let (Some((t, _)), Some(name)) = (structured.as_mut(), get("Name")) {
                    t.key.push(name);
                }
            }
            "Property" => {
                if let Some((t, _)) = structured.as_mut() {
                    t.properties.push(Property {
                        name: get("Name").unwrap_or_default(),
                        type_name: get("Type").unwrap_or_else(|| "Edm.String".into()),
                        nullable: a.get("Nullable").is_none_or(|v| v != "false"),
                        max_length: get("MaxLength").and_then(|v| v.parse().ok()),
                        precision: get("Precision").and_then(|v| v.parse().ok()),
                        scale: get("Scale").and_then(|v| v.parse().ok()),
                    });
                }
            }
            "NavigationProperty" => {
                if let Some((t, _)) = structured.as_mut() {
                    t.navigation_properties.push(NavigationProperty {
                        name: get("Name").unwrap_or_default(),
                        type_name: get("Type").unwrap_or_default(),
                        nullable: a.get("Nullable").is_none_or(|v| v != "false"),
                        partner: get("Partner"),
                    });
                }
            }
            "EnumType" => {
                let t = EnumType {
                    namespace: namespace.clone(),
                    name: get("Name").unwrap_or_default(),
                    is_flags: flag("IsFlags"),
                    members: Vec::new(),
                };
                if is_empty {
                    md.enum_types.push(t);
                } else {
                    enum_type = Some(t);
                }
            }
            "Member" => {
                if let Some(t) = enum_type.as_mut() {
                    t.members.push(EnumMember {
                        name: get("Name").unwrap_or_default(),
                        value: get("Value").and_then(|v| v.parse().ok()),
                    });
                }
            }
            tag @ ("Action" | "Function") => {
                let op = Operation {
                    namespace: namespace.clone(),
                    name: get("Name").unwrap_or_default(),
                    is_function: tag == "Function",
                    is_bound: flag("IsBound"),
                    ..Default::default()
                };
                if is_empty {
                    md.operations.push(op);
                } else {
                    operation = Some(op);
                }
            }
            "Parameter" => {
                if let Some(op) = operation.as_mut() {
                    op.parameters.push(Parameter {
                        name: get("Name").unwrap_or_default(),
                        type_name: get("Type").unwrap_or_default(),
                        nullable: a.get("Nullable").is_none_or(|v| v != "false"),
                    });
                }
            }
            "ReturnType" => {
                if let Some(op) = operation.as_mut() {
                    op.return_type = get("Type");
                }
            }
            "EntitySet" => {
                if let (Some(name), Some(entity_type)) = (get("Name"), get("EntityType")) {
                    md.entity_sets.push(EntitySet { name, entity_type });
                }
            }
            _ => {}
        }
        buf.clear();
    }

    if !saw_edmx {
        return Err(Error::NotMetadata);
    }
    // Store entity set types with aliases expanded so lookups are simple.
    let normalized: Vec<String> = md
        .entity_sets
        .iter()
        .map(|s| md.normalize(&s.entity_type).into_owned())
        .collect();
    for (s, t) in md.entity_sets.iter_mut().zip(normalized) {
        s.entity_type = t;
    }
    Ok(md)
}

fn finish_structured(md: &mut Metadata, t: Option<(StructuredType, bool)>) {
    match t {
        Some((t, true)) => md.entity_types.push(t),
        Some((t, false)) => md.complex_types.push(t),
        None => {}
    }
}

fn attrs(e: &BytesStart) -> Result<HashMap<String, String>, Error> {
    let mut out = HashMap::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| Error::Xml(err.to_string()))?;
        let key = attr.key.local_name().into_inner().to_string();
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|err| Error::Xml(err.to_string()))?
            .into_owned();
        out.insert(key, value);
    }
    Ok(out)
}
