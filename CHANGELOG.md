# Changelog

## Unreleased

- Export can write one file per entity, named after its entity set (for
  example `CustomersV3.openapi-3.0.json`), in addition to or instead of the
  combined file.
- A dialog now shows when an export is finished or failed, with a button to
  open the output folder.

## 1.1.3 (2026-10-29)

- The egui app now uses a light theme.

## 1.1.2 (2026-10-29)

- Fix: after the environment refused a sign-in (401/403), every retry reused
  that sign-in until the app was restarted. The app now forgets it and asks
  you to sign in again.

## 1.1.1 (2026-10-29)

- New app icon (orange Rust gear with API braces) in all apps, the Windows
  .exe files and the installers.

## 1.1.0 (2026-10-29)

- New Slint desktop app (`crates/d365-openapi-slint`), released for Windows and
  macOS next to the egui and Tauri apps.
- Dataverse support: Customer Engagement, Field Service and other Dataverse
  environments (`*.crm*.dynamics.com`). Metadata comes from `/api/data/v9.2`,
  and the generated paths use that root. `cross-company` is only added for
  Finance & Operations.
- macOS: universal dmgs for every app.

## 1.0.0 (2026-10-28)

First release.

- **Converter**: D365FO OData `$metadata` to OpenAPI 3.0, OpenAPI 3.1 or
  Swagger 2.0, as JSON or YAML, for only the selected entity sets. Covers
  list, get, create, update and delete with composite keys, bound actions and
  functions, enums, complex types and navigation between selected entities.
- **Sign-in and download**: interactive browser sign-in (an app registration is
  optional), metadata download with progress and cancel.
- **Local cache**: metadata is saved per environment and reused on the next
  start. "Check for updates" replaces it only when D365FO has changed. The
  cache folder can be changed in Settings.
- **Two desktop apps** sharing the same settings and data:
  - egui app (`d365-openapi-app`)
  - Tauri app (`d365-openapi-tauri`)
