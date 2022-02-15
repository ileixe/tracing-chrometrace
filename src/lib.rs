//! A [Layer](https://docs.rs/tracing-subscriber/0.3.0/tracing_subscriber/layer/trait.Layer.html) that for logs formatted representations of `tracing` events viewed by the
//! [Chrome Trace Viewer](https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/preview) at `chrome://tracing`.
//!
//! # Usage
//! ```rust
//! use tracing_chrometrace::ChromeLayer;
//! use tracing_subscriber::{Registry, prelude::*};
//!
//! tracing_subscriber::registry().with(ChromeLayer::default()).init();
//! ```

#![feature(thread_id_value)]

use serde::Serialize;
use std::marker::PhantomData;
use std::str::FromStr;
use std::{collections::HashMap, io, process, thread, time::Instant};
use strum_macros::EnumString;
use tracing::Subscriber;
use tracing::{span, Event};
use tracing_subscriber::{fmt::MakeWriter, layer::Context, registry::LookupSpan, Layer};

#[derive(Debug, EnumString)]
pub enum EventType {
    DurationBegin,
    DurationEnd,
    Complete,
    Instant,
    Counter,
    AsyncStart,
    AsyncInstant,
    AsyncEnd,
    FlowStart,
    FlowStep,
    FlowEnd,
    Sample,
    ObjectCreated,
    ObjectSnapshot,
    ObjectDestroyed,
    Metadata,
    MemoryDumpGlobal,
    MemoryDumpProcess,
    Mark,
    ClockSync,
    ContextBegin,
    ContextEnd,
}

impl Serialize for EventType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let event = match *self {
            EventType::DurationBegin => "B",
            EventType::DurationEnd => "E",
            EventType::Complete => "X",
            EventType::Instant => "i",
            EventType::Counter => "C",
            EventType::AsyncStart => "b",
            EventType::AsyncInstant => "n",
            EventType::AsyncEnd => "e",
            EventType::FlowStart => "s",
            EventType::FlowStep => "t",
            EventType::FlowEnd => "f",
            EventType::Sample => "P",
            EventType::ObjectCreated => "N",
            EventType::ObjectSnapshot => "O",
            EventType::ObjectDestroyed => "D",
            EventType::Metadata => "M",
            EventType::MemoryDumpGlobal => "V",
            EventType::MemoryDumpProcess => "v",
            EventType::Mark => "R",
            EventType::ClockSync => "c",
            EventType::ContextBegin => "(",
            EventType::ContextEnd => ")",
        };

        serializer.serialize_str(event)
    }
}

#[derive(Serialize, Debug)]
struct EventDescription {
    name: String,
    cat: String,
    ph: EventType,
    ts: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    dur: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tts: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    pid: u64,
    tid: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<HashMap<String, String>>,
}

impl EventDescription {
    fn new(start: Instant, event_type: EventType, mut fields: HashMap<String, String>) -> Self {
        let name = fields
            .remove("name")
            .unwrap_or("DefaultEventName".to_string());

        let cat = fields
            .remove("cat")
            .unwrap_or("DefaultCategory".to_string());

        let ts = fields
            .remove("ts")
            .map_or(start.elapsed().as_nanos() as f64 / 1000., |x| {
                x.trim_matches('"').parse().unwrap()
            });

        let dur = fields
            .remove("dur")
            .map_or(None, |x| Some(x.trim_matches('"').parse().unwrap()));

        let id = fields.remove("id");

        let pid = fields.remove("pid").map_or(process::id() as u64, |x| {
            x.trim_matches('"').parse().unwrap()
        });

        let tid = fields
            .remove("tid")
            .map_or(thread::current().id().as_u64().get(), |x| {
                x.trim_matches('"').parse().unwrap()
            });

        EventDescription {
            name,
            cat,
            ph: event_type,
            dur,
            ts,
            tts: None, // Not yet supported
            id,
            pid,
            tid,
            args: if fields.len() > 0 { Some(fields) } else { None },
        }
    }
}

#[derive(Debug)]
pub struct ChromeLayer<S, W = fn() -> std::io::Stdout> {
    pub start: Instant,
    make_writer: W,
    _inner: PhantomData<S>,
}

impl<S> Default for ChromeLayer<S> {
    fn default() -> ChromeLayer<S> {
        Self {
            start: Instant::now(),
            make_writer: io::stdout,
            _inner: PhantomData,
        }
    }
}

impl<S, W> ChromeLayer<S, W> {
    pub fn with_writer<W2>(self, make_writer: W2) -> ChromeLayer<S, W2>
    where
        W2: for<'writer> MakeWriter<'writer> + 'static,
    {
        // TODO: Any other way to make a valid JSON array? Note that we even don't have close parenthesis.
        let mut writer = make_writer.make_writer();
        // Add dummy empty entry to make valid JSON
        io::Write::write_all(&mut writer, b"[{}\n").unwrap();
        drop(writer);

        ChromeLayer {
            start: Instant::now(),
            make_writer,
            _inner: PhantomData,
        }
    }

    fn write(&self, writer: &mut dyn io::Write, description: &EventDescription) -> io::Result<()> {
        io::Write::write_all(
            writer,
            (",".to_owned() + &serde_json::to_string(description).unwrap() + "\n").as_bytes(),
        )
    }
}

struct Fields {
    inner: HashMap<String, String>,
}

impl<'a> tracing_subscriber::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.inner
            .insert(field.name().to_string(), format!("{:?}", value));
    }
}

impl<S, W> Layer<S> for ChromeLayer<S, W>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");

        let mut visitor = Fields {
            inner: HashMap::new(),
        };
        attrs.record(&mut visitor);

        span.extensions_mut().insert(visitor);
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields {
            inner: HashMap::new(),
        };
        event.record(&mut fields);

        let event_type = fields
            .inner
            .remove("event")
            .map_or(EventType::Instant, |e| {
                EventType::from_str(&e.trim_matches('"'))
                    .expect(format!("EventType expected, not {:?}", e).as_str())
            });

        let description = EventDescription::new(self.start, event_type, fields.inner);

        let mut writer = self.make_writer.make_writer();

        self.write(&mut writer, &description)
            .expect("Failed to write event in tracing-chrometrace");
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");

        let mut extensions = span.extensions_mut();

        if extensions.get_mut::<bool>().is_some() {
            // If recoding of the span is already started (async case), skip it
            return;
        } else {
            extensions.insert(true);
        }

        if let Some(fields) = extensions.get_mut::<Fields>() {
            let mut fields = fields.inner.clone();

            let event_type = fields
                .remove("event")
                .map_or(EventType::DurationBegin, |e| {
                    EventType::from_str(&e.trim_matches('"'))
                        .expect(format!("EventType expected, not {:?}", e).as_str())
                });

            let description = EventDescription::new(self.start, event_type, fields);

            let mut writer = self.make_writer.make_writer();
            self.write(&mut writer, &description)
                .expect("Failed to write event in tracing-chrometrace");
        };
    }

    fn on_exit(&self, _id: &span::Id, _ctx: Context<'_, S>) {}

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");

        if let Some(fields) = span.extensions().get::<Fields>() {
            let mut fields = fields.inner.clone();

            let event_type = fields.remove("event").map_or(EventType::DurationEnd, |e| {
                EventType::from_str(&e.trim_matches('"'))
                    .expect(format!("EventType expected, not {:?}", e).as_str())
            });

            let description = EventDescription::new(self.start, event_type, fields);

            let mut writer = self.make_writer.make_writer();
            self.write(&mut writer, &description)
                .expect("Failed to write event in tracing-chrometrace");
        };
    }
}
