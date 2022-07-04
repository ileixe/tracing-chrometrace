use itertools::Itertools;
use std::{
    fs::{self, File},
    thread,
    time::Duration,
};
use tempfile;

use tracing_appender::non_blocking::NonBlocking;
use tracing_chrometrace::{ChromeEvent, ChromeLayer};
use tracing_subscriber::prelude::*;

#[test]
fn test_init() {
    let (writer, _guard) = ChromeLayer::with_writer(std::io::stdout);

    tracing_subscriber::registry().with(writer).init();

    tracing::info!(target = "chrome_layer", message = "hello");
}

#[test]
fn test_concurrent_write() {
    let file = temp_file::empty();
    let (writer, worker) = NonBlocking::new(File::create(file.path()).unwrap());
    let (writer, guard) = ChromeLayer::with_writer(writer);

    let iterations = 1000;

    tracing_subscriber::registry().with(writer).init();

    let handle = thread::spawn(move || {
        for i in 0..iterations {
            tracing::info!(thread = 0, index = i);
        }
    });

    let handle2 = thread::spawn(move || {
        for i in 0..iterations {
            tracing::info!(thread = 1, index = i);
        }
    });

    let handle3 = thread::spawn(move || {
        for i in 0..iterations {
            tracing::info!(thread = 2, index = i);
        }
    });

    let handle4 = thread::spawn(move || {
        for i in 0..iterations {
            tracing::info!(thread = 3, index = i);
        }
    });

    handle.join();
    handle2.join();
    handle3.join();
    handle4.join();

    drop(guard);
    drop(worker);

    let events = fs::read_to_string(file.path()).unwrap();
    let events: Vec<ChromeEvent> = serde_json::from_str::<Vec<ChromeEvent>>(&events).unwrap();

    let expected: Vec<i32> = (0..iterations).collect();

    for i in 0..4 {
        let found: Vec<i32> = events
            .iter()
            .filter(|e| e.args["thread"] == i.to_string())
            .map(|e| e.args["index"].parse().unwrap())
            .collect();

        assert_eq!(expected, found)
    }
}
