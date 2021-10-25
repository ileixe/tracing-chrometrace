tracing-chrometrace
===================

# Overview

A tracing [Layer](https://docs.rs/tracing-subscriber/0.3.0/tracing_subscriber/layer/trait.Layer.html) that for logs formatted representations of `tracing` events viewed by the [Chrome Trace Viewer](https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/preview) at `chrome://tracing`.

# Usage

```rust
use tracing_chrometrace::ChromeLayer;
use tracing_subscriber::{Registry, prelude::*};

tracing_subscriber::registry().with(ChromeLayer::default()).init();
```
