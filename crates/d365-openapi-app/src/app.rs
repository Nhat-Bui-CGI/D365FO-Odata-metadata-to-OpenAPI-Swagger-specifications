use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use d365_fetch::{Cache, Environment, Progress, Settings, Token};
use d365_openapi_core::{Metadata, Options, SpecVersion};
use eframe::egui::{self, Color32, RichText};

use crate::jobs::{self, Event, Msg};
use d365_openapi_desktop::model::{
    AppState, EntityRow, entity_rows, filter_rows, format_bytes, format_time,
};
use d365_openapi_desktop::pipeline::{self, ExportJob};

struct Busy {
    status: String,
    progress: Option<Progress>,
    sign_in_url: Option<String>,
    cancel: Arc<AtomicBool>,
    started: Instant,
}

struct Loaded {
    env_url: String,
    metadata: Arc<Metadata>,
    rows: Vec<EntityRow>,
}

struct EnvEditor {
    /// Index being edited, `None` for a new environment.
    index: Option<usize>,
    env: Environment,
    error: Option<String>,
}

struct SettingsEditor {
    cache_dir: String,
    error: Option<String>,
}

struct Preview {
    title: String,
    text: String,
}

pub struct App {
    settings: Settings,
    settings_path: PathBuf,
    state: AppState,
    state_path: PathBuf,
    state_dirty: Option<Instant>,
    cache: Cache,
    current_env: Option<usize>,
    loaded: Option<Loaded>,
    tokens: HashMap<String, Token>,
    search: String,
    selected_only: bool,
    filtered: Vec<usize>,
    filter_key: Option<(String, bool, usize)>,
    busy: Option<Busy>,
    last_result: Option<Result<String, String>>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    env_editor: Option<EnvEditor>,
    settings_editor: Option<SettingsEditor>,
    preview: Option<Preview>,
    confirm_delete: Option<usize>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Light);
        let settings_path = Settings::default_path();
        let (settings, settings_error) = match Settings::load(&settings_path) {
            Ok(s) => (s, None),
            Err(e) => (
                Settings::default(),
                Some(format!("Could not read settings ({e}); using defaults.")),
            ),
        };
        let state_path = AppState::default_path();
        let state = AppState::load(&state_path);
        let cache = Cache::new(settings.cache_dir());
        let (tx, rx) = channel();
        let mut app = Self {
            current_env: None,
            settings,
            settings_path,
            state,
            state_path,
            state_dirty: None,
            cache,
            loaded: None,
            tokens: HashMap::new(),
            search: String::new(),
            selected_only: false,
            filtered: Vec::new(),
            filter_key: None,
            busy: None,
            last_result: settings_error.map(Err),
            tx,
            rx,
            env_editor: None,
            settings_editor: None,
            preview: None,
            confirm_delete: None,
        };
        let last = app.state.last_environment.clone();
        let index = app
            .settings
            .environments
            .iter()
            .position(|e| Some(&e.url) == last.as_ref())
            .or((!app.settings.environments.is_empty()).then_some(0));
        if let Some(i) = index {
            app.select_environment(i, &cc.egui_ctx);
        }
        app
    }

    // ---------- jobs ----------

    fn start(
        &mut self,
        label: &str,
        ctx: &egui::Context,
        job: impl FnOnce(&jobs::Ctx) -> Result<String, String> + Send + 'static,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.busy = Some(Busy {
            status: format!("{label}…"),
            progress: None,
            sign_in_url: None,
            cancel: cancel.clone(),
            started: Instant::now(),
        });
        self.last_result = None;
        jobs::spawn(
            jobs::Ctx {
                tx: self.tx.clone(),
                egui: ctx.clone(),
                cancel,
            },
            job,
        );
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Event(Event::Status(s)) => {
                    if let Some(b) = &mut self.busy {
                        b.status = s;
                    }
                }
                Msg::Event(Event::Progress(p)) => {
                    if let Some(b) = &mut self.busy {
                        b.progress = Some(p);
                        b.sign_in_url = None;
                        b.status = match p {
                            Progress::Connecting => {
                                "Waiting for the server to send the metadata…".into()
                            }
                            Progress::Downloading { .. } => "Downloading metadata…".into(),
                            Progress::Done { .. } => "Download complete.".into(),
                        };
                    }
                }
                Msg::Event(Event::SignInUrl(url)) => {
                    if let Some(b) = &mut self.busy {
                        b.sign_in_url = Some(url);
                    }
                }
                Msg::Event(Event::Token { env_url, token }) => {
                    self.tokens.insert(env_url, token);
                }
                Msg::Event(Event::SignedOut { env_url }) => {
                    self.tokens.remove(&env_url);
                }
                Msg::Event(Event::Loaded { env_url, metadata }) => {
                    let rows = entity_rows(&metadata);
                    // Drop selections that no longer exist in the metadata.
                    if let Some(sel) = self.state.selections.get_mut(&env_url) {
                        let names: BTreeSet<&str> =
                            rows.iter().map(|r| r.set_name.as_str()).collect();
                        sel.retain(|s| names.contains(s.as_str()));
                    }
                    self.loaded = Some(Loaded {
                        env_url,
                        metadata,
                        rows,
                    });
                    self.filter_key = None;
                }
                Msg::Finished(r) => {
                    self.busy = None;
                    self.last_result = Some(r);
                }
            }
        }
    }

    fn current_environment(&self) -> Option<&Environment> {
        self.current_env
            .and_then(|i| self.settings.environments.get(i))
    }

    fn select_environment(&mut self, index: usize, ctx: &egui::Context) {
        if self.busy.is_some() {
            return;
        }
        self.current_env = Some(index);
        self.loaded = None;
        self.filter_key = None;
        let Some(env) = self.settings.environments.get(index).cloned() else {
            return;
        };
        self.state.last_environment = Some(env.url.clone());
        self.mark_dirty();
        if self.cache.entry(&env.url).is_some() {
            let cache = self.cache.clone();
            let url = env.url.clone();
            self.start("Loading", ctx, move |c| {
                pipeline::load_cached(&|e| c.emit(e), &cache, &url)
            });
        } else {
            self.last_result = None;
        }
    }

    fn start_download(&mut self, ctx: &egui::Context) {
        let Some(env) = self.current_environment().cloned() else {
            return;
        };
        let token = self.tokens.get(&env.url).cloned();
        let cache = self.cache.clone();
        let label = if self.cache.entry(&env.url).is_some() {
            "Checking for updates"
        } else {
            "Downloading"
        };
        self.start(label, ctx, move |c| {
            pipeline::download(&|e| c.emit(e), &c.cancel, &cache, &env, token)
        });
    }

    fn base_options(&self) -> Options {
        pipeline::base_options(self.current_environment(), &self.state.output)
    }

    fn selected(&self) -> Vec<String> {
        let Some(l) = &self.loaded else {
            return Vec::new();
        };
        self.state
            .selections
            .get(&l.env_url)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.loaded else { return };
        let Some(dir) = self.state.output.output_dir.clone() else {
            self.last_result = Some(Err("Choose an output folder first.".into()));
            return;
        };
        let job = ExportJob {
            metadata: l.metadata.clone(),
            entity_sets: self.selected(),
            files: self.state.output.planned_files(&dir),
            base_options: self.base_options(),
        };
        self.start("Exporting", ctx, move |c| {
            pipeline::export(&|e| c.emit(e), job)
        });
    }

    fn show_preview(&mut self) {
        let Some(l) = &self.loaded else { return };
        let o = &self.state.output;
        let (Some(version), Some(format)) =
            (o.versions().first().copied(), o.formats().first().copied())
        else {
            return;
        };
        let opts = Options {
            version,
            ..self.base_options()
        };
        match pipeline::preview(&l.metadata, &self.selected(), &opts, format, 400_000) {
            Ok(text) => {
                self.preview = Some(Preview {
                    title: format!(
                        "Preview: {} {}",
                        version.label(),
                        format.extension().to_uppercase()
                    ),
                    text,
                });
            }
            Err(e) => self.last_result = Some(Err(e)),
        }
    }

    fn mark_dirty(&mut self) {
        self.state_dirty.get_or_insert_with(Instant::now);
    }

    fn save_state_if_due(&mut self) {
        if self
            .state_dirty
            .is_some_and(|t| t.elapsed() > Duration::from_millis(500))
        {
            if let Err(e) = self.state.save(&self.state_path) {
                self.last_result = Some(Err(format!("Could not save app state: {e}")));
            }
            self.state_dirty = None;
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.last_result = Some(Err(format!("Could not save settings: {e}")));
        }
    }

    // ---------- panels ----------

    fn environments_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Environments");
        ui.add_space(4.0);
        let busy = self.busy.is_some();
        let mut clicked = None;
        for (i, env) in self.settings.environments.iter().enumerate() {
            let selected = self.current_env == Some(i);
            let r = ui.add_enabled(
                !busy || selected,
                egui::Button::selectable(selected, &env.name)
                    .min_size(egui::vec2(ui.available_width(), 0.0)),
            );
            if r.on_hover_text(&env.url).clicked() && !selected {
                clicked = Some(i);
            }
        }
        if let Some(i) = clicked {
            self.select_environment(i, ui.ctx());
        }
        if self.settings.environments.is_empty() {
            ui.label(RichText::new("Add your Dynamics 365 environment (Finance & Operations or Dataverse) to get started.").weak());
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("➕ Add")).clicked() {
                self.env_editor = Some(EnvEditor {
                    index: None,
                    env: Environment {
                        name: String::new(),
                        url: String::new(),
                        tenant: String::new(),
                        client_id: String::new(),
                    },
                    error: None,
                });
            }
            if let Some(i) = self.current_env {
                if ui.add_enabled(!busy, egui::Button::new("✏ Edit")).clicked() {
                    self.env_editor = Some(EnvEditor {
                        index: Some(i),
                        env: self.settings.environments[i].clone(),
                        error: None,
                    });
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("🗑 Remove"))
                    .clicked()
                {
                    self.confirm_delete = Some(i);
                }
            }
        });

        let Some(env) = self.current_environment().cloned() else {
            return;
        };
        ui.separator();
        ui.label(RichText::new(&env.url).small().weak());
        ui.add_space(6.0);
        ui.strong("Saved metadata");
        match self.cache.entry(&env.url) {
            Some(e) => {
                egui::Grid::new("cache_info").num_columns(2).show(ui, |ui| {
                    ui.label("Downloaded");
                    ui.label(format_time(e.downloaded_at));
                    ui.end_row();
                    ui.label("Last checked");
                    ui.label(format_time(e.checked_at));
                    ui.end_row();
                    ui.label("Size");
                    ui.label(format_bytes(e.size));
                    ui.end_row();
                    if let Some(l) = self.loaded.as_ref().filter(|l| l.env_url == env.url) {
                        ui.label("Entities");
                        ui.label(l.rows.len().to_string());
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                if ui
                    .add_enabled(!busy, egui::Button::new("🔄 Check for updates"))
                    .on_hover_text(
                        "Download again and replace the saved copy only if the metadata has changed",
                    )
                    .clicked()
                {
                    self.start_download(ui.ctx());
                }
            }
            None => {
                ui.label(RichText::new("Not downloaded yet.").weak());
                ui.add_space(6.0);
                if ui
                    .add_enabled(!busy, egui::Button::new("⬇ Sign in and download"))
                    .clicked()
                {
                    self.start_download(ui.ctx());
                }
            }
        }
        if self.tokens.get(&env.url).is_some_and(|t| !t.is_expired()) {
            ui.label(
                RichText::new("✔ Signed in")
                    .small()
                    .color(Color32::from_rgb(60, 160, 90)),
            );
        }
    }

    fn entities_panel(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.loaded else {
            ui.centered_and_justified(|ui| {
                let text = if self.settings.environments.is_empty() {
                    "Add an environment on the left."
                } else if self.busy.is_some() {
                    "Working…"
                } else {
                    "Download the metadata to see its entities."
                };
                ui.label(RichText::new(text).weak());
            });
            return;
        };
        let env_url = loaded.env_url.clone();
        let total = loaded.rows.len();

        ui.horizontal(|ui| {
            ui.heading("Entities");
            let n = self.state.selections.get(&env_url).map_or(0, BTreeSet::len);
            ui.label(RichText::new(format!("{n} of {total} selected")).weak());
        });
        ui.horizontal(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("🔍 Search entities, e.g. sales order")
                    .desired_width(f32::INFINITY),
            );
            if search.changed() {
                self.filter_key = None;
            }
        });

        let selected_count = self.state.selections.get(&env_url).map_or(0, BTreeSet::len);
        let key = (self.search.clone(), self.selected_only, selected_count);
        if self.filter_key.as_ref() != Some(&key) {
            let empty = BTreeSet::new();
            let sel = self.state.selections.get(&env_url).unwrap_or(&empty);
            self.filtered = filter_rows(&loaded.rows, &self.search, sel, self.selected_only);
            self.filter_key = Some(key);
        }

        ui.horizontal(|ui| {
            if ui
                .button(format!("Select all {} shown", self.filtered.len()))
                .clicked()
            {
                let sel = self.state.selections.entry(env_url.clone()).or_default();
                sel.extend(
                    self.filtered
                        .iter()
                        .map(|&i| loaded.rows[i].set_name.clone()),
                );
                self.state_dirty.get_or_insert_with(Instant::now);
            }
            if ui.button("Clear shown").clicked() {
                if let Some(sel) = self.state.selections.get_mut(&env_url) {
                    for &i in &self.filtered {
                        sel.remove(&loaded.rows[i].set_name);
                    }
                }
                self.state_dirty.get_or_insert_with(Instant::now);
            }
            if ui
                .toggle_value(&mut self.selected_only, "Selected only")
                .changed()
            {
                self.filter_key = None;
            }
        });
        ui.separator();

        let row_height = ui.spacing().interact_size.y;
        let sel = self.state.selections.entry(env_url).or_default();
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            row_height,
            self.filtered.len(),
            |ui, range| {
                for &i in &self.filtered[range] {
                    let row = &loaded.rows[i];
                    let mut on = sel.contains(&row.set_name);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, &row.set_name).changed() {
                            changed = true;
                            if on {
                                sel.insert(row.set_name.clone());
                            } else {
                                sel.remove(&row.set_name);
                            }
                        }
                        if row.type_name != row.set_name {
                            ui.label(RichText::new(&row.type_name).weak().small());
                        }
                    });
                }
            },
        );
        if changed {
            self.mark_dirty();
        }
        if self.filtered.is_empty() {
            ui.label(RichText::new("No entities match.").weak());
        }
    }

    fn output_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Output");
        ui.add_space(4.0);
        let before = self.state.output.clone();
        let o = &mut self.state.output;
        ui.strong("Formats");
        ui.checkbox(&mut o.openapi30, SpecVersion::OpenApi30.label());
        ui.checkbox(&mut o.openapi31, SpecVersion::OpenApi31.label());
        ui.checkbox(&mut o.swagger2, SpecVersion::Swagger2.label());
        ui.horizontal(|ui| {
            ui.checkbox(&mut o.json, "JSON");
            ui.checkbox(&mut o.yaml, "YAML");
        });
        ui.add_space(6.0);
        ui.strong("Include");
        ui.checkbox(&mut o.include_operations, "Actions and functions");
        ui.checkbox(&mut o.include_navigation, "Navigation properties")
            .on_hover_text("Only links between entities that are both selected");
        ui.checkbox(&mut o.include_server_url, "Environment URL as server");
        ui.add_space(6.0);
        ui.strong("Save to");
        ui.horizontal(|ui| {
            let dir = o
                .output_dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "No folder chosen".into());
            ui.add(egui::Label::new(RichText::new(dir).small()).truncate());
        });
        if ui.button("📁 Choose folder…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            o.output_dir = Some(dir);
        }
        ui.horizontal(|ui| {
            ui.label("File name");
            ui.text_edit_singleline(&mut o.file_name);
        });
        if *o != before {
            self.mark_dirty();
        }

        let o = &self.state.output;
        let files = o
            .output_dir
            .as_ref()
            .map(|d| o.planned_files(d))
            .unwrap_or_default();
        let selected = self.selected().len();
        ui.add_space(8.0);
        let problem = if self.loaded.is_none() {
            Some("Load metadata first.")
        } else if selected == 0 {
            Some("Select at least one entity.")
        } else if o.versions().is_empty() || o.formats().is_empty() {
            Some("Pick at least one format and JSON or YAML.")
        } else {
            None
        };
        let busy = self.busy.is_some();
        let export_ok = problem.is_none() && !busy && o.output_dir.is_some();
        let (mut preview, mut export) = (false, false);
        ui.horizontal(|ui| {
            preview = ui
                .add_enabled(problem.is_none() && !busy, egui::Button::new("👁 Preview"))
                .clicked();
            export = ui
                .add_enabled(
                    export_ok,
                    egui::Button::new(
                        RichText::new(format!("💾 Export {} file(s)", files.len())).strong(),
                    ),
                )
                .clicked();
        });
        if preview {
            self.show_preview();
        }
        if export {
            self.start_export(ui.ctx());
        }
        if let Some(p) = problem {
            ui.label(RichText::new(p).weak().small());
        } else if !files.is_empty() {
            for (_, _, f) in &files {
                ui.label(
                    RichText::new(f.file_name().unwrap_or_default().to_string_lossy())
                        .small()
                        .weak(),
                );
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if let Some(b) = &self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&b.status);
                ui.label(RichText::new(format!("{}s", b.started.elapsed().as_secs())).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        b.cancel.store(true, Ordering::Relaxed);
                    }
                    if let Some(url) = &b.sign_in_url
                        && ui
                            .button("Copy sign-in link")
                            .on_hover_text("Use this if the browser did not open")
                            .clicked()
                    {
                        ui.ctx().copy_text(url.clone());
                    }
                });
            });
            match b.progress {
                Some(Progress::Downloading {
                    received,
                    total: Some(total),
                }) if total > 0 => {
                    let f = received as f32 / total as f32;
                    ui.add(egui::ProgressBar::new(f).show_percentage().text(format!(
                        "{} of {}",
                        format_bytes(received),
                        format_bytes(total)
                    )));
                }
                Some(Progress::Downloading { received, total: _ }) => {
                    ui.add(
                        egui::ProgressBar::new(0.0)
                            .animate(true)
                            .text(format!("{} received", format_bytes(received))),
                    );
                }
                Some(Progress::Connecting) => {
                    ui.add(
                        egui::ProgressBar::new(0.0)
                            .animate(true)
                            .text("Connecting…"),
                    );
                }
                _ => {}
            }
            // Keep the elapsed timer ticking.
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else {
            match &self.last_result {
                Some(Ok(m)) => {
                    ui.label(RichText::new(format!("✔ {m}")).color(Color32::from_rgb(60, 160, 90)))
                }
                Some(Err(m)) => {
                    ui.label(RichText::new(format!("⚠ {m}")).color(Color32::from_rgb(210, 80, 70)))
                }
                None => ui.label(RichText::new("Ready").weak()),
            };
        }
        ui.add_space(4.0);
    }

    // ---------- windows ----------

    fn env_editor_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = &mut self.env_editor else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(if ed.index.is_some() { "Edit environment" } else { "Add environment" })
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("env_form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut ed.env.name).hint_text("e.g. UAT").desired_width(340.0));
                    ui.end_row();
                    ui.label("Environment URL");
                    ui.add(egui::TextEdit::singleline(&mut ed.env.url).hint_text("https://contoso.operations.dynamics.com or https://contoso.crm.dynamics.com").desired_width(340.0));
                    ui.end_row();
                    ui.label("Tenant ID");
                    ui.add(egui::TextEdit::singleline(&mut ed.env.tenant).hint_text("Optional: tenant ID or contoso.onmicrosoft.com").desired_width(340.0));
                    ui.end_row();
                    ui.label("Client ID");
                    ui.add(egui::TextEdit::singleline(&mut ed.env.client_id).hint_text("Optional: client ID of your app registration").desired_width(340.0));
                    ui.end_row();
                });
                ui.label(RichText::new("Leave Tenant and Client ID empty to sign in without an app registration (if your tenant allows it). If sign-in is blocked, use your own app registration with redirect URI http://localhost and the Dynamics ERP Odata.FullAccess permission. See the README.").small().weak());
                if let Some(e) = &ed.error {
                    ui.colored_label(Color32::from_rgb(210, 80, 70), e);
                }
                ui.horizontal(|ui| {
                    save = ui.button("Save").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if save {
            match pipeline::validate_environment(&ed.env) {
                Ok(env) => {
                    let index = ed.index;
                    let new_index = match index {
                        Some(i) => {
                            self.settings.environments[i] = env;
                            i
                        }
                        None => {
                            self.settings.environments.push(env);
                            self.settings.environments.len() - 1
                        }
                    };
                    self.env_editor = None;
                    self.save_settings();
                    self.select_environment(new_index, ctx);
                    return;
                }
                Err(e) => ed.error = Some(e),
            }
        }
        if !open || cancel {
            self.env_editor = None;
        }
    }

    fn confirm_delete_window(&mut self, ctx: &egui::Context) {
        let Some(i) = self.confirm_delete else { return };
        let Some(env) = self.settings.environments.get(i).cloned() else {
            self.confirm_delete = None;
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("confirm_delete")).show(ctx, |ui| {
            ui.heading(format!("Remove {}?", env.name));
            ui.label("Its saved metadata and entity selection are deleted too.");
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                self.settings.environments.remove(i);
                let _ = self.cache.remove(&env.url);
                self.state.selections.remove(&env.url);
                self.tokens.remove(&env.url);
                self.mark_dirty();
                self.save_settings();
                self.confirm_delete = None;
                self.current_env = None;
                self.loaded = None;
                if !self.settings.environments.is_empty() {
                    self.select_environment(0, ctx);
                }
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = &mut self.settings_editor else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        egui::Window::new("Settings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.strong("Metadata folder");
            ui.label(RichText::new("Where downloaded metadata is saved. Existing files are moved when you change it.").small().weak());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut ed.cache_dir).desired_width(380.0));
                if ui.button("📁 Browse…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(&ed.cache_dir).pick_folder() {
                        ed.cache_dir = dir.display().to_string();
                    }
            });
            if ui.small_button("Reset to default").clicked() {
                ed.cache_dir = Settings::default_cache_dir().display().to_string();
            }
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Settings file: {}", self.settings_path.display())).small().weak());
            ui.label(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).small().weak());
            if let Some(e) = &ed.error {
                ui.colored_label(Color32::from_rgb(210, 80, 70), e);
            }
            ui.add_space(4.0);
            apply = ui.button("Save").clicked();
        });
        if apply {
            let new_dir = PathBuf::from(ed.cache_dir.trim());
            if new_dir.as_os_str().is_empty() {
                ed.error = Some("Enter a folder.".into());
                return;
            }
            match self.cache.move_to(&new_dir) {
                Ok(cache) => {
                    self.cache = cache;
                    self.settings.cache_dir =
                        (new_dir != Settings::default_cache_dir()).then_some(new_dir);
                    self.save_settings();
                    self.settings_editor = None;
                    self.last_result = Some(Ok(format!(
                        "Metadata folder is now {}",
                        self.cache.dir().display()
                    )));
                }
                Err(e) => ed.error = Some(format!("Could not move the saved metadata: {e}")),
            }
        }
        if !open {
            self.settings_editor = None;
        }
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.preview else { return };
        let mut open = true;
        egui::Window::new(&p.title)
            .open(&mut open)
            .default_size([720.0, 560.0])
            .show(ctx, |ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(p.text.clone());
                }
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut p.text.as_str())
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        if !open {
            self.preview = None;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_messages();
        let ctx = ui.ctx().clone();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("D365 → OpenAPI");
                ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(self.busy.is_none(), egui::Button::new("⚙ Settings"))
                        .clicked()
                    {
                        self.settings_editor = Some(SettingsEditor {
                            cache_dir: self.cache.dir().display().to_string(),
                            error: None,
                        });
                    }
                });
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("environments")
            .default_size(250.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.environments_panel(ui));
            });
        egui::Panel::right("output")
            .default_size(270.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.output_panel(ui));
            });
        egui::CentralPanel::default_margins().show(ui, |ui| self.entities_panel(ui));

        self.env_editor_window(&ctx);
        self.confirm_delete_window(&ctx);
        self.settings_window(&ctx);
        self.preview_window(&ctx);
        self.save_state_if_due();
        if self.state_dirty.is_some() {
            ctx.request_repaint_after(Duration::from_millis(600));
        }
    }
}
