# tracing-chrometrace

A [`tracing`] layer that writes [Chrome Trace][format] events, which `chrome://tracing` and
[Perfetto] read.

```toml
[dependencies]
tracing-chrometrace = "0.2"
```

```rust
use tracing_chrometrace::ChromeLayer;
use tracing_subscriber::prelude::*;

let (layer, trace) = ChromeLayer::with_writer(std::io::stdout);
tracing_subscriber::registry().with(layer).init();

tracing::info_span!("load", cat = "io").in_scope(|| {
    // the work being reported on
});

drop(trace);
```

Hold `trace` for as long as you are recording. Dropping it writes what is pending and ends the
JSON array. The format calls that final bracket optional and `chrome://tracing` supplies it,
but not every viewer does, so let the value drop rather than counting on that.

## Fields decide the event

The names the format defines are read from the span or the event. Every other field arrives
under `args`, keeping the JSON type it was recorded with.

A span that names `ph` reports one event when it closes, so you can report an extent you
already know rather than one the clock measures. A device timeline replayed after the fact is
the case this exists for: the timestamps are the device's, not the host's.

```rust
tracing::info_span!("NPU", cat = "NPU", ph = "Complete", ts = 2012429, dur = 7306);
```

A span that names no phase reports its enter and its close as a pair, timed by the clock.

Recording is cheap enough to instrument work measured in microseconds; `cargo bench` reports
what it costs on your machine. See the [API documentation] for the fields the format names and
the phases it accepts.

## License

MIT

[`tracing`]: https://docs.rs/tracing
[format]: https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/preview
[Perfetto]: https://ui.perfetto.dev
[API documentation]: https://docs.rs/tracing-chrometrace
