use tracing_chrometrace::ChromeLayer;
use tracing_subscriber::prelude::*;

#[test]
fn test_init() {
    let (writer, _guard) = ChromeLayer::with_writer(std::io::stdout);

    tracing_subscriber::registry().with(writer).init();

    tracing::info!(target = "chrome_layer", message = "hello");
}
