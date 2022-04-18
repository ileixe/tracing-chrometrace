use std::time::Instant;

use tracing_chrometrace::ChromeEvent;
use tracing_chrometrace::ChromeLayer;
use tracing_chrometrace::ChromeWriter;
use tracing_subscriber::prelude::*;

#[test]
fn test_layer_init() {
    let (writer, _guard) = ChromeWriter::with(std::io::stdout);

    tracing_subscriber::registry()
        .with(ChromeLayer::default().with_writer(writer))
        .init();

    tracing::info!(target = "chrome_layer", message = "hello");
}

#[test]
fn test_serde() {
    let event = ChromeEvent::builder(Instant::now())
        .id("1")
        .arg(("a".to_string(), "a".to_string()))
        .ts(1.0)
        .build()
        .unwrap();

    let serialized = serde_json::to_string(&event).unwrap();
    println!("{:?}", serialized);

    let deserialized: ChromeEvent = serde_json::from_str(&serialized).unwrap();

    assert_eq!(event, deserialized);
}
