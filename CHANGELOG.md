# Changelog

## Unreleased

- Dataverse support: Customer Engagement, Field Service and other Dataverse
  environments (`*.crm*.dynamics.com`). Metadata comes from `/api/data/v9.2`,
  and the generated paths use that root. `cross-company` is only added for
  Finance & Operations.

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
