//! Signs in through the browser, downloads $metadata and stores it in the cache.
//!
//! cargo run --example fetch -- <environment-url> <tenant> <client-id>

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use d365_fetch::auth::{AuthConfig, DEFAULT_AUTHORITY};
use d365_fetch::{
    Cache, Progress, Settings, download_metadata, normalize_environment_url, sign_in_interactive,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [env, tenant, client_id] = args.as_slice() else {
        eprintln!("usage: fetch <environment-url> <tenant> <client-id>");
        std::process::exit(2);
    };
    let env = normalize_environment_url(env)?;
    let cfg = AuthConfig {
        authority: DEFAULT_AUTHORITY.into(),
        tenant: tenant.clone(),
        client_id: client_id.clone(),
        environment_url: env.clone(),
    };
    let cancel = AtomicBool::new(false);
    let token = sign_in_interactive(&cfg, Duration::from_secs(300), &cancel, |url| {
        eprintln!("Opening browser. If it doesn't open, visit:\n{url}");
        webbrowser::open(url)
    })?;
    let data = download_metadata(&env, &token.access_token, &cancel, |p| match p {
        Progress::Connecting => eprintln!("Connecting..."),
        Progress::Downloading { received, .. } => {
            eprint!("\rDownloaded {:.1} MB", received as f64 / 1e6)
        }
        Progress::Done { bytes } => eprintln!("\nDone, {bytes} bytes"),
    })?;
    let cache = Cache::new(Settings::default().cache_dir());
    let outcome = cache.store(&env, &data)?;
    eprintln!("{outcome:?}: {}", cache.metadata_path(&env).display());
    Ok(())
}
