//! Converts a saved $metadata file.
//!
//! cargo run --example convert -- <metadata.xml> <swagger-2.0|openapi-3.0|openapi-3.1> <json|yaml> [EntitySet ...]
//! With no entity sets, every entity set is included.

use d365_openapi_core::{Options, OutputFormat, SpecVersion, generate, parse_reader, render};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!(
            "usage: convert <metadata.xml> <swagger-2.0|openapi-3.0|openapi-3.1> <json|yaml> [EntitySet ...]"
        );
        std::process::exit(2);
    }
    let file = std::io::BufReader::new(std::fs::File::open(&args[0])?);
    let md = parse_reader(file)?;
    let version = SpecVersion::ALL
        .into_iter()
        .find(|v| v.slug() == args[1])
        .ok_or("unknown spec version")?;
    let format = match args[2].as_str() {
        "json" => OutputFormat::Json,
        "yaml" => OutputFormat::Yaml,
        _ => return Err("format must be json or yaml".into()),
    };
    let sets: Vec<&str> = if args.len() > 3 {
        args[3..].iter().map(String::as_str).collect()
    } else {
        md.entity_sets.iter().map(|s| s.name.as_str()).collect()
    };
    let opts = Options {
        version,
        ..Default::default()
    };
    print!("{}", render(&generate(&md, &sets, &opts)?, format)?);
    Ok(())
}
