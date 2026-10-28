# D365FO OData metadata to OpenAPI

Desktop tool (Rust + egui) that converts the OData `$metadata` of a Dynamics 365
Finance & Operations environment into OpenAPI 3.0, OpenAPI 3.1 or Swagger 2.0
specifications, for only the entities you choose.

## Status

- [x] Converter core (`crates/d365-openapi-core`)
- [x] Fetch metadata with Azure AD browser sign-in, local cache with configurable location (`crates/d365-fetch`)
- [ ] egui desktop app (search/select entities, pick formats, preview, export, download progress)
- [ ] Verified imports (Swagger UI, Azure API Management, codegen) and installers

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
   **Microsoft Dynamics ERP** → Delegated → `user_impersonation`
   (or `Odata.FullAccess`). Grant admin consent if your tenant requires it.
4. Copy the **Application (client) ID** and **Directory (tenant) ID**.

Your D365FO user needs normal access to the data entities; no extra D365FO
setup is needed for delegated sign-in.

Try it from the command line:

```sh
cargo run -p d365-fetch --example fetch -- https://contoso.operations.dynamics.com <tenant-id> <client-id>
```
