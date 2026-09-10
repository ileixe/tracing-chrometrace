use rusty_fork::rusty_fork_test;
use std::{
    fs::{self, File},
    thread,
};

use tracing_appender::non_blocking::NonBlocking;
use tracing_chrometrace::{ChromeEvent, ChromeLayer};
use tracing_subscriber::prelude::*;

rusty_fork_test! {
    #[test]
    fn test_init() {
        let (writer, _guard) = ChromeLayer::with_writer(std::io::stdout);

        tracing_subscriber::registry().with(writer).init();

        tracing::info!(target = "chrome_layer", message = "hello");
    }

    /// What `origin` is for: an event the layer stamps itself and one a caller places against
    /// `origin` have to land on one axis, which holds only while both are microseconds past it.
    #[test]
    fn a_caller_places_an_event_on_the_layer_s_own_axis() {
        let file = temp_file::empty();
        let (writer, trace) = ChromeLayer::with_writer(File::create(file.path()).unwrap());
        let origin = writer.origin();

        tracing_subscriber::registry().with(writer).init();

        std::thread::sleep(std::time::Duration::from_millis(20));
        let placed = origin.elapsed().as_micros() as f64;
        tracing::info!(name = "placed", ts = placed);
        tracing::info!(name = "stamped");

        drop(trace);

        let written = fs::read_to_string(file.path()).unwrap();
        let events: Vec<ChromeEvent> = serde_json::from_str(&written).unwrap();
        let [placed_event, stamped] = &events[..] else {
            panic!("both events are written, got {}", events.len())
        };
        assert_eq!(placed_event.ts, placed);
        // The stamped one is recorded after, so the axis agrees only if it reads later.
        assert!(
            stamped.ts >= placed_event.ts,
            "stamped {} precedes placed {}, so they are not on one axis",
            stamped.ts,
            placed_event.ts,
        );
        assert!(placed_event.ts >= 20_000.0, "the placed event lands past the sleep");
    }

    /// The spec calls the closing bracket optional and `chrome://tracing` supplies it, but the
    /// Perfetto UI rejects a trace without one, so the written file has to carry it itself.
    #[test]
    fn closes_the_array_past_a_flush() {
        let file = temp_file::empty();
        let (writer, trace) = ChromeLayer::with_writer(File::create(file.path()).unwrap());

        tracing_subscriber::registry().with(writer).init();

        // Enough events that the writer is called several times before the trace ends.
        for cycle in 0..4000u64 {
            tracing::info_span!(
                "NPU",
                cat = "NPU",
                name = "Task",
                ph = "Complete",
                ts = cycle * 8,
                dur = 7,
            )
            .in_scope(|| {});
        }

        drop(trace);

        let written = fs::read_to_string(file.path()).unwrap();
        assert!(written.starts_with('['), "the array has to open");
        assert_eq!(written.trim_end().chars().last(), Some(']'));

        let events: Vec<ChromeEvent> = serde_json::from_str(&written).unwrap();
        assert_eq!(events.len(), 4000);
        assert!(events.iter().all(|event| event.dur == Some(7.0)));
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

        handle.join().unwrap();
        handle2.join().unwrap();
        handle3.join().unwrap();
        handle4.join().unwrap();

        drop(guard);
        drop(worker);

        let events = fs::read_to_string(file.path()).unwrap();
        let events: Vec<ChromeEvent> = serde_json::from_str::<Vec<ChromeEvent>>(&events).unwrap();

        let expected: Vec<i32> = (0..iterations).collect();

        for i in 0..4 {
            let found: Vec<i32> = events
                .iter()
                .filter(|e| e.args.get("thread") == Some(&serde_json::json!(i)))
                .map(|e| e.args.get("index").unwrap().as_i64().unwrap() as i32)
                .collect();

            assert_eq!(expected, found)
        }
    }
}
