# D365FO OData metadata to OpenAPI

Desktop tool (Rust + egui) that converts the OData `$metadata` of a Dynamics 365
Finance & Operations environment into OpenAPI 3.0, OpenAPI 3.1 or Swagger 2.0
specifications, for only the entities you choose.

## Status

- [x] Converter core (`crates/d365-openapi-core`)
- [x] Fetch metadata with Azure AD browser sign-in, local cache with configurable location (`crates/d365-fetch`)
- [x] egui desktop app (`crates/d365-openapi-app`)
- [ ] Verified imports (Swagger UI, Azure API Management, codegen) and installers

## Desktop app

![Main window](docs/screenshot-main.png)

```sh
cargo run --release -p d365-openapi-app
```

1. **Add** an environment: URL, tenant ID and the client ID of your app
   registration (see *One-time Azure AD setup* below).
2. **Sign in and download.** Your browser opens for sign-in, and the status bar
   shows progress while the metadata downloads. The metadata is saved, so next
   time the app opens it straight from disk. **Check for updates** downloads
   again and only replaces the saved copy if D365FO changed.
3. **Search** and tick the entities you want. Your selection is remembered per
   environment.
4. Pick **formats** (OpenAPI 3.0 / 3.1 / Swagger 2.0, JSON / YAML), an output
   folder and a file name, then **Preview** or **Export**. You get one file per
   format, e.g. `contoso-uat.openapi-3.0.json`.

**Settings** lets you change where metadata is saved; existing files are moved.

The app draws with OpenGL. To try the wgpu renderer instead (DirectX 12 /
Vulkan / Metal), start it with `D365_OPENAPI_RENDERER=wgpu`.

![Sign-in in progress](docs/screenshot-sign-in.png)

## Tauri version

The same app is also available with a web-based UI built on Tauri
(`crates/d365-openapi-tauri`). It shares all logic with the egui app; only the
UI differs. On Windows it uses the built-in WebView2.

![Tauri version](docs/screenshot-tauri.png)

```sh
cargo run --release -p d365-openapi-tauri
```

To build installers (MSI and NSIS) on Windows, install the Tauri CLI
(`cargo install tauri-cli --version "^2"`) and run `cargo tauri build` in
`crates/d365-openapi-tauri`.

Linux needs the WebKitGTK development packages to build
(`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`).

## Converter core

Parses `$metadata` (entity types, keys, properties, enums, complex types,
navigation properties, bound actions/functions) and generates for each selected
entity set:

- `GET/POST /data/{EntitySet}` with `$filter`, `$select`, `$expand`, `$orderby`,
  `$top`, `$skip`, `$count` and `cross-company`
- `GET/PATCH/DELETE /data/{EntitySet}(key1='{key1}',key2='{key2}')`
- Bound actions and functions
- Schemas for the entity types plus every enum and complex type they use.
  Navigation properties are kept only when their target is also selected.

Try it on a saved metadata file:

```sh
cargo run --example convert -- metadata.xml openapi-3.0 yaml CustomersV3 SalesOrderHeadersV2
```

Versions: `swagger-2.0`, `openapi-3.0`, `openapi-3.1`. Formats: `json`, `yaml`.
Leave out the entity sets to include all of them.

```sh
cargo test
```

## Fetching metadata (`crates/d365-fetch`)

- **Sign-in:** interactive browser sign-in (OAuth 2.0 authorization code + PKCE,
  loopback redirect). Nothing is stored on disk; tokens live in memory.
- **Download:** `GET {environment}/data/$metadata` with progress events
  (connecting, bytes received of total, done) and cancellation.
- **Cache:** one `<host>.xml` plus `<host>.json` (hash, size, dates) per
  environment. Re-downloading identical metadata leaves the file alone and
  reports "unchanged". The folder can be moved; files move with it.
- **Settings:** environments and cache folder in `settings.json` under the OS
  config folder (`%APPDATA%\D365 OpenAPI\config` on Windows).

### One-time Azure AD setup

Sign-in needs an app registration in your tenant:

1. Azure portal → **App registrations** → **New registration**. Any name,
   single tenant.
2. **Authentication** → **Add a platform** → **Mobile and desktop
   applications** → custom redirect URI `http://localhost`.
   Set **Allow public client flows** to **Yes**.
3. **API permissions** → **Add a permission** → **APIs my organization uses** →
   **Microsoft Dynamics ERP** → Delegated → `Odata.FullAccess`.
   Grant admin consent if your tenant requires it.
4. Copy the **Application (client) ID** and **Directory (tenant) ID**.

Your D365FO user needs normal access to the data entities; no extra D365FO
setup is needed for delegated sign-in.

Try it from the command line:

```sh
cargo run -p d365-fetch --example fetch -- https://contoso.operations.dynamics.com <tenant-id> <client-id>
```
