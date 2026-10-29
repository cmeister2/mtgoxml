//! Load and validate saved CardDataSource files for a single deployment snapshot.

use std::fs::File;
use std::path::PathBuf;

use mtgoxml::{Deployment, carddata::CardDataFile};

/// Load components from a directory and optionally print one resolved object.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if !(4..=5).contains(&arguments.len()) {
        return Err("usage: database DEPLOYMENT_URL DEPLOYMENT_XML APPLICATION_XML CARD_DATA_DIRECTORY [OBJECT_ID]".into());
    }
    let deployment = Deployment::from_bytes(&arguments[0], &std::fs::read(&arguments[1])?)?;
    let application = deployment.with_application_bytes(&std::fs::read(&arguments[2])?)?;
    let directory = PathBuf::from(&arguments[3]);
    let mut database = application.card_database();
    for resource in application
        .card_data_resources()
        .filter(|resource| resource.is_xml())
    {
        // Read each independent component; the master index needs no expansion.
        let filename = resource.path.rsplit('/').next().unwrap_or_default();
        if filename.eq_ignore_ascii_case("MTGOV3CardData.xml") {
            continue;
        }
        let file = CardDataFile::from_reader(File::open(directory.join(filename))?)?;
        database.insert(&application, file)?;
    }
    database.validate()?;
    println!(
        "{} objects, {} set fragments, {} lookup tables",
        database.len(),
        database.sets().len(),
        database.lookups().len()
    );
    if let Some(id) = arguments.get(4) {
        // Show inherited properties through the same public API as live loading.
        let object = database.object(id.parse()?)?;
        println!(
            "{}: {} ({})",
            object.id(),
            object.name().unwrap_or("Unnamed object"),
            object.set_code()
        );
        println!(
            "Type: {:?}; foil: {}",
            object.object_type(),
            object.is_foil()?
        );
    }
    Ok(())
}
