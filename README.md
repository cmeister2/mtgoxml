# mtgoxml

Resolve Magic: The Gathering Online data into cards and other digital objects,
including lookup tables, clone inheritance, and nested properties.

Enable `http` to load live data:

```toml
[dependencies]
mtgoxml = { path = "/path/to/mtgoxml", features = ["http"] }
```

```rust
use mtgoxml::Client;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let application = client.live()?;
    let database = client.load_card_sets(&application, ["V09"])?;
    let card = database.card(33915)?;
    println!("Name: {:?}, foil: {}", card.name(), card.is_foil()?);
    Ok(())
}
```

Rust **1.85+**. Parsing and resolution also work offline without `http`.

Build the API docs with `cargo doc --all-features --no-deps --open`.

- [Usage and resource discovery](docs/usage.md)
- [Card-data parsing and resolution](docs/card-data.md)
- [Development and releases](docs/development.md)
