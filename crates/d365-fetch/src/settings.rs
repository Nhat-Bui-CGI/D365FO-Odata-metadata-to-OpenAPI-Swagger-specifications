//! The app's saved settings: environments and the metadata cache location.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::auth::{AuthConfig, DEFAULT_AUTHORITY};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// Display name, e.g. "UAT".
    pub name: String,
    /// Normalised URL, e.g. `https://contoso-uat.sandbox.operations.dynamics.com`.
    pub url: String,
    /// Tenant ID or domain. Empty means `organizations` (the account's own tenant).
    #[serde(default)]
    pub tenant: String,
    /// Client ID of the app registration used for sign-in. Empty means the
    /// Azure CLI's public client ID, which needs no app registration if the
    /// tenant allows it.
    #[serde(default)]
    pub client_id: String,
}

impl Environment {
    pub fn auth_config(&self) -> AuthConfig {
        AuthConfig {
            authority: DEFAULT_AUTHORITY.into(),
            tenant: or_default(&self.tenant, DEFAULT_TENANT),
            client_id: or_default(&self.client_id, AZURE_CLI_CLIENT_ID),
            environment_url: self.url.clone(),
        }
    }
}

/// Microsoft's public client ID for the Azure CLI. It exists in every tenant and
/// allows `http://localhost` redirects, so it can be used without an app registration.
pub const AZURE_CLI_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";
/// Lets the signed-in work account pick its own tenant.
pub const DEFAULT_TENANT: &str = "organizations";

fn or_default(value: &str, default: &str) -> String {
    let v = value.trim();
    if v.is_empty() {
        default.into()
    } else {
        v.into()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where cached metadata is stored. `None` means the default data folder.
    pub cache_dir: Option<PathBuf>,
    pub environments: Vec<Environment>,
}

impl Settings {
    fn project_dirs() -> Option<directories::ProjectDirs> {
        directories::ProjectDirs::from("", "", "D365 OpenAPI")
    }

    /// e.g. `%APPDATA%\D365 OpenAPI\config\settings.json` on Windows.
    pub fn default_path() -> PathBuf {
        Self::project_dirs()
            .map(|d| d.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("settings.json"))
    }

    /// e.g. `%APPDATA%\D365 OpenAPI\data\metadata` on Windows.
    pub fn default_cache_dir() -> PathBuf {
        Self::project_dirs()
            .map(|d| d.data_dir().join("metadata"))
            .unwrap_or_else(|| PathBuf::from("metadata"))
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.cache_dir
            .clone()
            .unwrap_or_else(Self::default_cache_dir)
    }

    /// Loads settings, or defaults if the file does not exist yet.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ids_use_defaults() {
        let env = Environment {
            name: "x".into(),
            url: "https://x".into(),
            tenant: " ".into(),
            client_id: String::new(),
        };
        let cfg = env.auth_config();
        assert_eq!(cfg.tenant, DEFAULT_TENANT);
        assert_eq!(cfg.client_id, AZURE_CLI_CLIENT_ID);
    }

    #[test]
    fn round_trips() {
        let path = std::env::temp_dir().join(format!(
            "d365-settings-{}/settings.json",
            std::process::id()
        ));
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let s = Settings {
            cache_dir: Some("/tmp/somewhere".into()),
            environments: vec![Environment {
                name: "UAT".into(),
                url: "https://contoso-uat.sandbox.operations.dynamics.com".into(),
                tenant: "contoso.onmicrosoft.com".into(),
                client_id: "00000000-0000-0000-0000-000000000000".into(),
            }],
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        assert_eq!(s.cache_dir(), PathBuf::from("/tmp/somewhere"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
