//! Resolve a card from selected live MTGO set files and lookup tables.

use mtgoxml::Client;

/// Print a card's resolved properties, defaulting to the V09 Mystical Tutor foil.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let id = arguments
        .next()
        .map_or(Ok(33915), |value| value.parse::<u64>())?;
    let set = arguments.next().unwrap_or_else(|| "V09".into());
    let client = Client::new();
    let application = client.live()?;
    let database = client.load_card_sets(&application, [set])?;
    let card = database.card(id)?;
    println!(
        "{} ({})",
        card.name().unwrap_or("Unnamed card"),
        card.set_code()
    );
    println!("Foil: {}", card.is_foil()?);
    if let Some(cost) = card.mana_cost() {
        println!("Mana cost: {cost}");
    }
    if let Some(text) = card.rules_text() {
        println!("{text}");
    }
    Ok(())
}
