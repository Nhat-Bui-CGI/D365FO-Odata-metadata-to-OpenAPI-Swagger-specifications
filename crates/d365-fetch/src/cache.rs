//! Local metadata cache: one XML file plus a small JSON sidecar per environment.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub environment_url: String,
    /// Unix seconds when the current content was first downloaded.
    pub downloaded_at: u64,
    /// Unix seconds of the last update check, changed or not.
    pub checked_at: u64,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// Nothing was cached for this environment before.
    New,
    /// The metadata changed and the cache was replaced.
    Updated,
    /// Same content as before; only `checked_at` changed.
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the cached `$metadata` XML for an environment (may not exist).
    pub fn metadata_path(&self, environment_url: &str) -> PathBuf {
        self.dir.join(format!("{}.xml", file_stem(environment_url)))
    }

    fn entry_path(&self, environment_url: &str) -> PathBuf {
        self.dir
            .join(format!("{}.json", file_stem(environment_url)))
    }

    pub fn entry(&self, environment_url: &str) -> Option<CacheEntry> {
        let text = fs::read_to_string(self.entry_path(environment_url)).ok()?;
        let entry: CacheEntry = serde_json::from_str(&text).ok()?;
        self.metadata_path(environment_url)
            .is_file()
            .then_some(entry)
    }

    /// Saves downloaded metadata, replacing the cached file only if the content changed.
    pub fn store(&self, environment_url: &str, data: &[u8]) -> Result<StoreOutcome, Error> {
        fs::create_dir_all(&self.dir)?;
        let now = unix_now();
        let sha256 = hex(&Sha256::digest(data));
        let previous = self.entry(environment_url);
        let outcome = match &previous {
            None => StoreOutcome::New,
            Some(p) if p.sha256 == sha256 => StoreOutcome::Unchanged,
            Some(_) => StoreOutcome::Updated,
        };
        if outcome != StoreOutcome::Unchanged {
            write_atomic(&self.metadata_path(environment_url), data)?;
        }
        let entry = CacheEntry {
            environment_url: environment_url.to_string(),
            downloaded_at: match (&previous, outcome) {
                (Some(p), StoreOutcome::Unchanged) => p.downloaded_at,
                _ => now,
            },
            checked_at: now,
            sha256,
            size: data.len() as u64,
        };
        write_atomic(
            &self.entry_path(environment_url),
            serde_json::to_string_pretty(&entry)?.as_bytes(),
        )?;
        Ok(outcome)
    }

    pub fn remove(&self, environment_url: &str) -> Result<(), Error> {
        for p in [
            self.metadata_path(environment_url),
            self.entry_path(environment_url),
        ] {
            match fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Moves every cached environment to `new_dir` and returns the cache at its new location.
    pub fn move_to(&self, new_dir: impl Into<PathBuf>) -> Result<Cache, Error> {
        let new = Cache::new(new_dir);
        if new.dir == self.dir {
            return Ok(new);
        }
        fs::create_dir_all(&new.dir)?;
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Ok(new);
        };
        for entry in entries {
            let path = entry?.path();
            let is_cache_file = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("xml" | "json")
            );
            if !is_cache_file {
                continue;
            }
            let target = new.dir.join(path.file_name().unwrap());
            // rename fails across drives; fall back to copy + delete.
            if fs::rename(&path, &target).is_err() {
                fs::copy(&path, &target)?;
                fs::remove_file(&path)?;
            }
        }
        Ok(new)
    }
}

/// `https://contoso.operations.dynamics.com` -> `contoso.operations.dynamics.com`.
fn file_stem(environment_url: &str) -> String {
    let without_scheme = environment_url
        .split_once("://")
        .map_or(environment_url, |(_, rest)| rest);
    without_scheme
        .trim_end_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), Error> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("d365-fetch-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    const ENV: &str = "https://contoso.operations.dynamics.com";

    #[test]
    fn store_detects_changes() {
        let cache = Cache::new(temp_dir("store"));
        assert!(cache.entry(ENV).is_none());
        assert_eq!(cache.store(ENV, b"<a/>").unwrap(), StoreOutcome::New);
        let first = cache.entry(ENV).unwrap();
        assert_eq!(first.size, 4);
        assert_eq!(cache.store(ENV, b"<a/>").unwrap(), StoreOutcome::Unchanged);
        assert_eq!(cache.entry(ENV).unwrap().downloaded_at, first.downloaded_at);
        assert_eq!(cache.store(ENV, b"<b/>").unwrap(), StoreOutcome::Updated);
        assert_eq!(fs::read(cache.metadata_path(ENV)).unwrap(), b"<b/>");
        assert!(
            cache
                .metadata_path(ENV)
                .ends_with("contoso.operations.dynamics.com.xml")
        );
        cache.remove(ENV).unwrap();
        assert!(cache.entry(ENV).is_none());
        fs::remove_dir_all(cache.dir()).unwrap();
    }

    #[test]
    fn move_to_new_location() {
        let old = Cache::new(temp_dir("old"));
        old.store(ENV, b"<a/>").unwrap();
        let new_dir = temp_dir("new");
        let new = old.move_to(&new_dir).unwrap();
        assert!(old.entry(ENV).is_none());
        assert_eq!(new.entry(ENV).unwrap().size, 4);
        fs::remove_dir_all(old.dir()).unwrap();
        fs::remove_dir_all(new_dir).unwrap();
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("http://127.0.0.1:8080"), "127.0.0.1_8080");
    }
}
