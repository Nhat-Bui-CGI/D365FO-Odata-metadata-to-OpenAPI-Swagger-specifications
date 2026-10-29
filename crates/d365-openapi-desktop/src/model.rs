//! UI-independent state: what the user selected and how output files are named.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use d365_openapi_core::{Metadata, OutputFormat, SpecVersion};
use serde::{Deserialize, Serialize};

/// Output choices, saved between runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputOptions {
    pub swagger2: bool,
    pub openapi30: bool,
    pub openapi31: bool,
    pub json: bool,
    pub yaml: bool,
    pub include_operations: bool,
    pub include_navigation: bool,
    /// Put the environment URL in `servers` / `host`.
    pub include_server_url: bool,
    pub output_dir: Option<PathBuf>,
    /// Write one file with all selected entities, named `file_name`.
    pub combined: bool,
    pub file_name: String,
    /// Also write one file per selected entity, named after its entity set.
    pub per_entity: bool,
}

/// One file an export writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub version: SpecVersion,
    pub format: OutputFormat,
    pub path: PathBuf,
    /// The one entity set in a per-entity file; `None` for the combined file.
    pub entity_set: Option<String>,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            swagger2: false,
            openapi30: true,
            openapi31: false,
            json: true,
            yaml: false,
            include_operations: true,
            include_navigation: true,
            include_server_url: true,
            output_dir: None,
            combined: true,
            file_name: "d365fo-api".into(),
            per_entity: false,
        }
    }
}

impl OutputOptions {
    pub fn versions(&self) -> Vec<SpecVersion> {
        let mut v = Vec::new();
        if self.swagger2 {
            v.push(SpecVersion::Swagger2);
        }
        if self.openapi30 {
            v.push(SpecVersion::OpenApi30);
        }
        if self.openapi31 {
            v.push(SpecVersion::OpenApi31);
        }
        v
    }

    pub fn formats(&self) -> Vec<OutputFormat> {
        let mut f = Vec::new();
        if self.json {
            f.push(OutputFormat::Json);
        }
        if self.yaml {
            f.push(OutputFormat::Yaml);
        }
        f
    }

    /// Every file an export writes: `<dir>/<name>.<version>.<ext>`, where the
    /// name is `file_name` for the combined file and the entity set name for
    /// per-entity files. Files of one document are next to each other.
    pub fn planned_files(&self, dir: &Path, entity_sets: &[String]) -> Vec<PlannedFile> {
        let mut docs: Vec<(String, Option<&String>)> = Vec::new();
        if self.combined {
            docs.push((sanitize_file_name(&self.file_name), None));
        }
        if self.per_entity {
            docs.extend(entity_sets.iter().map(|s| (sanitize_file_name(s), Some(s))));
        }
        let mut out = Vec::new();
        for version in self.versions() {
            for (name, set) in &docs {
                for format in self.formats() {
                    out.push(PlannedFile {
                        version,
                        format,
                        path: dir.join(format!("{name}.{}.{}", version.slug(), format.extension())),
                        entity_set: set.cloned(),
                    });
                }
            }
        }
        out
    }

    /// Why nothing can be exported with these choices, if so.
    pub fn problem(&self) -> Option<&'static str> {
        if self.versions().is_empty() || self.formats().is_empty() {
            Some("Pick at least one format and JSON or YAML.")
        } else if !self.combined && !self.per_entity {
            Some("Pick a combined file, one file per entity, or both.")
        } else {
            None
        }
    }
}

/// File names for display: the first `max`, then a count of the rest.
pub fn file_list_summary(files: &[PlannedFile], max: usize) -> Vec<String> {
    let mut names: Vec<String> = files
        .iter()
        .take(max)
        .map(|f| {
            f.path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    if files.len() > max {
        names.push(format!("… and {} more", files.len() - max));
    }
    names
}

pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_. ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "d365fo-api".into()
    } else {
        cleaned
    }
}

/// App state saved between runs (separate from the fetch settings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// Selected entity sets per environment URL.
    pub selections: BTreeMap<String, BTreeSet<String>>,
    pub output: OutputOptions,
    pub last_environment: Option<String>,
}

impl AppState {
    pub fn default_path() -> PathBuf {
        d365_fetch::Settings::default_path().with_file_name("app_state.json")
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)
    }
}

/// One row of the entity list.
#[derive(Debug, Clone)]
pub struct EntityRow {
    pub set_name: String,
    /// Unqualified entity type name.
    pub type_name: String,
}

pub fn entity_rows(md: &Metadata) -> Vec<EntityRow> {
    let mut rows: Vec<EntityRow> = md
        .entity_sets
        .iter()
        .map(|s| EntityRow {
            set_name: s.name.clone(),
            type_name: s
                .entity_type
                .rsplit('.')
                .next()
                .unwrap_or(&s.entity_type)
                .to_string(),
        })
        .collect();
    rows.sort_by_key(|r| r.set_name.to_lowercase());
    rows
}

/// Indices of rows matching the search. Space-separated words must all match
/// (case-insensitive) the entity set or type name.
pub fn filter_rows(
    rows: &[EntityRow],
    query: &str,
    selected: &BTreeSet<String>,
    selected_only: bool,
) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| !selected_only || selected.contains(&r.set_name))
        .filter(|(_, r)| {
            let hay = format!(
                "{} {}",
                r.set_name.to_lowercase(),
                r.type_name.to_lowercase()
            );
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn format_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / (1 << 10) as f64),
        n => format!("{n} B"),
    }
}

/// Unix seconds -> `2026-10-28 14:05 UTC`.
pub fn format_time(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<EntityRow> {
        [
            ("CustomersV3", "CustomerV3"),
            ("SalesOrderHeadersV2", "SalesOrderHeaderV2"),
            ("SalesOrderLines", "SalesOrderLine"),
        ]
        .iter()
        .map(|(s, t)| EntityRow {
            set_name: s.to_string(),
            type_name: t.to_string(),
        })
        .collect()
    }

    #[test]
    fn filters_by_all_words() {
        let none = BTreeSet::new();
        assert_eq!(filter_rows(&rows(), "", &none, false), [0, 1, 2]);
        assert_eq!(filter_rows(&rows(), "sales", &none, false), [1, 2]);
        assert_eq!(filter_rows(&rows(), "sales line", &none, false), [2]);
        assert_eq!(filter_rows(&rows(), "CUSTOMERV3", &none, false), [0]);
        let sel: BTreeSet<String> = ["SalesOrderLines".to_string()].into();
        assert_eq!(filter_rows(&rows(), "", &sel, true), [2]);
    }

    #[test]
    fn plans_one_file_per_version_and_format() {
        let opts = OutputOptions {
            swagger2: true,
            openapi31: true,
            yaml: true,
            file_name: "my api/v1".into(),
            ..Default::default()
        };
        let names = file_list_summary(&opts.planned_files(Path::new("/out"), &[]), 99);
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"my api_v1.swagger-2.0.json".to_string()));
        assert!(names.contains(&"my api_v1.openapi-3.1.yaml".to_string()));
    }

    #[test]
    fn plans_one_file_per_entity() {
        let sets = ["CustomersV3".to_string(), "SalesOrderLines".to_string()];
        let opts = OutputOptions {
            per_entity: true,
            yaml: true,
            ..Default::default()
        };
        let files = opts.planned_files(Path::new("/out"), &sets);
        let names = file_list_summary(&files, 99);
        assert_eq!(
            names,
            [
                "d365fo-api.openapi-3.0.json",
                "d365fo-api.openapi-3.0.yaml",
                "CustomersV3.openapi-3.0.json",
                "CustomersV3.openapi-3.0.yaml",
                "SalesOrderLines.openapi-3.0.json",
                "SalesOrderLines.openapi-3.0.yaml",
            ]
        );
        assert_eq!(files[2].entity_set.as_deref(), Some("CustomersV3"));
        assert_eq!(files[0].entity_set, None);

        let only = OutputOptions {
            combined: false,
            ..opts
        };
        assert_eq!(only.planned_files(Path::new("/out"), &sets).len(), 4);
        assert_eq!(file_list_summary(&files, 2).last().unwrap(), "… and 4 more");
        let none = OutputOptions {
            per_entity: false,
            ..only
        };
        assert!(none.problem().is_some());
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(40 << 20), "40.0 MB");
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_time(1_793_196_300), "2026-10-28 14:05 UTC");
    }

    #[test]
    fn state_round_trips() {
        let path = std::env::temp_dir().join(format!("d365-app-state-{}.json", std::process::id()));
        let mut s = AppState::default();
        s.selections
            .entry("https://x".into())
            .or_default()
            .insert("CustomersV3".into());
        s.save(&path).unwrap();
        assert_eq!(AppState::load(&path), s);
        fs::remove_file(path).unwrap();
    }
}
