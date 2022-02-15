#[test]
fn test_layer_init() {
    use tracing_chrometrace::ChromeLayer;
    use tracing_subscriber::{prelude::*, Registry};

    tracing_subscriber::registry()
        .with(ChromeLayer::default())
        .init();
}
