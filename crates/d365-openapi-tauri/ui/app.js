const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const state = {
  environments: [],
  current: null, // environment URL
  rows: [], // [{ set, type }] for the current environment
  selected: new Set(),
  filtered: [],
  output: null,
  busy: false,
  signInUrl: null,
  startedAt: 0,
  timer: null,
  loading: false,
};

// ---------- status bar ----------

function setStatus(text, kind = "muted") {
  const el = $("status-text");
  el.textContent = text;
  el.className = kind;
}

function setBusy(busy, label) {
  state.busy = busy;
  $("spinner").hidden = !busy;
  $("cancel").hidden = !busy;
  $("progress-wrap").hidden = true;
  $("copy-link").hidden = true;
  clearInterval(state.timer);
  $("elapsed").textContent = "";
  if (busy) {
    state.startedAt = Date.now();
    setStatus(label + "…");
    state.timer = setInterval(() => {
      $("elapsed").textContent = Math.floor((Date.now() - state.startedAt) / 1000) + "s";
    }, 250);
  }
  renderEnvironments();
  renderOutputState();
}

function showProgress(received, total) {
  $("progress-wrap").hidden = false;
  const bar = $("progress-bar");
  if (total) {
    bar.classList.remove("indeterminate");
    bar.style.width = Math.min(100, (received / total) * 100).toFixed(1) + "%";
  } else {
    bar.classList.add("indeterminate");
    bar.style.width = "";
  }
}

listen("job", ({ payload: e }) => {
  switch (e.kind) {
    case "started":
      setBusy(true, e.label);
      break;
    case "status":
      setStatus(e.text);
      break;
    case "progress":
      state.signInUrl = null;
      $("copy-link").hidden = true;
      setStatus(e.text);
      showProgress(e.received, e.total);
      break;
    case "signInUrl":
      state.signInUrl = e.url;
      $("copy-link").hidden = false;
      break;
    case "finished":
      setBusy(false);
      state.loading = false;
      setStatus((e.ok ? "✔ " : "⚠ ") + e.text, e.ok ? "ok" : "error");
      refreshEnvironmentInfo();
      renderEntities();
      break;
  }
});

listen("loaded", ({ payload }) => {
  if (payload.envUrl !== state.current) return;
  state.rows = payload.rows.sort((a, b) => a.set.toLowerCase().localeCompare(b.set.toLowerCase()));
  state.selected = new Set(payload.selected);
  renderEntities();
});

$("cancel").onclick = () => invoke("cancel");
$("copy-link").onclick = () => state.signInUrl && navigator.clipboard.writeText(state.signInUrl);

async function run(promise) {
  try {
    return await promise;
  } catch (err) {
    setStatus("⚠ " + err, "error");
    throw err;
  }
}

// ---------- environments ----------

function currentEnv() {
  return state.environments.find((e) => e.url === state.current) || null;
}

function renderEnvironments() {
  const list = $("env-list");
  list.replaceChildren();
  for (const env of state.environments) {
    const li = document.createElement("li");
    li.textContent = env.name;
    li.title = env.url;
    if (env.url === state.current) li.classList.add("selected");
    else if (state.busy) li.classList.add("disabled");
    li.onclick = () => env.url !== state.current && selectEnvironment(env.url);
    list.append(li);
  }
  $("env-empty").hidden = state.environments.length > 0;
  $("env-add").disabled = state.busy;
  $("env-edit").disabled = state.busy || !state.current;
  $("env-remove").disabled = state.busy || !state.current;
  $("download").disabled = state.busy;
  $("open-settings").disabled = state.busy;
}

async function refreshEnvironmentInfo() {
  const env = currentEnv();
  $("env-details").hidden = !env;
  if (!env) return;
  $("env-url").textContent = env.url;
  const info = await invoke("environment_info", { url: env.url });
  const dl = $("cache-info");
  dl.replaceChildren();
  const add = (k, v) => {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    dl.append(dt, dd);
  };
  if (info.downloadedAt) {
    add("Downloaded", info.downloadedAt);
    add("Last checked", info.checkedAt);
    add("Size", info.size);
    if (state.rows.length) add("Entities", state.rows.length);
  }
  dl.hidden = !info.downloadedAt;
  $("cache-none").hidden = !!info.downloadedAt;
  $("download").textContent = info.downloadedAt ? "🔄 Check for updates" : "⬇ Sign in and download";
  $("download").title = info.downloadedAt ? "Download again and replace the saved copy only if the metadata has changed" : "";
  $("signed-in").hidden = !info.signedIn;
}

async function selectEnvironment(url) {
  if (state.busy) return;
  state.current = url;
  state.rows = [];
  state.selected = new Set();
  renderEnvironments();
  state.loading = true;
  renderEntities();
  await refreshEnvironmentInfo();
  const hasSaved = await run(invoke("load_environment", { url }));
  if (!hasSaved) {
    state.loading = false;
    setStatus("Ready");
    renderEntities();
  }
}

$("download").onclick = () => state.current && run(invoke("download", { url: state.current }));

let editingIndex = null;
function openEnvDialog(index) {
  editingIndex = index;
  const env = index == null ? { name: "", url: "", tenant: "", client_id: "" } : state.environments[index];
  const form = $("env-form");
  for (const k of ["name", "url", "tenant", "client_id"]) form.elements[k].value = env[k];
  $("env-dialog-title").textContent = index == null ? "Add environment" : "Edit environment";
  $("env-error").textContent = "";
  $("env-dialog").showModal();
}
$("env-add").onclick = () => openEnvDialog(null);
$("env-edit").onclick = () => openEnvDialog(state.environments.findIndex((e) => e.url === state.current));
$("env-form").addEventListener("submit", async (ev) => {
  if (ev.submitter?.value !== "save") return;
  ev.preventDefault();
  const f = ev.target.elements;
  const env = { name: f.name.value, url: f.url.value, tenant: f.tenant.value, client_id: f.client_id.value };
  try {
    const saved = await invoke("save_environment", { index: editingIndex, env });
    if (editingIndex == null) state.environments.push(saved);
    else state.environments[editingIndex] = saved;
    $("env-dialog").close();
    state.current = null;
    await selectEnvironment(saved.url);
  } catch (err) {
    $("env-error").textContent = err;
  }
});

$("env-remove").onclick = () => {
  const env = currentEnv();
  if (!env) return;
  $("confirm-title").textContent = `Remove ${env.name}?`;
  $("confirm-dialog").showModal();
};
$("confirm-dialog").addEventListener("close", async () => {
  if ($("confirm-dialog").returnValue !== "yes") return;
  const url = state.current;
  await run(invoke("remove_environment", { url }));
  state.environments = state.environments.filter((e) => e.url !== url);
  state.current = null;
  state.rows = [];
  renderEnvironments();
  renderEntities();
  await refreshEnvironmentInfo();
  if (state.environments.length) selectEnvironment(state.environments[0].url);
});

// ---------- entities ----------

function applyFilter() {
  const words = $("search").value.toLowerCase().split(/\s+/).filter(Boolean);
  const selectedOnly = $("selected-only").checked;
  state.filtered = state.rows.filter((r) => {
    if (selectedOnly && !state.selected.has(r.set)) return false;
    const hay = (r.set + " " + r.type).toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

function renderEntities() {
  const hasRows = state.rows.length > 0;
  $("entities").hidden = !hasRows;
  $("entities-empty").hidden = hasRows;
  if (!hasRows) {
    $("entities-empty").textContent = !state.environments.length
      ? "Add an environment on the left."
      : state.loading || state.busy
        ? "Working…"
        : "Download the metadata to see its entities.";
    renderOutputState();
    return;
  }
  applyFilter();
  const list = $("entity-list");
  const frag = document.createDocumentFragment();
  for (const r of state.filtered) {
    const li = document.createElement("li");
    const label = document.createElement("label");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = state.selected.has(r.set);
    cb.dataset.set = r.set;
    const name = document.createElement("span");
    name.textContent = r.set;
    label.append(cb, name);
    if (r.type !== r.set) {
      const t = document.createElement("span");
      t.className = "type";
      t.textContent = r.type;
      label.append(t);
    }
    li.append(label);
    frag.append(li);
  }
  list.replaceChildren(frag);
  $("no-match").hidden = state.filtered.length > 0;
  $("select-shown").textContent = `Select all ${state.filtered.length} shown`;
  updateCount();
}

function updateCount() {
  $("sel-count").textContent = `${state.selected.size} of ${state.rows.length} selected`;
  renderOutputState();
}

let saveSelectionTimer = null;
function saveSelection() {
  clearTimeout(saveSelectionTimer);
  const url = state.current;
  const selected = [...state.selected];
  saveSelectionTimer = setTimeout(() => run(invoke("set_selection", { url, selected })), 300);
}

$("entity-list").addEventListener("change", (ev) => {
  const set = ev.target.dataset.set;
  if (!set) return;
  if (ev.target.checked) state.selected.add(set);
  else state.selected.delete(set);
  updateCount();
  saveSelection();
});
$("search").addEventListener("input", renderEntities);
$("selected-only").addEventListener("change", renderEntities);
$("select-shown").onclick = () => {
  state.filtered.forEach((r) => state.selected.add(r.set));
  renderEntities();
  saveSelection();
};
$("clear-shown").onclick = () => {
  state.filtered.forEach((r) => state.selected.delete(r.set));
  renderEntities();
  saveSelection();
};

// ---------- output ----------

async function saveOutput() {
  const names = await run(invoke("set_output", { output: state.output }));
  renderOutputState(names);
}

let plannedNames = [];
function renderOutputState(names) {
  if (names) plannedNames = names;
  const o = state.output;
  if (!o) return;
  $("out-dir").textContent = o.output_dir || "No folder chosen";
  const anyVersion = o.openapi30 || o.openapi31 || o.swagger2;
  const anyFormat = o.json || o.yaml;
  const problem = !state.rows.length
    ? "Load metadata first."
    : !state.selected.size
      ? "Select at least one entity."
      : !anyVersion || !anyFormat
        ? "Pick at least one format and JSON or YAML."
        : "";
  $("out-problem").textContent = problem || (o.output_dir ? "" : "Choose an output folder to export.");
  $("preview").disabled = !!problem || state.busy;
  $("export").disabled = !!problem || state.busy || !o.output_dir;
  $("export").textContent = `Export ${plannedNames.length} file(s)`;
  const list = $("out-files");
  list.replaceChildren(
    ...(problem ? [] : plannedNames).map((n) => {
      const li = document.createElement("li");
      li.textContent = n;
      return li;
    }),
  );
}

document.querySelectorAll("[data-opt]").forEach((cb) => {
  cb.addEventListener("change", () => {
    state.output[cb.dataset.opt] = cb.checked;
    saveOutput();
  });
});
$("file-name").addEventListener("change", (ev) => {
  state.output.file_name = ev.target.value;
  saveOutput();
});
$("choose-dir").onclick = async () => {
  const dir = await invoke("pick_folder", { start: state.output.output_dir });
  if (dir) {
    state.output.output_dir = dir;
    saveOutput();
  }
};
$("export").onclick = () => run(invoke("export", { url: state.current }));
$("preview").onclick = async () => {
  const o = state.output;
  const version = o.openapi30 ? "OpenAPI 3.0" : o.openapi31 ? "OpenAPI 3.1" : "Swagger 2.0";
  const text = await run(invoke("preview", { url: state.current }));
  $("preview-title").textContent = `Preview: ${version} ${o.json ? "JSON" : "YAML"}`;
  $("preview-text").textContent = text;
  $("preview-dialog").showModal();
};
$("copy-preview").onclick = () => navigator.clipboard.writeText($("preview-text").textContent);
$("close-preview").onclick = () => $("preview-dialog").close();

// ---------- settings ----------

let defaultCacheDir = "";
$("open-settings").onclick = async () => {
  const init = await invoke("init");
  $("cache-dir").value = init.cacheDir;
  $("settings-path").textContent = `Settings file: ${init.settingsPath} · Version ${init.version}`;
  $("settings-error").textContent = "";
  $("settings-dialog").showModal();
};
$("browse-cache").onclick = async () => {
  const dir = await invoke("pick_folder", { start: $("cache-dir").value });
  if (dir) $("cache-dir").value = dir;
};
$("reset-cache").onclick = () => ($("cache-dir").value = defaultCacheDir);
$("settings-form").addEventListener("submit", async (ev) => {
  if (ev.submitter?.value !== "save") return;
  ev.preventDefault();
  try {
    const dir = await invoke("set_cache_dir", { dir: $("cache-dir").value });
    $("settings-dialog").close();
    setStatus("✔ Metadata folder is now " + dir, "ok");
    refreshEnvironmentInfo();
  } catch (err) {
    $("settings-error").textContent = err;
  }
});

// ---------- start ----------

(async () => {
  const init = await invoke("init");
  state.environments = init.environments;
  state.output = init.output;
  $("version").textContent = "v" + init.version;
  defaultCacheDir = init.defaultCacheDir;
  document.querySelectorAll("[data-opt]").forEach((cb) => (cb.checked = !!state.output[cb.dataset.opt]));
  $("file-name").value = state.output.file_name;
  renderOutputState(await invoke("planned_files"));
  renderEnvironments();
  renderEntities();
  const start = state.environments.find((e) => e.url === init.lastEnvironment) || state.environments[0];
  if (start) selectEnvironment(start.url);
})();
