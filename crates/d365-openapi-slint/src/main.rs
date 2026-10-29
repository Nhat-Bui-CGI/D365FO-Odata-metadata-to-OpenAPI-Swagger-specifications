// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use d365_fetch::{Cache, Environment, Progress, Settings, Token};
use d365_openapi_core::{Metadata, Options};
use d365_openapi_desktop::model::{
    AppState, EntityRow, OutputOptions, entity_rows, file_list_summary, filter_rows, format_bytes,
    format_time,
};
use d365_openapi_desktop::pipeline::{self, Event, ExportJob};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

struct App {
    ui: slint::Weak<AppWindow>,
    settings: Settings,
    settings_path: PathBuf,
    state: AppState,
    state_path: PathBuf,
    cache: Cache,
    /// Index of the chosen environment in `settings.environments`.
    current: Option<usize>,
    loaded: Option<(String, Arc<Metadata>)>,
    rows: Vec<EntityRow>,
    /// Row indices currently in the list, ascending.
    shown: Vec<usize>,
    shown_model: Rc<VecModel<EntityItem>>,
    env_model: Rc<VecModel<EnvItem>>,
    tokens: HashMap<String, Token>,
    cancel: Option<Arc<AtomicBool>>,
    /// Environment being edited in the dialog (`None` = adding).
    editing: Option<usize>,
    /// Output folder of the running export; its result is shown in a dialog.
    exporting: Option<PathBuf>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` with the app state. Never hold it across a native dialog: the
/// dialog's event loop can call back into the UI.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(a.borrow_mut().as_mut().expect("app initialised")))
}

impl App {
    fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }

    fn env(&self) -> Option<&Environment> {
        self.current.and_then(|i| self.settings.environments.get(i))
    }

    fn selection(&mut self) -> &mut BTreeSet<String> {
        let url = self.env().map(|e| e.url.clone()).unwrap_or_default();
        self.state.selections.entry(url).or_default()
    }

    fn save_settings(&self) -> Result<(), String> {
        self.settings
            .save(&self.settings_path)
            .map_err(|e| format!("Could not save settings: {e}"))
    }

    fn save_state(&self) {
        if let Err(e) = self.state.save(&self.state_path) {
            self.set_status(&format!("Could not save app state: {e}"), true);
        }
    }

    fn set_status(&self, text: &str, error: bool) {
        let ui = self.ui();
        ui.set_status_text(text.into());
        ui.set_status_error(error);
    }

    // ---------- environments ----------

    fn refresh_environments(&self) {
        self.env_model.set_vec(
            self.settings
                .environments
                .iter()
                .map(|e| EnvItem {
                    name: e.name.as_str().into(),
                    url: e.url.as_str().into(),
                })
                .collect::<Vec<_>>(),
        );
        self.ui()
            .set_current_env(self.current.map_or(-1, |i| i as i32));
        self.refresh_env_info();
    }

    fn refresh_env_info(&self) {
        let ui = self.ui();
        let Some(env) = self.env() else {
            ui.set_env_info("".into());
            ui.set_has_saved(false);
            return;
        };
        let entry = self.cache.entry(&env.url);
        let mut info = match &entry {
            Some(e) => format!(
                "Saved copy: {} ({})\nLast checked {}",
                format_time(e.downloaded_at),
                format_bytes(e.size),
                format_time(e.checked_at)
            ),
            None => "No metadata saved yet.".to_string(),
        };
        if self.tokens.get(&env.url).is_some_and(|t| !t.is_expired()) {
            info.push_str("\nSigned in");
        }
        ui.set_env_info(info.into());
        ui.set_has_saved(entry.is_some());
    }

    fn select_env(&mut self, index: usize) {
        if index >= self.settings.environments.len() {
            return;
        }
        self.current = Some(index);
        let url = self.settings.environments[index].url.clone();
        self.state.last_environment = Some(url.clone());
        self.save_state();
        self.loaded = None;
        self.rows.clear();
        self.ui().set_loaded(false);
        self.ui().set_search("".into());
        self.refresh_environments();
        self.refresh_list();
        if self.cache.entry(&url).is_some() {
            let cache = self.cache.clone();
            self.run_job("Loading saved metadata", move |emit, _| {
                pipeline::load_cached(emit, &cache, &url)
            });
        } else {
            self.set_status("No metadata saved yet. Sign in and download it.", false);
        }
    }

    fn open_editor(&mut self, existing: bool) {
        let ui = self.ui();
        let env = if existing { self.env().cloned() } else { None };
        self.editing = if existing { self.current } else { None };
        let env = env.unwrap_or_default();
        ui.set_editing_existing(existing);
        ui.set_editor_name(env.name.into());
        ui.set_editor_url(env.url.into());
        ui.set_editor_tenant(env.tenant.into());
        ui.set_editor_client(env.client_id.into());
        ui.set_editor_error("".into());
        ui.set_show_editor(true);
    }

    fn save_environment(&mut self) {
        let ui = self.ui();
        let env = Environment {
            name: ui.get_editor_name().into(),
            url: ui.get_editor_url().into(),
            tenant: ui.get_editor_tenant().trim().into(),
            client_id: ui.get_editor_client().trim().into(),
        };
        let env = match pipeline::validate_environment(&env) {
            Ok(env) => env,
            Err(e) => return ui.set_editor_error(e.into()),
        };
        let index = match self.editing {
            Some(i) if i < self.settings.environments.len() => {
                self.settings.environments[i] = env;
                i
            }
            _ => {
                self.settings.environments.push(env);
                self.settings.environments.len() - 1
            }
        };
        if let Err(e) = self.save_settings() {
            return ui.set_editor_error(e.into());
        }
        ui.set_show_editor(false);
        self.select_env(index);
    }

    fn remove_environment(&mut self) {
        let Some(i) = self.current else { return };
        let url = self.settings.environments.remove(i).url;
        let _ = self.cache.remove(&url);
        self.state.selections.remove(&url);
        self.tokens.remove(&url);
        self.current = None;
        self.loaded = None;
        self.rows.clear();
        self.ui().set_loaded(false);
        if let Err(e) = self.save_settings() {
            self.set_status(&e, true);
        }
        self.save_state();
        self.refresh_environments();
        self.refresh_list();
    }

    fn download(&mut self) {
        let Some(env) = self.env().cloned() else {
            return;
        };
        let cache = self.cache.clone();
        let token = self.tokens.get(&env.url).cloned();
        let label = if cache.entry(&env.url).is_some() {
            "Checking for updates"
        } else {
            "Downloading"
        };
        self.run_job(label, move |emit, cancel| {
            pipeline::download(emit, cancel, &cache, &env, token)
        });
    }

    // ---------- background jobs ----------

    fn run_job(
        &mut self,
        label: &str,
        job: impl FnOnce(&(dyn Fn(Event) + Sync), &AtomicBool) -> Result<String, String>
        + Send
        + 'static,
    ) {
        if self.cancel.is_some() {
            return self.set_status("Another task is still running.", true);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let ui = self.ui();
        ui.set_busy(true);
        ui.set_sign_in_url("".into());
        ui.set_show_progress(false);
        self.set_status(&format!("{label}…"), false);
        std::thread::spawn(move || {
            let emit = |e: Event| {
                let _ = slint::invoke_from_event_loop(move || with_app(|a| a.on_event(e)));
            };
            let result = job(&emit, &cancel);
            let _ = slint::invoke_from_event_loop(move || with_app(|a| a.on_finished(result)));
        });
    }

    fn on_event(&mut self, e: Event) {
        let ui = self.ui();
        match e {
            Event::Status(text) => self.set_status(&text, false),
            Event::Progress(p) => {
                let (progress, text) = match p {
                    Progress::Connecting => (
                        -1.0,
                        "Waiting for the server to send the metadata…".to_string(),
                    ),
                    Progress::Downloading { received, total } => match total {
                        Some(t) if t > 0 => (
                            received as f32 / t as f32,
                            format!(
                                "Downloading metadata… {} of {}",
                                format_bytes(received),
                                format_bytes(t)
                            ),
                        ),
                        _ => (
                            -1.0,
                            format!("Downloading metadata… {} received", format_bytes(received)),
                        ),
                    },
                    Progress::Done { .. } => (1.0, "Download complete.".to_string()),
                };
                ui.set_show_progress(true);
                ui.set_progress(progress);
                self.set_status(&text, false);
            }
            Event::SignInUrl(url) => ui.set_sign_in_url(url.into()),
            Event::Token { env_url, token } => {
                self.tokens.insert(env_url, token);
            }
            Event::SignedOut { env_url } => {
                self.tokens.remove(&env_url);
            }
            Event::Loaded { env_url, metadata } => {
                if self.env().is_none_or(|e| e.url != env_url) {
                    return; // The user switched environments meanwhile.
                }
                self.rows = entity_rows(&metadata);
                let names: BTreeSet<&str> = self.rows.iter().map(|r| r.set_name.as_str()).collect();
                let sel = self.state.selections.entry(env_url.clone()).or_default();
                sel.retain(|s| names.contains(s.as_str()));
                self.loaded = Some((env_url, metadata));
                ui.set_loaded(true);
                self.refresh_list();
            }
        }
    }

    fn on_finished(&mut self, result: Result<String, String>) {
        self.cancel = None;
        let ui = self.ui();
        ui.set_busy(false);
        ui.set_show_progress(false);
        ui.set_sign_in_url("".into());
        if self.exporting.take().is_some() {
            ui.set_export_ok(result.is_ok());
            ui.set_export_text(result.as_ref().unwrap_or_else(|e| e).as_str().into());
            ui.set_show_export_result(true);
        }
        match result {
            Ok(text) => self.set_status(&text, false),
            Err(text) => self.set_status(&text, true),
        }
        self.refresh_env_info();
    }

    // ---------- entity list ----------

    fn refresh_list(&mut self) {
        let ui = self.ui();
        let selected = self.selection().clone();
        self.shown = filter_rows(
            &self.rows,
            &ui.get_search(),
            &selected,
            ui.get_selected_only(),
        );
        let items: Vec<EntityItem> = self
            .shown
            .iter()
            .map(|&i| {
                let r = &self.rows[i];
                EntityItem {
                    set: r.set_name.as_str().into(),
                    type_name: r.type_name.as_str().into(),
                    checked: selected.contains(&r.set_name),
                    index: i as i32,
                }
            })
            .collect();
        self.shown_model.set_vec(items);
        self.refresh_count();
    }

    fn refresh_count(&mut self) {
        self.refresh_planned_files();
        let total = self.rows.len();
        let shown = self.shown.len();
        let selected = self.selection().len();
        let text = if total == 0 {
            String::new()
        } else if shown == total {
            format!("{total} entities · {selected} selected")
        } else {
            format!("{shown} of {total} entities shown · {selected} selected")
        };
        self.ui().set_entity_count(text.into());
    }

    fn toggle_entity(&mut self, index: usize, checked: bool) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let name = row.set_name.clone();
        if checked {
            self.selection().insert(name);
        } else {
            self.selection().remove(&name);
        }
        // Keep the model in step so the row stays right when it scrolls back in.
        if let Ok(pos) = self.shown.binary_search(&index)
            && let Some(mut item) = self.shown_model.row_data(pos)
        {
            item.checked = checked;
            self.shown_model.set_row_data(pos, item);
        }
        self.save_state();
        self.refresh_count();
    }

    fn select_shown(&mut self) {
        let names: Vec<String> = self
            .shown
            .iter()
            .map(|&i| self.rows[i].set_name.clone())
            .collect();
        self.selection().extend(names);
        self.save_state();
        self.refresh_list();
    }

    fn clear_selection(&mut self) {
        self.selection().clear();
        self.save_state();
        self.refresh_list();
    }

    // ---------- output ----------

    fn show_output(&self) {
        let ui = self.ui();
        let o = &self.state.output;
        ui.set_swagger2(o.swagger2);
        ui.set_openapi30(o.openapi30);
        ui.set_openapi31(o.openapi31);
        ui.set_json(o.json);
        ui.set_yaml(o.yaml);
        ui.set_include_operations(o.include_operations);
        ui.set_include_navigation(o.include_navigation);
        ui.set_include_server_url(o.include_server_url);
        ui.set_output_dir(
            o.output_dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default()
                .into(),
        );
        ui.set_combined(o.combined);
        ui.set_file_name(o.file_name.as_str().into());
        ui.set_per_entity(o.per_entity);
        self.refresh_planned_files();
    }

    fn output_changed(&mut self) {
        let ui = self.ui();
        let dir = ui.get_output_dir().trim().to_string();
        self.state.output = OutputOptions {
            swagger2: ui.get_swagger2(),
            openapi30: ui.get_openapi30(),
            openapi31: ui.get_openapi31(),
            json: ui.get_json(),
            yaml: ui.get_yaml(),
            include_operations: ui.get_include_operations(),
            include_navigation: ui.get_include_navigation(),
            include_server_url: ui.get_include_server_url(),
            output_dir: (!dir.is_empty()).then(|| PathBuf::from(dir)),
            combined: ui.get_combined(),
            file_name: ui.get_file_name().into(),
            per_entity: ui.get_per_entity(),
        };
        self.save_state();
        self.refresh_planned_files();
    }

    fn refresh_planned_files(&self) {
        let o = &self.state.output;
        let files = o.planned_files(
            &o.output_dir.clone().unwrap_or_default(),
            &self.selected_sets(),
        );
        let text = match o.problem() {
            Some(p) => p.to_string(),
            None if files.is_empty() => "Select at least one entity.".to_string(),
            None => format!(
                "Writes {} file(s):\n{}",
                files.len(),
                file_list_summary(&files, 12).join("\n")
            ),
        };
        self.ui().set_planned_files(text.into());
    }

    fn options(&self) -> Options {
        pipeline::base_options(self.env(), &self.state.output)
    }

    fn selected_sets(&self) -> Vec<String> {
        self.env()
            .and_then(|e| self.state.selections.get(&e.url))
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn preview(&mut self) {
        let Some((_, md)) = self.loaded.clone() else {
            return;
        };
        let o = &self.state.output;
        let (Some(&version), Some(&format)) = (o.versions().first(), o.formats().first()) else {
            return self.set_status("Pick a format, and JSON or YAML.", true);
        };
        let opts = Options {
            version,
            ..self.options()
        };
        match pipeline::preview(&md, &self.selected_sets(), &opts, format, 300_000) {
            Ok(text) => {
                let ui = self.ui();
                ui.set_preview_text(text.into());
                ui.set_show_preview(true);
            }
            Err(e) => self.set_status(&e, true),
        }
    }

    fn export(&mut self) {
        let Some((_, metadata)) = self.loaded.clone() else {
            return;
        };
        let Some(dir) = self.state.output.output_dir.clone() else {
            return self.set_status("Choose an output folder first.", true);
        };
        let entity_sets = self.selected_sets();
        let job = ExportJob {
            metadata,
            files: self.state.output.planned_files(&dir, &entity_sets),
            entity_sets,
            base_options: self.options(),
        };
        self.run_job("Exporting", move |emit, _| pipeline::export(emit, job));
        if self.cancel.is_some() {
            self.exporting = Some(dir);
        }
    }

    // ---------- settings ----------

    fn open_settings(&self) {
        let ui = self.ui();
        ui.set_cache_dir(self.cache.dir().display().to_string().into());
        ui.set_settings_error("".into());
        ui.set_show_settings(true);
    }

    fn save_cache_dir(&mut self) {
        let ui = self.ui();
        let new_dir = PathBuf::from(ui.get_cache_dir().trim());
        if new_dir.as_os_str().is_empty() {
            return ui.set_settings_error("Enter a folder.".into());
        }
        if self.cancel.is_some() {
            return ui.set_settings_error("Wait for the current task to finish.".into());
        }
        match self.cache.move_to(&new_dir) {
            Ok(cache) => self.cache = cache,
            Err(e) => {
                return ui
                    .set_settings_error(format!("Could not move the saved metadata: {e}").into());
            }
        }
        self.settings.cache_dir = (new_dir != Settings::default_cache_dir()).then_some(new_dir);
        if let Err(e) = self.save_settings() {
            return ui.set_settings_error(e.into());
        }
        ui.set_show_settings(false);
        self.set_status(
            &format!("Saved metadata is now in {}", self.cache.dir().display()),
            false,
        );
        self.refresh_env_info();
    }
}

/// Shows a folder picker. Call it without holding the app state.
fn pick_folder(start: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if !start.trim().is_empty() {
        dialog = dialog.set_directory(start.trim());
    }
    dialog.pick_folder().map(|p| p.display().to_string())
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;

    let settings_path = Settings::default_path();
    let settings = Settings::load(&settings_path).unwrap_or_default();
    let state_path = AppState::default_path();
    let state = AppState::load(&state_path);
    let cache = Cache::new(settings.cache_dir());
    let shown_model = Rc::new(VecModel::default());
    let env_model = Rc::new(VecModel::default());
    ui.set_entities(ModelRc::from(shown_model.clone()));
    ui.set_environments(ModelRc::from(env_model.clone()));
    ui.set_default_cache_dir(Settings::default_cache_dir().display().to_string().into());
    ui.set_settings_path(settings_path.display().to_string().into());
    ui.set_version(env!("CARGO_PKG_VERSION").into());

    let last = state
        .last_environment
        .as_ref()
        .and_then(|u| settings.environments.iter().position(|e| &e.url == u));
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            ui: ui.as_weak(),
            settings,
            settings_path,
            state,
            state_path,
            cache,
            current: None,
            loaded: None,
            rows: Vec::new(),
            shown: Vec::new(),
            shown_model,
            env_model,
            tokens: HashMap::new(),
            cancel: None,
            exporting: None,
            editing: None,
        })
    });
    with_app(|a| {
        a.refresh_environments();
        a.show_output();
        match last {
            Some(i) => a.select_env(i),
            None => a.refresh_list(),
        }
    });

    ui.on_select_env(|i| with_app(|a| a.select_env(i as usize)));
    ui.on_add_env(|| with_app(|a| a.open_editor(false)));
    ui.on_edit_env(|| with_app(|a| a.open_editor(true)));
    ui.on_save_env(|| with_app(|a| a.save_environment()));
    ui.on_remove_env(|| with_app(|a| a.remove_environment()));
    ui.on_download(|| with_app(|a| a.download()));
    ui.on_cancel(|| {
        with_app(|a| {
            if let Some(c) = &a.cancel {
                c.store(true, Ordering::Relaxed);
            }
        })
    });
    {
        let weak = ui.as_weak();
        ui.on_copy_sign_in_link(move || {
            let Some(ui) = weak.upgrade() else { return };
            let url = ui.get_sign_in_url().to_string();
            let copied = arboard::Clipboard::new().and_then(|mut c| c.set_text(url));
            with_app(|a| match copied {
                Ok(()) => a.set_status("Sign-in link copied. Paste it into your browser.", false),
                Err(e) => a.set_status(&format!("Could not copy the link: {e}"), true),
            });
        });
    }
    ui.on_search_changed(|| with_app(|a| a.refresh_list()));
    ui.on_toggle_entity(|i, checked| with_app(|a| a.toggle_entity(i as usize, checked)));
    ui.on_select_shown(|| with_app(|a| a.select_shown()));
    ui.on_clear_selection(|| with_app(|a| a.clear_selection()));
    ui.on_output_changed(|| with_app(|a| a.output_changed()));
    {
        let weak = ui.as_weak();
        ui.on_browse_output(move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(dir) = pick_folder(&ui.get_output_dir()) {
                ui.set_output_dir(SharedString::from(dir));
                with_app(|a| a.output_changed());
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_browse_cache_dir(move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(dir) = pick_folder(&ui.get_cache_dir()) {
                ui.set_cache_dir(SharedString::from(dir));
            }
        });
    }
    ui.on_preview(|| with_app(|a| a.preview()));
    ui.on_export(|| with_app(|a| a.export()));
    ui.on_open_output_folder(|| {
        with_app(|a| {
            if let Some(dir) = a.state.output.output_dir.clone()
                && let Err(e) = pipeline::open_folder(&dir)
            {
                a.set_status(&e, true);
            }
        })
    });
    ui.on_open_settings(|| with_app(|a| a.open_settings()));
    ui.on_save_cache_dir(|| with_app(|a| a.save_cache_dir()));

    ui.run()
}
