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
