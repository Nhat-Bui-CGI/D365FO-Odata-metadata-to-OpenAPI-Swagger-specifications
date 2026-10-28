# D365FO OData metadata to OpenAPI

Desktop tool (Rust + egui) that converts the OData `$metadata` of a Dynamics 365
Finance & Operations environment into OpenAPI 3.0, OpenAPI 3.1 or Swagger 2.0
specifications, for only the entities you choose.

## Status

- [x] Converter core (`crates/d365-openapi-core`)
- [ ] Fetch metadata with Azure AD sign-in, local cache with configurable location
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
