//! Builds an OpenAPI 3.0 / 3.1 or Swagger 2.0 document for a chosen set of
//! Dynamics 365 entity sets.

use std::collections::{BTreeSet, VecDeque};

use serde_json::{Map, Value, json};

use crate::edm::*;
use crate::{Error, Platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecVersion {
    Swagger2,
    OpenApi30,
    OpenApi31,
}

impl SpecVersion {
    pub const ALL: [SpecVersion; 3] = [Self::Swagger2, Self::OpenApi30, Self::OpenApi31];

    pub fn label(self) -> &'static str {
        match self {
            Self::Swagger2 => "Swagger 2.0",
            Self::OpenApi30 => "OpenAPI 3.0",
            Self::OpenApi31 => "OpenAPI 3.1",
        }
    }

    /// Short name usable in file names, e.g. `openapi-3.0`.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Swagger2 => "swagger-2.0",
            Self::OpenApi30 => "openapi-3.0",
            Self::OpenApi31 => "openapi-3.1",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub version: SpecVersion,
    pub title: String,
    /// Decides the service root path and platform-specific parameters.
    pub platform: Platform,
    /// Environment URL, e.g. `https://contoso.operations.dynamics.com`.
    pub server_url: Option<String>,
    /// Emit paths for bound actions and functions.
    pub include_operations: bool,
    /// Emit navigation properties whose target entity set is also selected.
    pub include_navigation: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            version: SpecVersion::OpenApi30,
            platform: Platform::FinanceOperations,
            title: "Dynamics 365 Finance & Operations OData API".into(),
            server_url: None,
            include_operations: true,
            include_navigation: true,
        }
    }
}

/// Generates a spec covering only the named entity sets.
pub fn generate(md: &Metadata, entity_sets: &[&str], opts: &Options) -> Result<Value, Error> {
    let mut sets = Vec::new();
    for name in entity_sets {
        let set = md
            .entity_set(name)
            .ok_or_else(|| Error::UnknownEntitySet(name.to_string()))?;
        let ty = md
            .entity_type(&set.entity_type)
            .ok_or_else(|| Error::UnknownType(set.entity_type.clone()))?;
        sets.push((set, ty));
    }
    if sets.is_empty() {
        return Err(Error::NothingSelected);
    }
    Gen::new(md, opts, &sets).run(&sets)
}

struct Gen<'a> {
    md: &'a Metadata,
    v: SpecVersion,
    opts: &'a Options,
    /// Qualified names of selected entity types (navigation targets must be in here).
    selected_types: BTreeSet<String>,
    schemas: Map<String, Value>,
    queued: BTreeSet<String>,
    queue: VecDeque<String>,
}

impl<'a> Gen<'a> {
    fn new(md: &'a Metadata, opts: &'a Options, sets: &[(&EntitySet, &StructuredType)]) -> Self {
        Self {
            md,
            v: opts.version,
            opts,
            selected_types: sets.iter().map(|(_, t)| t.qualified_name()).collect(),
            schemas: Map::new(),
            queued: BTreeSet::new(),
            queue: VecDeque::new(),
        }
    }

    fn run(mut self, sets: &[(&EntitySet, &StructuredType)]) -> Result<Value, Error> {
        let mut paths = Map::new();
        for (set, ty) in sets {
            self.entity_set_paths(set, ty, &mut paths);
        }
        // Emit every schema referenced so far, plus whatever those reference.
        while let Some(q) = self.queue.pop_front() {
            self.emit_schema(&q);
        }
        self.schemas
            .insert("ODataError".into(), odata_error_schema());
        self.schemas.sort_keys();

        let tags: Vec<Value> = sets
            .iter()
            .map(|(s, t)| json!({ "name": s.name, "description": format!("Entity set {} ({})", s.name, t.name) }))
            .collect();
        let info = json!({
            "title": self.opts.title,
            "version": "1.0",
            "description": format!("Generated from Dynamics 365 {} OData $metadata.", self.opts.platform.label()),
        });

        let doc = match self.v {
            SpecVersion::Swagger2 => {
                let mut doc = json!({ "swagger": "2.0", "info": info });
                if let Some(url) = &self.opts.server_url {
                    let (scheme, host) = url.split_once("://").unwrap_or(("https", url));
                    doc["host"] = host.trim_end_matches('/').into();
                    doc["schemes"] = json!([scheme]);
                }
                doc["basePath"] = "/".into();
                doc["consumes"] = json!(["application/json"]);
                doc["produces"] = json!(["application/json"]);
                doc["tags"] = tags.into();
                doc["paths"] = paths.into();
                doc["definitions"] = self.schemas.into();
                doc["securityDefinitions"] = json!({
                    "bearer": {
                        "type": "apiKey", "in": "header", "name": "Authorization",
                        "description": "Azure AD access token, as `Bearer <token>`."
                    }
                });
                doc["security"] = json!([{ "bearer": [] }]);
                doc
            }
            SpecVersion::OpenApi30 | SpecVersion::OpenApi31 => {
                let openapi = if self.v == SpecVersion::OpenApi30 {
                    "3.0.3"
                } else {
                    "3.1.0"
                };
                let server = match &self.opts.server_url {
                    Some(url) => json!({ "url": url.trim_end_matches('/') }),
                    None => json!({
                        "url": "https://{environment}",
                        "variables": { "environment": { "default": self.opts.platform.example_host() } }
                    }),
                };
                json!({
                    "openapi": openapi,
                    "info": info,
                    "servers": [server],
                    "tags": tags,
                    "paths": paths,
                    "components": {
                        "schemas": self.schemas,
                        "securitySchemes": {
                            "bearer": {
                                "type": "http", "scheme": "bearer", "bearerFormat": "JWT",
                                "description": "Azure AD access token for the environment."
                            }
                        }
                    },
                    "security": [{ "bearer": [] }],
                })
            }
        };
        Ok(doc)
    }

    // ---------- paths ----------

    fn entity_set_paths(
        &mut self,
        set: &EntitySet,
        ty: &StructuredType,
        paths: &mut Map<String, Value>,
    ) {
        let type_ref = self.schema_ref(&ty.qualified_name());
        let collection_name = format!("{}Collection", schema_name(ty));
        let collection = self.collection_schema(&ty.qualified_name());
        self.schemas.insert(collection_name.clone(), collection);
        let tag = json!([set.name]);
        // Only Finance & Operations has legal entities.
        let cross_company = (self.opts.platform == Platform::FinanceOperations).then(|| {
            self.param(
                "cross-company",
                "query",
                json!({ "type": "boolean" }),
                false,
                "Return data from all legal entities instead of only the default company.",
            )
        });

        // Collection: list + create.
        let mut list_params = vec![
            self.param(
                "$filter",
                "query",
                json!({"type":"string"}),
                false,
                "OData filter expression.",
            ),
            self.param(
                "$select",
                "query",
                json!({"type":"string"}),
                false,
                "Comma-separated properties to return.",
            ),
            self.param(
                "$expand",
                "query",
                json!({"type":"string"}),
                false,
                "Navigation properties to expand.",
            ),
            self.param(
                "$orderby",
                "query",
                json!({"type":"string"}),
                false,
                "Sort order, e.g. `Name desc`.",
            ),
            self.param(
                "$top",
                "query",
                json!({"type":"integer","minimum":0}),
                false,
                "Maximum number of records.",
            ),
            self.param(
                "$skip",
                "query",
                json!({"type":"integer","minimum":0}),
                false,
                "Number of records to skip.",
            ),
            self.param(
                "$count",
                "query",
                json!({"type":"boolean"}),
                false,
                "Include the total count.",
            ),
        ];
        list_params.extend(cross_company.clone());

        let coll_path = format!("{}/{}", self.opts.platform.data_path(), set.name);
        let mut coll = Map::new();
        coll.insert(
            "get".into(),
            self.operation(
                &tag,
                format!("{}_List", set.name),
                format!("List {}", set.name),
                list_params,
                None,
                ("200", Some(self.schema_ref_by_name(&collection_name))),
            ),
        );
        coll.insert(
            "post".into(),
            self.operation(
                &tag,
                format!("{}_Create", set.name),
                format!("Create a record in {}", set.name),
                vec![],
                Some(type_ref.clone()),
                ("201", Some(type_ref.clone())),
            ),
        );
        paths.insert(coll_path.clone(), coll.into());

        // Single entity by key.
        let key = self.md.key_of(ty);
        let props = self.md.all_properties(ty);
        let mut key_params = Vec::new();
        let mut key_parts = Vec::new();
        for k in key {
            let prop = props.iter().find(|p| &p.name == k);
            let type_name = prop.map(|p| p.type_name.as_str()).unwrap_or("Edm.String");
            key_parts.push(format!("{k}={}", self.key_literal(k, type_name)));
            let mut schema = self.primitive_or_ref(type_name);
            if let Some(p) = prop.filter(|p| p.max_length.is_some() && schema.get("$ref").is_none())
            {
                schema["maxLength"] = p.max_length.into();
            }
            key_params.push(self.param(k, "path", schema, true, ""));
        }
        let entity_path = if key.is_empty() {
            None
        } else if key.len() == 1 {
            // Single keys may be written without the name: Set('X').
            let k = &key[0];
            let type_name = props
                .iter()
                .find(|p| &p.name == k)
                .map(|p| p.type_name.as_str())
                .unwrap_or("Edm.String");
            Some(format!("{coll_path}({})", self.key_literal(k, type_name)))
        } else {
            Some(format!("{coll_path}({})", key_parts.join(",")))
        };

        if let Some(entity_path) = &entity_path {
            let mut get_params = key_params.clone();
            get_params.push(self.param(
                "$select",
                "query",
                json!({"type":"string"}),
                false,
                "Comma-separated properties to return.",
            ));
            get_params.push(self.param(
                "$expand",
                "query",
                json!({"type":"string"}),
                false,
                "Navigation properties to expand.",
            ));
            get_params.extend(cross_company.clone());
            let mut item = Map::new();
            item.insert(
                "get".into(),
                self.operation(
                    &tag,
                    format!("{}_Get", set.name),
                    format!("Get a record from {}", set.name),
                    get_params,
                    None,
                    ("200", Some(type_ref.clone())),
                ),
            );
            let mut write_params = key_params.clone();
            write_params.extend(cross_company.clone());
            item.insert(
                "patch".into(),
                self.operation(
                    &tag,
                    format!("{}_Update", set.name),
                    format!("Update a record in {}", set.name),
                    write_params.clone(),
                    Some(type_ref.clone()),
                    ("204", None),
                ),
            );
            item.insert(
                "delete".into(),
                self.operation(
                    &tag,
                    format!("{}_Delete", set.name),
                    format!("Delete a record from {}", set.name),
                    write_params,
                    None,
                    ("204", None),
                ),
            );
            paths.insert(entity_path.clone(), item.into());
        }

        if self.opts.include_operations {
            self.bound_operation_paths(
                set,
                ty,
                &coll_path,
                entity_path.as_deref(),
                &key_params,
                paths,
            );
        }
    }

    fn bound_operation_paths(
        &mut self,
        set: &EntitySet,
        ty: &StructuredType,
        coll_path: &str,
        entity_path: Option<&str>,
        key_params: &[Value],
        paths: &mut Map<String, Value>,
    ) {
        let md = self.md;
        let qn = ty.qualified_name();
        for op in md.operations.iter().filter(|o| o.is_bound) {
            let Some(binding) = op.parameters.first() else {
                continue;
            };
            let (bound_to, on_collection) = element_type(&binding.type_name);
            if md.normalize(bound_to) != qn {
                continue;
            }
            let (base_path, mut params) = if on_collection {
                (coll_path.to_string(), Vec::new())
            } else {
                match entity_path {
                    Some(p) => (p.to_string(), key_params.to_vec()),
                    None => continue,
                }
            };
            let rest = &op.parameters[1..];
            let op_id = format!("{}_{}", set.name, op.name);
            // Entity and complex results come back as-is; primitives and collections are wrapped in `value`.
            let (status, response) = match op.return_type.as_deref() {
                Some(r) => {
                    let schema = self.type_schema(r, false);
                    let wrapped = element_type(r).1
                        || matches!(
                            md.resolve(r),
                            TypeRef::Primitive | TypeRef::Enum(_) | TypeRef::Unknown
                        );
                    let schema = if wrapped {
                        json!({ "type": "object", "properties": { "value": schema } })
                    } else {
                        schema
                    };
                    ("200", Some(schema))
                }
                None => ("204", None),
            };
            let summary = format!(
                "{} {} on {}",
                if op.is_function {
                    "Call function"
                } else {
                    "Invoke action"
                },
                op.name,
                set.name
            );
            let (path, method, body) = if op.is_function {
                let args: Vec<String> = rest.iter().map(|p| format!("{0}=@{0}", p.name)).collect();
                for p in rest {
                    let schema = self.primitive_or_ref(&p.type_name);
                    params.push(self.param(
                        &format!("@{}", p.name),
                        "query",
                        schema,
                        !p.nullable,
                        "Function parameter as an OData literal.",
                    ));
                }
                (
                    format!("{base_path}/{}({})", op.qualified_name(), args.join(",")),
                    "get",
                    None,
                )
            } else {
                let mut props = Map::new();
                for p in rest {
                    props.insert(p.name.clone(), self.type_schema(&p.type_name, p.nullable));
                }
                let body =
                    (!rest.is_empty()).then(|| json!({ "type": "object", "properties": props }));
                (format!("{base_path}/{}", op.qualified_name()), "post", body)
            };
            let operation = self.operation(
                &json!([set.name]),
                op_id,
                summary,
                params,
                body,
                (status, response),
            );
            let entry = paths.entry(path).or_insert_with(|| json!({}));
            entry[method] = operation;
        }
    }

    /// One operation object. `body` is a request body schema, `response` is (status, schema).
    fn operation(
        &self,
        tags: &Value,
        operation_id: String,
        summary: String,
        params: Vec<Value>,
        body: Option<Value>,
        response: (&str, Option<Value>),
    ) -> Value {
        let mut op = json!({ "tags": tags, "summary": summary, "operationId": operation_id });
        let mut params = params;
        let error_ref = self.schema_ref_by_name("ODataError");
        let mut responses = Map::new();
        match self.v {
            SpecVersion::Swagger2 => {
                if let Some(schema) = body {
                    params.push(
                        json!({ "name": "body", "in": "body", "required": true, "schema": schema }),
                    );
                }
                let mut ok = json!({ "description": "Success" });
                if let Some(schema) = response.1 {
                    ok["schema"] = schema;
                }
                responses.insert(response.0.into(), ok);
                responses.insert(
                    "default".into(),
                    json!({ "description": "Error", "schema": error_ref }),
                );
            }
            _ => {
                if let Some(schema) = body {
                    op["requestBody"] = json!({ "required": true, "content": { "application/json": { "schema": schema } } });
                }
                let mut ok = json!({ "description": "Success" });
                if let Some(schema) = response.1 {
                    ok["content"] = json!({ "application/json": { "schema": schema } });
                }
                responses.insert(response.0.into(), ok);
                responses.insert(
                    "default".into(),
                    json!({
                        "description": "Error",
                        "content": { "application/json": { "schema": error_ref } }
                    }),
                );
            }
        }
        if !params.is_empty() {
            op["parameters"] = params.into();
        }
        op["responses"] = responses.into();
        op
    }

    /// A parameter object. In Swagger 2.0 the schema keywords are inlined;
    /// references are not allowed there, so they collapse to `string`.
    fn param(
        &self,
        name: &str,
        location: &str,
        schema: Value,
        required: bool,
        description: &str,
    ) -> Value {
        let mut p =
            json!({ "name": name, "in": location, "required": required || location == "path" });
        if !description.is_empty() {
            p["description"] = description.into();
        }
        match self.v {
            SpecVersion::Swagger2 => {
                let schema = if schema.get("$ref").is_some() {
                    json!({ "type": "string" })
                } else {
                    schema
                };
                if let Value::Object(m) = schema {
                    for (k, v) in m {
                        p[k] = v;
                    }
                }
            }
            _ => p["schema"] = schema,
        }
        p
    }

    /// How a key value is written inside `Set(...)`.
    fn key_literal(&self, name: &str, type_name: &str) -> String {
        match self.md.resolve(type_name) {
            TypeRef::Enum(e) => format!("{}'{{{name}}}'", e.qualified_name()),
            _ => match type_name {
                "Edm.String" => format!("'{{{name}}}'"),
                _ => format!("{{{name}}}"),
            },
        }
    }

    // ---------- schemas ----------

    fn collection_schema(&mut self, entity: &str) -> Value {
        json!({
            "type": "object",
            "properties": {
                "@odata.context": { "type": "string" },
                "@odata.count": { "type": "integer", "format": "int64" },
                "@odata.nextLink": { "type": "string" },
                "value": { "type": "array", "items": self.schema_ref(entity) },
            }
        })
    }

    fn emit_schema(&mut self, qualified: &str) {
        let md = self.md;
        let schema = match md.resolve(qualified) {
            TypeRef::Enum(e) => {
                let names: Vec<Value> = e.members.iter().map(|m| m.name.clone().into()).collect();
                let mut s = json!({ "type": "string", "enum": names, "title": e.name });
                if e.is_flags {
                    // Flags are sent as a comma-separated list of member names.
                    s = json!({ "type": "string", "title": e.name,
                        "description": format!("Comma-separated flags: {}", e.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")) });
                }
                (e.name.clone(), s)
            }
            TypeRef::Entity(t) | TypeRef::Complex(t) => {
                let is_entity = matches!(md.resolve(qualified), TypeRef::Entity(_));
                let mut props = Map::new();
                if is_entity {
                    props.insert(
                        "@odata.etag".into(),
                        json!({ "type": "string", "readOnly": true }),
                    );
                }
                for p in md.all_properties(t) {
                    let mut s = self.type_schema(&p.type_name, p.nullable);
                    if let (Some(n), Some(obj)) = (p.max_length, s.as_object_mut())
                        && obj.get("type").is_some_and(|t| {
                            t == "string"
                                || t.as_array().is_some_and(|a| a.contains(&"string".into()))
                        })
                    {
                        obj.insert("maxLength".into(), n.into());
                    }
                    props.insert(p.name.clone(), s);
                }
                if self.opts.include_navigation && is_entity {
                    for nav in md.all_navigation_properties(t) {
                        let (target, many) = element_type(&nav.type_name);
                        if !self.selected_types.contains(md.normalize(target).as_ref()) {
                            continue;
                        }
                        let s = self.type_schema(&nav.type_name, nav.nullable && !many);
                        props.insert(nav.name.clone(), s);
                    }
                }
                let mut s = json!({ "type": "object", "title": t.name, "properties": props });
                let key = md.key_of(t);
                if is_entity && !key.is_empty() {
                    s["description"] = format!("Key: {}", key.join(", ")).into();
                }
                (schema_name(t), s)
            }
            TypeRef::Primitive | TypeRef::Unknown => return,
        };
        self.schemas.insert(schema.0, schema.1);
    }

    /// Schema for a property/parameter type, applying nullability for the target version.
    fn type_schema(&mut self, type_name: &str, nullable: bool) -> Value {
        let (inner, many) = element_type(type_name);
        if many {
            return json!({ "type": "array", "items": self.type_schema(inner, false) });
        }
        let base = self.primitive_or_ref(inner);
        if !nullable {
            return base;
        }
        let is_ref = base.get("$ref").is_some();
        match (self.v, is_ref) {
            (SpecVersion::Swagger2, _) => {
                let mut s = base;
                s["x-nullable"] = true.into();
                s
            }
            (SpecVersion::OpenApi30, true) => json!({ "allOf": [base], "nullable": true }),
            (SpecVersion::OpenApi30, false) => {
                let mut s = base;
                s["nullable"] = true.into();
                s
            }
            (SpecVersion::OpenApi31, true) => json!({ "anyOf": [base, { "type": "null" }] }),
            (SpecVersion::OpenApi31, false) => {
                let mut s = base;
                if let Some(t) = s.get("type").cloned() {
                    s["type"] = json!([t, "null"]);
                }
                s
            }
        }
    }

    /// A primitive schema, or a `$ref` to a named type (queuing it for emission).
    fn primitive_or_ref(&mut self, type_name: &str) -> Value {
        if let Some(p) = primitive(type_name) {
            return p;
        }
        match self.md.resolve(type_name) {
            TypeRef::Entity(_) | TypeRef::Complex(_) | TypeRef::Enum(_) => {
                self.schema_ref(type_name)
            }
            _ => {
                json!({ "type": "string", "description": format!("Unrecognised type {type_name}") })
            }
        }
    }

    fn schema_ref(&mut self, qualified: &str) -> Value {
        let q = self.md.normalize(qualified).into_owned();
        let name = match self.md.resolve(&q) {
            TypeRef::Entity(t) | TypeRef::Complex(t) => schema_name(t),
            TypeRef::Enum(e) => e.name.clone(),
            _ => q.clone(),
        };
        if self.queued.insert(q.clone()) {
            self.queue.push_back(q);
        }
        self.schema_ref_by_name(&name)
    }

    fn schema_ref_by_name(&self, name: &str) -> Value {
        let prefix = match self.v {
            SpecVersion::Swagger2 => "#/definitions/",
            _ => "#/components/schemas/",
        };
        json!({ "$ref": format!("{prefix}{name}") })
    }
}

fn schema_name(t: &StructuredType) -> String {
    t.name.clone()
}

fn primitive(type_name: &str) -> Option<Value> {
    Some(match type_name {
        "Edm.String" => json!({ "type": "string" }),
        "Edm.Boolean" => json!({ "type": "boolean" }),
        "Edm.Byte" => json!({ "type": "integer", "format": "uint8" }),
        "Edm.SByte" => json!({ "type": "integer", "format": "int8" }),
        "Edm.Int16" => json!({ "type": "integer", "format": "int16" }),
        "Edm.Int32" => json!({ "type": "integer", "format": "int32" }),
        "Edm.Int64" => json!({ "type": "integer", "format": "int64" }),
        "Edm.Decimal" => json!({ "type": "number", "format": "decimal" }),
        "Edm.Double" => json!({ "type": "number", "format": "double" }),
        "Edm.Single" => json!({ "type": "number", "format": "float" }),
        "Edm.Guid" => json!({ "type": "string", "format": "uuid" }),
        "Edm.Date" => json!({ "type": "string", "format": "date" }),
        "Edm.DateTimeOffset" => json!({ "type": "string", "format": "date-time" }),
        "Edm.TimeOfDay" => json!({ "type": "string", "format": "time" }),
        "Edm.Duration" => json!({ "type": "string", "format": "duration" }),
        "Edm.Binary" | "Edm.Stream" => json!({ "type": "string", "format": "byte" }),
        t if t.starts_with("Edm.") => json!({ "type": "string" }),
        _ => return None,
    })
}

fn odata_error_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "error": {
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "message": { "type": "string" },
                    "innererror": { "type": "object" }
                }
            }
        }
    })
}
