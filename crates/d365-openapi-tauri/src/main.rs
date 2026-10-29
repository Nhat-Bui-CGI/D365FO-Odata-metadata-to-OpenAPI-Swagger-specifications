// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use d365_fetch::{Cache, Environment, Progress, Settings, Token};
use d365_openapi_core::{Metadata, Options};
use d365_openapi_desktop::model::{
    AppState, OutputOptions, entity_rows, format_bytes, format_time,
};
use d365_openapi_desktop::pipeline::{self, Event, ExportJob};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

struct Inner {
    settings: Settings,
    settings_path: PathBuf,
    state: AppState,
    state_path: PathBuf,
    cache: Cache,
    loaded: Option<(String, Arc<Metadata>)>,
    tokens: HashMap<String, Token>,
    cancel: Option<Arc<AtomicBool>>,
}

type Shared = Arc<Mutex<Inner>>;

impl Inner {
    fn environment(&self, url: &str) -> Result<Environment, String> {
        self.settings
            .environments
            .iter()
            .find(|e| e.url == url)
            .cloned()
            .ok_or_else(|| "Unknown environment.".to_string())
    }

    fn save_settings(&self) -> Result<(), String> {
        self.settings
            .save(&self.settings_path)
            .map_err(|e| format!("Could not save settings: {e}"))
    }

    fn save_state(&self) -> Result<(), String> {
        self.state
            .save(&self.state_path)
            .map_err(|e| format!("Could not save app state: {e}"))
    }
}

// ---------- events sent to the page ----------

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum JobEvent {
    Started {
        label: String,
    },
    Status {
        text: String,
    },
    Progress {
        received: u64,
        total: Option<u64>,
        text: String,
    },
    SignInUrl {
        url: String,
    },
    Finished {
        ok: bool,
        text: String,
    },
}

#[derive(Clone, Serialize)]
struct Row {
    set: String,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadedEvent {
    env_url: String,
    rows: Vec<Row>,
    selected: Vec<String>,
}

fn emit_job(app: &AppHandle, e: JobEvent) {
    let _ = app.emit("job", e);
}

/// Runs a pipeline job on a background thread, forwarding its events to the page.
fn run_job(
    app: AppHandle,
    shared: Shared,
    label: &str,
    job: impl FnOnce(&(dyn Fn(Event) + Sync), &AtomicBool) -> Result<String, String> + Send + 'static,
) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut inner = shared.lock().unwrap();
        if inner.cancel.is_some() {
            return Err("Another task is still running.".into());
        }
        inner.cancel = Some(cancel.clone());
    }
    emit_job(
        &app,
        JobEvent::Started {
            label: label.into(),
        },
    );
    std::thread::spawn(move || {
        let emit = |e: Event| match e {
            Event::Status(text) => emit_job(&app, JobEvent::Status { text }),
            Event::Progress(p) => {
                let (received, total, text) = match p {
                    Progress::Connecting => (
                        0,
                        None,
                        "Waiting for the server to send the metadata…".to_string(),
                    ),
                    Progress::Downloading { received, total } => (
                        received,
                        total,
                        match total {
                            Some(t) => format!(
                                "Downloading metadata… {} of {}",
                                format_bytes(received),
                                format_bytes(t)
                            ),
                            None => {
                                format!("Downloading metadata… {} received", format_bytes(received))
                            }
                        },
                    ),
                    Progress::Done { bytes } => {
                        (bytes, Some(bytes), "Download complete.".to_string())
                    }
                };
                emit_job(
                    &app,
                    JobEvent::Progress {
                        received,
                        total,
                        text,
                    },
                );
            }
            Event::SignInUrl(url) => emit_job(&app, JobEvent::SignInUrl { url }),
            Event::Token { env_url, token } => {
                shared.lock().unwrap().tokens.insert(env_url, token);
            }
            Event::Loaded { env_url, metadata } => {
                let rows = entity_rows(&metadata);
                let selected = {
                    let mut inner = shared.lock().unwrap();
                    let names: BTreeSet<&str> = rows.iter().map(|r| r.set_name.as_str()).collect();
                    let sel = inner.state.selections.entry(env_url.clone()).or_default();
                    sel.retain(|s| names.contains(s.as_str()));
                    let selected = sel.iter().cloned().collect();
                    inner.loaded = Some((env_url.clone(), metadata));
                    selected
                };
                let rows = rows
                    .into_iter()
                    .map(|r| Row {
                        set: r.set_name,
                        type_name: r.type_name,
                    })
                    .collect();
                let _ = app.emit(
                    "loaded",
                    LoadedEvent {
                        env_url,
                        rows,
                        selected,
                    },
                );
            }
        };
        let result = job(&emit, &cancel);
        shared.lock().unwrap().cancel = None;
        let (ok, text) = match result {
            Ok(t) => (true, t),
            Err(t) => (false, t),
        };
        emit_job(&app, JobEvent::Finished { ok, text });
    });
    Ok(())
}

// ---------- commands ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitData {
    environments: Vec<Environment>,
    last_environment: Option<String>,
    output: OutputOptions,
    cache_dir: String,
    default_cache_dir: String,
    settings_path: String,
}

#[tauri::command]
fn init(shared: State<'_, Shared>) -> InitData {
    let inner = shared.lock().unwrap();
    InitData {
        environments: inner.settings.environments.clone(),
        last_environment: inner.state.last_environment.clone(),
        output: inner.state.output.clone(),
        cache_dir: inner.cache.dir().display().to_string(),
        default_cache_dir: Settings::default_cache_dir().display().to_string(),
        settings_path: inner.settings_path.display().to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentInfo {
    downloaded_at: Option<String>,
    checked_at: Option<String>,
    size: Option<String>,
    signed_in: bool,
}

#[tauri::command]
fn environment_info(shared: State<'_, Shared>, url: String) -> EnvironmentInfo {
    let inner = shared.lock().unwrap();
    let entry = inner.cache.entry(&url);
    EnvironmentInfo {
        downloaded_at: entry.as_ref().map(|e| format_time(e.downloaded_at)),
        checked_at: entry.as_ref().map(|e| format_time(e.checked_at)),
        size: entry.as_ref().map(|e| format_bytes(e.size)),
        signed_in: inner.tokens.get(&url).is_some_and(|t| !t.is_expired()),
    }
}

/// Adds (index `None`) or replaces an environment. Returns the saved environment.
#[tauri::command]
fn save_environment(
    shared: State<'_, Shared>,
    index: Option<usize>,
    env: Environment,
) -> Result<Environment, String> {
    let env = pipeline::validate_environment(&env)?;
    let mut inner = shared.lock().unwrap();
    match index {
        Some(i) if i < inner.settings.environments.len() => {
            inner.settings.environments[i] = env.clone()
        }
        _ => inner.settings.environments.push(env.clone()),
    }
    inner.save_settings()?;
    Ok(env)
}

#[tauri::command]
fn remove_environment(shared: State<'_, Shared>, url: String) -> Result<(), String> {
    let mut inner = shared.lock().unwrap();
    inner.settings.environments.retain(|e| e.url != url);
    let _ = inner.cache.remove(&url);
    inner.state.selections.remove(&url);
    inner.tokens.remove(&url);
    if inner.loaded.as_ref().is_some_and(|(u, _)| *u == url) {
        inner.loaded = None;
    }
    inner.save_settings()?;
    inner.save_state()
}

/// Loads saved metadata for an environment. Returns false if nothing is saved yet.
#[tauri::command]
fn load_environment(
    app: AppHandle,
    shared: State<'_, Shared>,
    url: String,
) -> Result<bool, String> {
    let cache = {
        let mut inner = shared.lock().unwrap();
        inner.environment(&url)?;
        inner.state.last_environment = Some(url.clone());
        inner.loaded = None;
        let _ = inner.save_state();
        if inner.cache.entry(&url).is_none() {
            return Ok(false);
        }
        inner.cache.clone()
    };
    run_job(app, shared.inner().clone(), "Loading", move |emit, _| {
        pipeline::load_cached(emit, &cache, &url)
    })?;
    Ok(true)
}

#[tauri::command]
fn download(app: AppHandle, shared: State<'_, Shared>, url: String) -> Result<(), String> {
    let (env, cache, token) = {
        let inner = shared.lock().unwrap();
        (
            inner.environment(&url)?,
            inner.cache.clone(),
            inner.tokens.get(&url).cloned(),
        )
    };
    let label = if cache.entry(&url).is_some() {
        "Checking for updates"
    } else {
        "Downloading"
    };
    run_job(app, shared.inner().clone(), label, move |emit, cancel| {
        pipeline::download(emit, cancel, &cache, &env, token)
    })
}

#[tauri::command]
fn cancel(shared: State<'_, Shared>) {
    if let Some(c) = &shared.lock().unwrap().cancel {
        c.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
fn set_selection(
    shared: State<'_, Shared>,
    url: String,
    selected: Vec<String>,
) -> Result<(), String> {
    let mut inner = shared.lock().unwrap();
    inner
        .state
        .selections
        .insert(url, selected.into_iter().collect());
    inner.save_state()
}

#[tauri::command]
fn set_output(shared: State<'_, Shared>, output: OutputOptions) -> Result<Vec<String>, String> {
    let mut inner = shared.lock().unwrap();
    inner.state.output = output;
    inner.save_state()?;
    Ok(planned_file_names(&inner.state.output))
}

fn planned_file_names(o: &OutputOptions) -> Vec<String> {
    o.planned_files(&o.output_dir.clone().unwrap_or_default())
        .into_iter()
        .map(|(_, _, p)| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[tauri::command]
async fn pick_folder(start: Option<String>) -> Option<String> {
    let mut dialog = rfd::AsyncFileDialog::new();
    if let Some(s) = start.filter(|s| !s.is_empty()) {
        dialog = dialog.set_directory(s);
    }
    dialog
        .pick_folder()
        .await
        .map(|f| f.path().display().to_string())
}

fn loaded_for(inner: &Inner, url: &str) -> Result<Arc<Metadata>, String> {
    match &inner.loaded {
        Some((u, md)) if u == url => Ok(md.clone()),
        _ => Err("Load the metadata first.".into()),
    }
}

#[tauri::command]
fn preview(shared: State<'_, Shared>, url: String) -> Result<String, String> {
    let inner = shared.lock().unwrap();
    let md = loaded_for(&inner, &url)?;
    let o = &inner.state.output;
    let version = *o.versions().first().ok_or("Pick a format.")?;
    let format = *o.formats().first().ok_or("Pick JSON or YAML.")?;
    let env = inner.environment(&url).ok();
    let opts = Options {
        version,
        ..pipeline::base_options(env.as_ref(), o)
    };
    let selected: Vec<String> = inner
        .state
        .selections
        .get(&url)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    pipeline::preview(&md, &selected, &opts, format, 2_000_000)
}

#[tauri::command]
fn export(app: AppHandle, shared: State<'_, Shared>, url: String) -> Result<(), String> {
    let job = {
        let inner = shared.lock().unwrap();
        let metadata = loaded_for(&inner, &url)?;
        let o = &inner.state.output;
        let dir = o
            .output_dir
            .clone()
            .ok_or("Choose an output folder first.")?;
        let env = inner.environment(&url).ok();
        ExportJob {
            metadata,
            entity_sets: inner
                .state
                .selections
                .get(&url)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default(),
            files: o.planned_files(&dir),
            base_options: pipeline::base_options(env.as_ref(), o),
        }
    };
    run_job(app, shared.inner().clone(), "Exporting", move |emit, _| {
        pipeline::export(emit, job)
    })
}

/// Moves saved metadata to a new folder. Returns the folder now in use.
#[tauri::command]
fn set_cache_dir(shared: State<'_, Shared>, dir: String) -> Result<String, String> {
    let new_dir = PathBuf::from(dir.trim());
    if new_dir.as_os_str().is_empty() {
        return Err("Enter a folder.".into());
    }
    let mut inner = shared.lock().unwrap();
    if inner.cancel.is_some() {
        return Err("Wait for the current task to finish.".into());
    }
    inner.cache = inner
        .cache
        .move_to(&new_dir)
        .map_err(|e| format!("Could not move the saved metadata: {e}"))?;
    inner.settings.cache_dir = (new_dir != Settings::default_cache_dir()).then_some(new_dir);
    inner.save_settings()?;
    Ok(inner.cache.dir().display().to_string())
}

#[tauri::command]
fn planned_files(shared: State<'_, Shared>) -> Vec<String> {
    planned_file_names(&shared.lock().unwrap().state.output)
}

fn main() {
    let settings_path = Settings::default_path();
    let settings = Settings::load(&settings_path).unwrap_or_default();
    let state_path = AppState::default_path();
    let state = AppState::load(&state_path);
    let cache = Cache::new(settings.cache_dir());
    let shared: Shared = Arc::new(Mutex::new(Inner {
        settings,
        settings_path,
        state,
        state_path,
        cache,
        loaded: None,
        tokens: HashMap::new(),
        cancel: None,
    }));

    tauri::Builder::default()
        .setup(move |app| {
            app.manage(shared);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init,
            environment_info,
            save_environment,
            remove_environment,
            load_environment,
            download,
            cancel,
            set_selection,
            set_output,
            pick_folder,
            preview,
            export,
            set_cache_dir,
            planned_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
