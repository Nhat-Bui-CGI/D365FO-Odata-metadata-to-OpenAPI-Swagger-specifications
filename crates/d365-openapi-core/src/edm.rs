//! In-memory model of the parts of an OData CSDL (EDMX v4) document that matter
//! for generating OpenAPI.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    /// Type as written in the metadata, e.g. `Edm.String` or `Collection(Ns.Enum)`.
    pub type_name: String,
    pub nullable: bool,
    pub max_length: Option<u64>,
    pub precision: Option<u32>,
    pub scale: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavigationProperty {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub partner: Option<String>,
}

/// An `EntityType` or a `ComplexType`. Complex types have no key.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StructuredType {
    pub namespace: String,
    pub name: String,
    pub base_type: Option<String>,
    pub is_abstract: bool,
    pub key: Vec<String>,
    pub properties: Vec<Property>,
    pub navigation_properties: Vec<NavigationProperty>,
}

impl StructuredType {
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumMember {
    pub name: String,
    pub value: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnumType {
    pub namespace: String,
    pub name: String,
    pub is_flags: bool,
    pub members: Vec<EnumMember>,
}

impl EnumType {
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// An `Action` or a `Function`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Operation {
    pub namespace: String,
    pub name: String,
    pub is_function: bool,
    pub is_bound: bool,
    /// For bound operations the first parameter is the binding parameter.
    pub parameters: Vec<Parameter>,
    pub return_type: Option<String>,
}

impl Operation {
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntitySet {
    pub name: String,
    /// Qualified entity type name.
    pub entity_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub entity_types: Vec<StructuredType>,
    pub complex_types: Vec<StructuredType>,
    pub enum_types: Vec<EnumType>,
    pub operations: Vec<Operation>,
    pub entity_sets: Vec<EntitySet>,
    /// Schema alias -> namespace.
    pub aliases: HashMap<String, String>,
}

/// What a qualified type name refers to.
#[derive(Debug, Clone, Copy)]
pub enum TypeRef<'a> {
    Entity(&'a StructuredType),
    Complex(&'a StructuredType),
    Enum(&'a EnumType),
    Primitive,
    Unknown,
}

/// Splits `Collection(X)` into `(X, true)`, anything else into `(X, false)`.
pub fn element_type(type_name: &str) -> (&str, bool) {
    match type_name
        .strip_prefix("Collection(")
        .and_then(|t| t.strip_suffix(')'))
    {
        Some(inner) => (inner, true),
        None => (type_name, false),
    }
}

impl Metadata {
    /// Replaces a leading schema alias with its namespace.
    pub fn normalize<'a>(&self, qualified: &'a str) -> std::borrow::Cow<'a, str> {
        if let Some((prefix, rest)) = qualified.rsplit_once('.')
            && let Some(ns) = self.aliases.get(prefix)
        {
            return format!("{ns}.{rest}").into();
        }
        qualified.into()
    }

    pub fn resolve(&self, type_name: &str) -> TypeRef<'_> {
        let (inner, _) = element_type(type_name);
        if inner.starts_with("Edm.") {
            return TypeRef::Primitive;
        }
        let q = self.normalize(inner);
        if let Some(t) = self.entity_types.iter().find(|t| t.qualified_name() == q) {
            return TypeRef::Entity(t);
        }
        if let Some(t) = self.complex_types.iter().find(|t| t.qualified_name() == q) {
            return TypeRef::Complex(t);
        }
        if let Some(t) = self.enum_types.iter().find(|t| t.qualified_name() == q) {
            return TypeRef::Enum(t);
        }
        TypeRef::Unknown
    }

    pub fn entity_type(&self, qualified: &str) -> Option<&StructuredType> {
        match self.resolve(qualified) {
            TypeRef::Entity(t) => Some(t),
            _ => None,
        }
    }

    pub fn entity_set(&self, name: &str) -> Option<&EntitySet> {
        self.entity_sets.iter().find(|s| s.name == name)
    }

    /// Case-insensitive substring search over entity set and entity type names.
    /// An empty query returns every entity set.
    pub fn search_entity_sets(&self, query: &str) -> Vec<&EntitySet> {
        let q = query.trim().to_lowercase();
        self.entity_sets
            .iter()
            .filter(|s| {
                q.is_empty()
                    || s.name.to_lowercase().contains(&q)
                    || s.entity_type.to_lowercase().contains(&q)
            })
            .collect()
    }

    /// Properties of a structured type including those inherited from base types,
    /// base properties first.
    pub fn all_properties<'a>(&'a self, t: &'a StructuredType) -> Vec<&'a Property> {
        let mut chain = vec![t];
        let mut cur = t;
        while let Some(base) = cur.base_type.as_deref() {
            let next = match self.resolve(base) {
                TypeRef::Entity(b) | TypeRef::Complex(b) => b,
                _ => break,
            };
            // Guard against cycles in malformed metadata.
            if chain.iter().any(|c| std::ptr::eq(*c, next)) {
                break;
            }
            chain.push(next);
            cur = next;
        }
        chain
            .iter()
            .rev()
            .flat_map(|c| c.properties.iter())
            .collect()
    }

    pub fn all_navigation_properties<'a>(
        &'a self,
        t: &'a StructuredType,
    ) -> Vec<&'a NavigationProperty> {
        let mut out: Vec<&NavigationProperty> = t.navigation_properties.iter().collect();
        let mut cur = t;
        let mut depth = 0;
        while let (Some(base), true) = (cur.base_type.as_deref(), depth < 32) {
            match self.resolve(base) {
                TypeRef::Entity(b) => {
                    out.splice(0..0, b.navigation_properties.iter());
                    cur = b;
                }
                _ => break,
            }
            depth += 1;
        }
        out
    }

    /// Key property names, taken from the nearest type in the base chain that declares one.
    pub fn key_of<'a>(&'a self, t: &'a StructuredType) -> &'a [String] {
        let mut cur = t;
        for _ in 0..32 {
            if !cur.key.is_empty() {
                return &cur.key;
            }
            match cur.base_type.as_deref().map(|b| self.resolve(b)) {
                Some(TypeRef::Entity(b)) => cur = b,
                _ => break,
            }
        }
        &t.key
    }
}
