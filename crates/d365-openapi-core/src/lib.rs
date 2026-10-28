//! Convert Dynamics 365 Finance & Operations OData `$metadata` into
//! OpenAPI 3.0 / 3.1 or Swagger 2.0 specifications.
//!
//! ```no_run
//! use d365_openapi_core::{parse_str, generate, render, Options, OutputFormat, SpecVersion};
//! let xml = std::fs::read_to_string("metadata.xml").unwrap();
//! let md = parse_str(&xml).unwrap();
//! let opts = Options { version: SpecVersion::OpenApi31, ..Default::default() };
//! let doc = generate(&md, &["CustomersV3"], &opts).unwrap();
//! println!("{}", render(&doc, OutputFormat::Yaml).unwrap());
//! ```

pub mod edm;
mod generate;
mod parse;

pub use edm::{EntitySet, Metadata};
pub use generate::{Options, SpecVersion, generate};
pub use parse::{parse_reader, parse_str};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid XML: {0}")]
    Xml(String),
    #[error("the document is not OData $metadata (no edmx:Edmx element)")]
    NotMetadata,
    #[error("entity set `{0}` is not in the metadata")]
    UnknownEntitySet(String),
    #[error("type `{0}` is not in the metadata")]
    UnknownType(String),
    #[error("no entity sets selected")]
    NothingSelected,
    #[error("could not serialise the spec: {0}")]
    Serialize(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    Json,
    Yaml,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

pub fn render(doc: &serde_json::Value, format: OutputFormat) -> Result<String, Error> {
    match format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(doc).map_err(|e| Error::Serialize(e.to_string()))
        }
        OutputFormat::Yaml => {
            serde_yaml::to_string(doc).map_err(|e| Error::Serialize(e.to_string()))
        }
    }
}
