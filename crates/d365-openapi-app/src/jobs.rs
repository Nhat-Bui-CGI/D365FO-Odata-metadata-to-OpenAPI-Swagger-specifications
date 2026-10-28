//! Work that runs off the UI thread. Each job sends [`Msg`]s back and asks
//! egui to repaint so progress shows immediately.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::Duration;

use d365_fetch::{Cache, Environment, Progress, StoreOutcome, Token};
use d365_openapi_core::{Metadata, Options, OutputFormat, SpecVersion};

use crate::model::format_time;

pub enum Msg {
    Status(String),
    Progress(Progress),
    /// The sign-in page URL, in case the browser did not open.
    SignInUrl(String),
    Loaded {
        env_url: String,
        metadata: Arc<Metadata>,
    },
    Token {
        env_url: String,
        token: Token,
    },
    /// A job finished; the text is shown in the status bar.
    Finished(Result<String, String>),
}

pub struct Ctx {
    pub tx: Sender<Msg>,
    pub egui: eframe::egui::Context,
    pub cancel: Arc<AtomicBool>,
}

impl Ctx {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.egui.request_repaint();
    }

    fn status(&self, s: impl Into<String>) {
        self.send(Msg::Status(s.into()));
    }

    fn finish(&self, r: Result<String, String>) {
        self.send(Msg::Finished(r));
    }
}

pub fn spawn(ctx: Ctx, job: impl FnOnce(&Ctx) -> Result<String, String> + Send + 'static) {
    std::thread::spawn(move || {
        let result = job(&ctx);
        ctx.finish(result);
    });
}

/// Reads and parses the cached metadata for an environment.
pub fn load_cached(ctx: &Ctx, cache: &Cache, env_url: &str) -> Result<String, String> {
    let entry = cache
        .entry(env_url)
        .ok_or("No metadata saved for this environment yet.")?;
    ctx.status("Loading saved metadata…");
    let metadata = parse_file(ctx, cache, env_url)?;
    let n = metadata.entity_sets.len();
    ctx.send(Msg::Loaded {
        env_url: env_url.into(),
        metadata,
    });
    Ok(format!(
        "Loaded {n} entities from the saved copy (downloaded {}).",
        format_time(entry.downloaded_at)
    ))
}

fn parse_file(ctx: &Ctx, cache: &Cache, env_url: &str) -> Result<Arc<Metadata>, String> {
    ctx.status("Reading metadata…");
    let file = std::fs::File::open(cache.metadata_path(env_url)).map_err(|e| e.to_string())?;
    let md = d365_openapi_core::parse_reader(std::io::BufReader::new(file))
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(md))
}

/// Signs in if needed, downloads `$metadata`, updates the cache and reloads.
pub fn download(
    ctx: &Ctx,
    cache: &Cache,
    env: &Environment,
    token: Option<Token>,
) -> Result<String, String> {
    let token = match token.filter(|t| !t.is_expired()) {
        Some(t) => t,
        None => {
            ctx.status("Waiting for you to sign in in the browser…");
            let cfg = env.auth_config();
            let t = d365_fetch::sign_in_interactive(
                &cfg,
                Duration::from_secs(300),
                &ctx.cancel,
                |url| {
                    ctx.send(Msg::SignInUrl(url.to_string()));
                    webbrowser::open(url)
                },
            )
            .map_err(|e| e.to_string())?;
            ctx.send(Msg::Token {
                env_url: env.url.clone(),
                token: t.clone(),
            });
            t
        }
    };
    ctx.status("Requesting metadata from D365FO (this can take a minute)…");
    let data = d365_fetch::download_metadata(&env.url, &token.access_token, &ctx.cancel, |p| {
        ctx.send(Msg::Progress(p))
    })
    .map_err(|e| e.to_string())?;
    ctx.status("Saving…");
    let outcome = cache.store(&env.url, &data).map_err(|e| e.to_string())?;
    let metadata = parse_file(ctx, cache, &env.url)?;
    let n = metadata.entity_sets.len();
    ctx.send(Msg::Loaded {
        env_url: env.url.clone(),
        metadata,
    });
    Ok(match outcome {
        StoreOutcome::New => format!("Downloaded and saved metadata: {n} entities."),
        StoreOutcome::Updated => {
            format!("Metadata changed in D365FO. Saved the new version: {n} entities.")
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

pub fn export(ctx: &Ctx, job: ExportJob) -> Result<String, String> {
    let sets: Vec<&str> = job.entity_sets.iter().map(String::as_str).collect();
    let mut written = Vec::new();
    let mut last: Option<(SpecVersion, serde_json::Value)> = None;
    for (version, format, path) in &job.files {
        ctx.status(format!("Writing {}…", path.display()));
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
