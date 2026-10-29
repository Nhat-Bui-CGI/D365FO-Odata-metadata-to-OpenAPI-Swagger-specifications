//! Long-running work, independent of any UI toolkit. Callers run these on a
//! background thread and forward [`Event`]s to their UI.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use d365_fetch::{Cache, Environment, Progress, StoreOutcome, Token};
use d365_openapi_core::{Metadata, Options, OutputFormat, Platform, SpecVersion};

use crate::model::format_time;

pub enum Event {
    Status(String),
    Progress(Progress),
    /// The sign-in page URL, in case the browser did not open.
    SignInUrl(String),
    Token {
        env_url: String,
        token: Token,
    },
    /// The saved token was refused; the next download signs in again.
    SignedOut {
        env_url: String,
    },
    Loaded {
        env_url: String,
        metadata: Arc<Metadata>,
    },
}

pub type Emit<'a> = &'a (dyn Fn(Event) + Sync);

/// Reads and parses the cached metadata for an environment.
pub fn load_cached(emit: Emit, cache: &Cache, env_url: &str) -> Result<String, String> {
    let entry = cache
        .entry(env_url)
        .ok_or("No metadata saved for this environment yet.")?;
    emit(Event::Status("Loading saved metadata…".into()));
    let metadata = parse_file(emit, cache, env_url)?;
    let n = metadata.entity_sets.len();
    emit(Event::Loaded {
        env_url: env_url.into(),
        metadata,
    });
    Ok(format!(
        "Loaded {n} entities from the saved copy (downloaded {}).",
        format_time(entry.downloaded_at)
    ))
}

fn parse_file(emit: Emit, cache: &Cache, env_url: &str) -> Result<Arc<Metadata>, String> {
    emit(Event::Status("Reading metadata…".into()));
    let file = std::fs::File::open(cache.metadata_path(env_url)).map_err(|e| e.to_string())?;
    let md = d365_openapi_core::parse_reader(std::io::BufReader::new(file))
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(md))
}

/// Signs in if needed, downloads `$metadata`, updates the cache and reloads.
pub fn download(
    emit: Emit,
    cancel: &AtomicBool,
    cache: &Cache,
    env: &Environment,
    token: Option<Token>,
) -> Result<String, String> {
    let sign_in = || -> Result<Token, String> {
        emit(Event::Status(
            "Waiting for you to sign in in the browser… (Cancel to start over)".into(),
        ));
        let cfg = env.auth_config();
        let t = d365_fetch::sign_in_interactive(&cfg, Duration::from_secs(300), cancel, |url| {
            emit(Event::SignInUrl(url.to_string()));
            webbrowser::open(url)
        })
        .map_err(|e| e.to_string())?;
        emit(Event::Token {
            env_url: env.url.clone(),
            token: t.clone(),
        });
        Ok(t)
    };
    let platform = Platform::detect(&env.url);
    let fetch = |token: &Token| {
        emit(Event::Status(format!(
            "Requesting metadata from {} (this can take a minute)…",
            platform.short_name()
        )));
        d365_fetch::download_metadata(
            &env.url,
            platform.data_path(),
            &token.access_token,
            cancel,
            |p| emit(Event::Progress(p)),
        )
    };
    let refused = |e: &d365_fetch::Error| {
        matches!(
            e,
            d365_fetch::Error::Status {
                status: 401 | 403,
                ..
            }
        )
    };
    let signed_out = || {
        emit(Event::SignedOut {
            env_url: env.url.clone(),
        })
    };

    let data = match token.filter(|t| !t.is_expired()) {
        // A saved sign-in can stop working (other account, access removed):
        // forget it and sign in again once.
        Some(saved) => match fetch(&saved) {
            Err(e) if refused(&e) => {
                signed_out();
                fetch(&sign_in()?)
            }
            other => other,
        },
        None => fetch(&sign_in()?),
    };
    let data = data.map_err(|e| {
        if refused(&e) {
            signed_out();
            format!(
                "{e}\nThis account has no access to the environment. \
                 Download again to sign in with another account."
            )
        } else {
            e.to_string()
        }
    })?;
    emit(Event::Status("Saving…".into()));
    let outcome = cache.store(&env.url, &data).map_err(|e| e.to_string())?;
    let metadata = parse_file(emit, cache, &env.url)?;
    let n = metadata.entity_sets.len();
    emit(Event::Loaded {
        env_url: env.url.clone(),
        metadata,
    });
    Ok(match outcome {
        StoreOutcome::New => format!("Downloaded and saved metadata: {n} entities."),
        StoreOutcome::Updated => {
            format!(
                "Metadata changed in {}. Saved the new version: {n} entities.",
                platform.short_name()
            )
        }
        StoreOutcome::Unchanged => "No changes. The saved metadata is already up to date.".into(),
    })
}

pub struct ExportJob {
    pub metadata: Arc<Metadata>,
    pub entity_sets: Vec<String>,
    pub files: Vec<(SpecVersion, OutputFormat, PathBuf)>,
    pub base_options: Options,
}

pub fn export(emit: Emit, job: ExportJob) -> Result<String, String> {
    let sets: Vec<&str> = job.entity_sets.iter().map(String::as_str).collect();
    let mut written = Vec::new();
    let mut last: Option<(SpecVersion, serde_json::Value)> = None;
    for (version, format, path) in &job.files {
        emit(Event::Status(format!("Writing {}…", path.display())));
        // Files are ordered by version, so each document is generated once.
        if last.as_ref().is_none_or(|(v, _)| v != version) {
            let opts = Options {
                version: *version,
                ..job.base_options.clone()
            };
            let doc = d365_openapi_core::generate(&job.metadata, &sets, &opts)
                .map_err(|e| e.to_string())?;
            last = Some((*version, doc));
        }
        let text = d365_openapi_core::render(&last.as_ref().unwrap().1, *format)
            .map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path.clone());
    }
    let dir = written
        .first()
        .and_then(|p| p.parent())
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    Ok(format!(
        "Exported {} file(s) for {} entities to {dir}",
        written.len(),
        sets.len()
    ))
}

/// Generates one document as text, for previews. Very large output is cut off.
pub fn preview(
    metadata: &Metadata,
    entity_sets: &[String],
    options: &Options,
    format: OutputFormat,
    limit: usize,
) -> Result<String, String> {
    let sets: Vec<&str> = entity_sets.iter().map(String::as_str).collect();
    let doc = d365_openapi_core::generate(metadata, &sets, options).map_err(|e| e.to_string())?;
    let mut text = d365_openapi_core::render(&doc, format).map_err(|e| e.to_string())?;
    if text.len() > limit {
        let cut = (0..=limit)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        text.truncate(cut);
        text.push_str("\n\n… preview truncated; the exported file is complete.");
    }
    Ok(text)
}

/// Spec options for an environment and the saved output choices.
pub fn base_options(env: Option<&Environment>, output: &crate::model::OutputOptions) -> Options {
    let platform = env.map_or_else(Platform::default, |e| Platform::detect(&e.url));
    Options {
        version: SpecVersion::OpenApi30,
        platform,
        title: match env {
            Some(env) => format!("{} OData API ({})", platform.short_name(), env.name),
            None => Options::default().title,
        },
        server_url: if output.include_server_url {
            env.map(|e| e.url.clone())
        } else {
            None
        },
        include_operations: output.include_operations,
        include_navigation: output.include_navigation,
    }
}

/// Checks and normalises an environment entered by the user.
pub fn validate_environment(env: &Environment) -> Result<Environment, String> {
    let url = d365_fetch::normalize_environment_url(&env.url)
        .map_err(|_| "Enter a valid environment URL.".to_string())?;
    let name = if env.name.trim().is_empty() {
        url.split("://").nth(1).unwrap_or(&url).to_string()
    } else {
        env.name.trim().to_string()
    };
    Ok(Environment {
        name,
        url,
        tenant: env.tenant.trim().into(),
        client_id: env.client_id.trim().into(),
    })
}
