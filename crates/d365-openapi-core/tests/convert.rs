use d365_openapi_core::{
    Error, Options, OutputFormat, Platform, SpecVersion, generate, parse_str, render,
};
use serde_json::Value;

const SAMPLE: &str = include_str!("fixtures/sample_metadata.xml");

fn spec(version: SpecVersion, sets: &[&str]) -> Value {
    let md = parse_str(SAMPLE).unwrap();
    generate(
        &md,
        sets,
        &Options {
            version,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn parses_sample() {
    let md = parse_str(SAMPLE).unwrap();
    assert_eq!(md.entity_sets.len(), 5);
    assert_eq!(md.entity_types.len(), 5);
    assert_eq!(md.complex_types.len(), 1);
    assert_eq!(md.enum_types.len(), 2);
    assert_eq!(md.operations.len(), 2);
    let customer = md
        .entity_type("Microsoft.Dynamics.DataEntities.CustomerV3")
        .unwrap();
    assert_eq!(customer.key, ["dataAreaId", "CustomerAccount"]);
    assert_eq!(customer.properties.len(), 8);
    let credit = customer
        .properties
        .iter()
        .find(|p| p.name == "CreditLimit")
        .unwrap();
    assert!(!credit.nullable);
    assert_eq!((credit.precision, credit.scale), (Some(32), Some(6)));
}

#[test]
fn rejects_non_metadata() {
    assert!(matches!(
        parse_str("<html><body/></html>"),
        Err(Error::NotMetadata)
    ));
}

#[test]
fn search_matches_set_and_type_names() {
    let md = parse_str(SAMPLE).unwrap();
    let names = |q| {
        md.search_entity_sets(q)
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("sales"), ["SalesOrderHeadersV2", "SalesOrderLines"]);
    assert_eq!(names("customerv3"), ["CustomersV3"]);
    assert_eq!(names("").len(), 5);
}

#[test]
fn only_selected_entities_are_emitted() {
    let doc = spec(SpecVersion::OpenApi30, &["CustomersV3"]);
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    assert!(schemas.contains_key("CustomerV3"));
    assert!(schemas.contains_key("NoYes"));
    assert!(!schemas.contains_key("SalesOrderHeaderV2"));
    assert!(!schemas.contains_key("ExchangeRate"));
    // Navigation to unselected entities is dropped.
    assert!(
        schemas["CustomerV3"]["properties"]
            .get("SalesOrderHeaders")
            .is_none()
    );
    let paths: Vec<&String> = doc["paths"].as_object().unwrap().keys().collect();
    assert!(paths.iter().all(|p| p.starts_with("/data/CustomersV3")));
}

#[test]
fn composite_key_path() {
    let doc = spec(SpecVersion::OpenApi30, &["CustomersV3"]);
    let item = &doc["paths"]["/data/CustomersV3(dataAreaId='{dataAreaId}',CustomerAccount='{CustomerAccount}')"];
    assert!(item["get"].is_object() && item["patch"].is_object() && item["delete"].is_object());
}

#[test]
fn single_key_path() {
    let doc = spec(SpecVersion::OpenApi30, &["ExchangeRates"]);
    assert!(doc["paths"]["/data/ExchangeRates({RecId})"]["get"].is_object());
}

#[test]
fn bound_actions() {
    let doc = spec(SpecVersion::OpenApi30, &["SalesOrderHeadersV2"]);
    let p = &doc["paths"];
    assert!(
        p["/data/SalesOrderHeadersV2/Microsoft.Dynamics.DataEntities.getOpenOrderCount"]["post"]
            .is_object()
    );
    assert!(p["/data/SalesOrderHeadersV2(dataAreaId='{dataAreaId}',SalesOrderNumber='{SalesOrderNumber}')/Microsoft.Dynamics.DataEntities.confirmSalesOrder"]["post"].is_object());
}

#[test]
fn nullability_per_version() {
    let name = |doc: &Value, schemas: &str| {
        doc[schemas]["CustomerV3"]["properties"]["OrganizationName"].clone()
    };
    let v2 = spec(SpecVersion::Swagger2, &["CustomersV3"]);
    assert_eq!(name(&v2, "definitions")["x-nullable"], true);
    let v30 = spec(SpecVersion::OpenApi30, &["CustomersV3"]);
    assert_eq!(
        v30["components"]["schemas"]["CustomerV3"]["properties"]["OrganizationName"]["nullable"],
        true
    );
    let v31 = spec(SpecVersion::OpenApi31, &["CustomersV3"]);
    assert_eq!(
        v31["components"]["schemas"]["CustomerV3"]["properties"]["OrganizationName"]["type"],
        serde_json::json!(["string", "null"])
    );
}

#[test]
fn unknown_entity_set_is_an_error() {
    let md = parse_str(SAMPLE).unwrap();
    assert!(matches!(
        generate(&md, &["Nope"], &Options::default()),
        Err(Error::UnknownEntitySet(_))
    ));
    assert!(matches!(
        generate(&md, &[], &Options::default()),
        Err(Error::NothingSelected)
    ));
}

#[test]
fn renders_json_and_yaml() {
    let doc = spec(SpecVersion::OpenApi31, &["CustomersV3"]);
    let json = render(&doc, OutputFormat::Json).unwrap();
    assert!(json.contains("\"openapi\": \"3.1.0\""));
    let yaml = render(&doc, OutputFormat::Yaml).unwrap();
    assert!(yaml.contains("openapi: 3.1.0"));
}

#[test]
fn handles_large_metadata() {
    // Roughly the size of a full D365FO environment (several thousand entities).
    let mut xml = String::from(
        r#"<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Ns" xmlns="http://docs.oasis-open.org/odata/ns/edm">"#,
    );
    for i in 0..5000 {
        xml.push_str(&format!(r#"<EntityType Name="E{i}"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String" Nullable="false"/>"#));
        for j in 0..40 {
            xml.push_str(&format!(
                r#"<Property Name="P{j}" Type="Edm.String" MaxLength="60"/>"#
            ));
        }
        xml.push_str("</EntityType>");
    }
    xml.push_str(r#"<EntityContainer Name="C">"#);
    for i in 0..5000 {
        xml.push_str(&format!(r#"<EntitySet Name="S{i}" EntityType="Ns.E{i}"/>"#));
    }
    xml.push_str("</EntityContainer></Schema></edmx:DataServices></edmx:Edmx>");
    let md = parse_str(&xml).unwrap();
    assert_eq!(md.entity_sets.len(), 5000);
    let doc = generate(&md, &["S1", "S4999"], &Options::default()).unwrap();
    assert_eq!(doc["paths"].as_object().unwrap().len(), 4);
}

#[test]
fn dataverse_paths() {
    let md = parse_str(include_str!("fixtures/dataverse_metadata.xml")).unwrap();
    assert_eq!(Platform::of_metadata(&md), Platform::Dataverse);
    assert_eq!(
        Platform::of_metadata(&parse_str(SAMPLE).unwrap()),
        Platform::FinanceOperations
    );
    let doc = generate(
        &md,
        &["accounts", "contacts", "msdyn_workorders"],
        &Options {
            platform: Platform::Dataverse,
            ..Default::default()
        },
    )
    .unwrap();
    let paths = doc["paths"].as_object().unwrap();
    assert!(paths.contains_key("/api/data/v9.2/accounts"));
    assert!(paths.contains_key("/api/data/v9.2/accounts({accountid})"));
    assert!(paths.contains_key("/api/data/v9.2/msdyn_workorders({msdyn_workorderid})"));
    assert!(!paths.keys().any(|p| p.starts_with("/data/")));
    let text = serde_json::to_string(&doc).unwrap();
    assert!(!text.contains("cross-company"));
    assert!(doc["components"]["schemas"]["account"]["properties"]["name"].is_object());
}
