//! Signs in through the browser, downloads $metadata and stores it in the cache.
//!
//! cargo run --example fetch -- <environment-url> [tenant] [client-id]

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use d365_fetch::{
    Cache, Environment, Progress, Settings, download_metadata, normalize_environment_url,
    sign_in_interactive,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(env) = args.first() else {
        eprintln!("usage: fetch <environment-url> [tenant] [client-id]");
        std::process::exit(2);
    };
    let env = normalize_environment_url(env)?;
    let cfg = Environment {
        name: String::new(),
        url: env.clone(),
        tenant: args.get(1).cloned().unwrap_or_default(),
        client_id: args.get(2).cloned().unwrap_or_default(),
    }
    .auth_config();
    let cancel = AtomicBool::new(false);
    let token = sign_in_interactive(&cfg, Duration::from_secs(300), &cancel, |url| {
        eprintln!("Opening browser. If it doesn't open, visit:\n{url}");
        webbrowser::open(url)
    })?;
    // Dataverse hosts look like contoso.crm4.dynamics.com.
    let service_path = if env.contains(".crm") {
        "/api/data/v9.2"
    } else {
        "/data"
    };
    let data = download_metadata(
        &env,
        service_path,
        &token.access_token,
        &cancel,
        |p| match p {
            Progress::Connecting => eprintln!("Connecting..."),
            Progress::Downloading { received, .. } => {
                eprint!("\rDownloaded {:.1} MB", received as f64 / 1e6)
            }
            Progress::Done { bytes } => eprintln!("\nDone, {bytes} bytes"),
        },
    )?;
    let cache = Cache::new(Settings::default().cache_dir());
    let outcome = cache.store(&env, &data)?;
    eprintln!("{outcome:?}: {}", cache.metadata_path(&env).display());
    Ok(())
}
