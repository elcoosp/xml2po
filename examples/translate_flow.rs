//! End-to-end library example: extract → translate → compile, with a
//! PO merge and an XLIFF round trip.
//!
//! Run with `cargo run --example translate_flow`.

use std::collections::HashMap;

use xml2po::message::{CatalogKey, OwnedCatalogKey};
use xml2po::po::PoCatalog;
use xml2po::{apply_translations, extract_messages, get_strategy};

fn main() -> Result<(), xml2po::Error> {
    let page = r#"<html><body>
    <h1>Welcome to our shop</h1>
    <p>We ship to <b>over 40</b> countries.</p>
    <img alt="A sunset over the harbor" src="hero.jpg">
</body></html>"#;

    let strategy = get_strategy("html")?;

    // 1. Extract: sentences stay whole thanks to inline placeholders.
    let messages = extract_messages(page, "index.html", strategy)?;
    println!("Extracted {} messages:", messages.len());
    for message in &messages {
        println!("  - {:?}", message.msgid);
    }

    // 2. Turn them into a PO catalog (what you would hand to translators).
    let catalog = PoCatalog::from_messages(&messages);
    let po = catalog.to_po_string();
    println!("\n--- PO output ---\n{po}");

    // 3. Simulate the translator: fill in the Spanish strings.
    let mut translated = PoCatalog::parse(&po)?;
    translated.set_language("es");
    let pairs = [
        ("Welcome to our shop", "Bienvenido a nuestra tienda"),
        (
            "We ship to <b>over 40</b> countries.",
            "Enviamos a <b>más de 40</b> países.",
        ),
        ("A sunset over the harbor", "Un atardecer sobre el puerto"),
    ];
    for (msgid, msgstr) in pairs {
        translated.set_translation(CatalogKey::new(None, msgid), msgstr);
    }

    // 4. Compile the translations back into the document.
    let translations: HashMap<OwnedCatalogKey, String> = translated.translations();
    let spanish = apply_translations(page, &translations, strategy)?;
    println!("--- Translated page ---\n{spanish}");

    // 5. Bonus: XLIFF round trip (feature `xliff`, on by default).
    #[cfg(feature = "xliff")]
    {
        let xliff = xml2po::catalog_to_xliff(&translated, "en", Some("es"));
        let imported = xml2po::catalog_from_xliff(&xliff)?;
        println!(
            "XLIFF round trip: {} entries, header language {}",
            imported.entries.len(),
            imported.metadata().language.as_deref().unwrap_or("?")
        );
    }

    Ok(())
}
