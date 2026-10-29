//! Print MTGO's discovered resource URLs without downloading any payloads.

use mtgoxml::{Client, LIVE_DEPLOYMENT_URL};

/// Discover the supplied deployment URL, defaulting to the official live client.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| LIVE_DEPLOYMENT_URL.to_owned());
    let application = Client::new().discover(&url)?;
    if let Some(version) = application.version() {
        eprintln!(
            "MTGO {version}: {} resources",
            application.resources().len()
        );
    }
    for resource in application.resources() {
        println!("{}\t{}", resource.path, resource.url);
    }
    Ok(())
}
