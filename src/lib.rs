//! A [Layer](https://docs.rs/tracing-subscriber/0.3.0/tracing_subscriber/layer/trait.Layer.html) that for logs formatted representations of `tracing` events viewed by the
//! [Chrome Trace Viewer](https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/preview) at `chrome://tracing`.
//!
//! # Usage
//! ```rust
//! use tracing_chrometrace::ChromeLayer;
//! use tracing_subscriber::{Registry, prelude::*};
//!
//! let (writer, guard) = ChromeLayer::with_writer(std::io::stdout);
//! tracing_subscriber::registry().with(writer).init();
//! ```

#![feature(thread_id_value)]

use std::borrow::Cow;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, io, time::SystemTime};

use crossbeam_queue::ArrayQueue;
use derivative::Derivative;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use strum::AsRefStr;
use strum_macros::EnumString;
use tracing::Subscriber;
use tracing::{span, Event};
use tracing_subscriber::{fmt::MakeWriter, layer::Context, registry::LookupSpan, Layer};

#[derive(Debug, Copy, Clone, Default, EnumString, AsRefStr, Serialize, Deserialize, PartialEq)]
pub enum EventType {
    #[serde(rename = "B")]
    DurationBegin,
    #[serde(rename = "E")]
    DurationEnd,
    #[serde(rename = "X")]
    Complete,
    #[default]
    #[serde(rename = "i")]
    Instant,
    #[serde(rename = "C")]
    Counter,
    #[serde(rename = "b")]
    AsyncStart,
    #[serde(rename = "n")]
    AsyncInstant,
    #[serde(rename = "e")]
    AsyncEnd,
    #[serde(rename = "s")]
    FlowStart,
    #[serde(rename = "t")]
    FlowStep,
    #[serde(rename = "f")]
    FlowEnd,
    #[serde(rename = "p")]
    Sample,
    #[serde(rename = "N")]
    ObjectCreated,
    #[serde(rename = "O")]
    ObjectSnapshot,
    #[serde(rename = "D")]
    ObjectDestroyed,
    #[serde(rename = "M")]
    Metadata,
    #[serde(rename = "V")]
    MemoryDumpGlobal,
    #[serde(rename = "v")]
    MemoryDumpProcess,
    #[serde(rename = "R")]
    Mark,
    #[serde(rename = "c")]
    ClockSync,
    #[serde(rename = "(")]
    ContextBegin,
    #[serde(rename = ")")]
    ContextEnd,
}

#[derive(Derivative, Serialize, Deserialize, Builder, Debug)]
#[derivative(PartialEq)]
#[builder(custom_constructor)]
#[builder(derive(Debug))]
pub struct ChromeEvent {
    #[builder(setter(custom))]
    #[serde(default = "SystemTime::now")]
    #[serde(skip)]
    #[allow(unused)]
    #[derivative(PartialEq = "ignore")]
    start: SystemTime,
    #[builder(default)]
    #[builder(setter(into))]
    pub name: Cow<'static, str>,
    #[builder(default)]
    #[builder(setter(into))]
    pub cat: Cow<'static, str>,
    #[builder(default)]
    pub ph: EventType,
    #[builder(
        default = "SystemTime::now().duration_since(self.start.unwrap()).unwrap().as_nanos() as f64 / 1000.0"
    )]
    pub ts: f64,
    #[builder(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(default)]
    pub tts: Option<f64>,
    #[builder(default)]
    #[builder(setter(into))]
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub id: Cow<'static, str>,
    #[builder(default = "std::process::id().into()")]
    pub pid: u64,
    #[builder(default = "std::thread::current().id().as_u64().into()")]
    pub tid: u64,
    #[builder(default, setter(each = "arg"))]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub args: HashMap<String, String>,
}

impl ChromeEvent {
    pub fn builder(start: SystemTime) -> ChromeEventBuilder {
        ChromeEventBuilder {
            start: Some(start),
            ..ChromeEventBuilder::create_empty()
        }
    }
}

#[derive(Debug)]
pub struct ChromeLayer<S, W = fn() -> std::io::Stdout> {
    pub start: SystemTime,
    make_writer: W,
    events: Arc<Mutex<ArrayQueue<String>>>,
    _inner: PhantomData<S>,
}

#[derive(Clone, Debug)]
pub struct ChromeWriter<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    make_writer: W,
}

impl<W> ChromeWriter<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    fn new(make_writer: W, events: Arc<Mutex<ArrayQueue<String>>>) -> (Self, ChromeWriterGuard<W>) {
        (
            Self {
                make_writer: make_writer.clone(),
            },
            ChromeWriterGuard::new(make_writer, events),
        )
    }
}

impl<W> io::Write for ChromeWriter<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut writer = self.make_writer.make_writer();
        io::Write::write(&mut writer, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a, W> MakeWriter<'a> for ChromeWriter<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    type Writer = ChromeWriter<W>;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

pub struct ChromeWriterGuard<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    make_writer: W,
    events: Arc<Mutex<ArrayQueue<String>>>,
}

impl<W> ChromeWriterGuard<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    fn new(make_writer: W, events: Arc<Mutex<ArrayQueue<String>>>) -> Self {
        // Write JSON opening parenthesis
        io::Write::write_all(&mut make_writer.make_writer(), b"[\n").unwrap();

        Self {
            make_writer,
            events,
        }
    }
}

impl<W> Drop for ChromeWriterGuard<W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    fn drop(&mut self) {
        let mut writer = self.make_writer.make_writer();

        let mut write = |event: String, is_last: bool| {
            let mut buf = String::with_capacity(event.len() + 1 /* Newline */ + 1 /* Null */);
            buf.push_str(&event);
            if !is_last {
                buf.push(',');
            }
            buf.push('\n');

            io::Write::write_all(&mut writer, buf.as_bytes()).unwrap();
        };

        if let Ok(lock) = self.events.lock() {
            let events = &*lock;
            // Write until last one left
            while events.len() > 1 {
                write(events.pop().unwrap(), false);
            }
            // Last one
            if let Some(event) = events.pop() {
                write(event, true);
            }
        }

        // Write JSON closing parenthesis
        io::Write::write_all(&mut writer, b"]\n").unwrap();
    }
}

impl<S, W> ChromeLayer<S, W>
where
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    pub fn with_writer(make_writer: W) -> (ChromeLayer<S, ChromeWriter<W>>, ChromeWriterGuard<W>) {
        let events = Arc::new(Mutex::new(ArrayQueue::new(1)));
        let (make_writer, guard) = ChromeWriter::new(make_writer, events.clone());
        (
            ChromeLayer {
                start: SystemTime::now(),
                make_writer,
                events,
                _inner: PhantomData,
            },
            guard,
        )
    }

    fn write(&self, writer: &mut dyn io::Write, event: ChromeEvent) -> io::Result<()> {
        let current = serde_json::to_string(&event).unwrap();

        // Proceed only when previous event exists
        if let Some(event) = self.events.lock().unwrap().force_push(current) {
            let mut buf = String::with_capacity(
                event.len() + 1 /* Comma */ + 1 /* Newline */ + 1, /* Null */
            );
            buf.push_str(&event);
            buf.push(',');
            buf.push('\n');
            io::Write::write_all(writer, buf.as_bytes())
        } else {
            Ok(())
        }
    }
}

struct ChromeEventVisitor {
    builder: ChromeEventBuilder,
    event: Option<String>,
}

impl tracing_subscriber::field::Visit for ChromeEventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let value = format!("{value:?}").trim_matches('"').to_string();
        let name = field.name();

        match name {
            "name" => {
                self.builder.name(value);
            }
            "cat" => {
                self.builder.cat(value);
            }
            "id" => {
                self.builder.id(value);
            }
            "ph" => {
                self.builder.ph(EventType::from_str(&value)
                    .unwrap_or_else(|_| panic!("Invalid EventType: {}", value)));
            }
            "ts" => {
                self.builder.ts(value.parse().expect("Invalid timestamp"));
            }
            "dur" => {
                self.builder
                    .dur(Some(value.parse().expect("Invalid timestamp")));
            }
            "tts" => {
                self.builder
                    .tts(Some(value.parse().expect("Invalid timestamp")));
            }
            "pid" => {
                self.builder.pid(value.parse().unwrap());
            }
            "tid" => {
                self.builder.tid(value.parse().unwrap());
            }
            "event" => {
                // Special keyword to annotate event type
                self.event = Some(value);
            }
            arg => {
                self.builder.arg((arg.to_string(), value));
            }
        }
    }
}

struct AsyncEntered(bool);

impl<S, W> Layer<S> for ChromeLayer<S, W>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    W: Clone + for<'writer> MakeWriter<'writer> + 'static,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");

        let mut visitor = ChromeEventVisitor {
            builder: ChromeEvent::builder(self.start),
            event: None,
        };
        attrs.record(&mut visitor);

        span.extensions_mut().insert(visitor);
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = ChromeEventVisitor {
            builder: ChromeEvent::builder(self.start),
            event: None,
        };

        // Default event type
        visitor.builder.ph(EventType::Instant);

        event.record(&mut visitor);

        self.write(
            &mut self.make_writer.make_writer(),
            visitor.builder.build().unwrap(),
        )
        .expect("Failed to write event in tracing-chrometrace");
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");

        let mut extensions = span.extensions_mut();

        if extensions.get_mut::<AsyncEntered>().is_some() {
            // If recoding of the span is already started (async case), skip it
            return;
        } else {
            extensions.insert(AsyncEntered(true));
        }

        if let Some(visitor) = extensions.get_mut::<ChromeEventVisitor>() {
            // Only "async" event suppported now
            if visitor
                .event
                .as_ref()
                .map(|event| event == "async")
                .unwrap_or(false)
            {
                visitor.builder.ph(EventType::AsyncStart);
            } else {
                visitor.builder.ph(EventType::DurationBegin);
            }

            self.write(
                &mut self.make_writer.make_writer(),
                visitor.builder.build().unwrap(),
            )
            .expect("Failed to write event in tracing-chrometrace");
        }
    }

    fn on_exit(&self, _id: &span::Id, _ctx: Context<'_, S>) {}

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).expect("Span not found, this is a bug");

        let mut extensions = span.extensions_mut();

        if let Some(visitor) = extensions.get_mut::<ChromeEventVisitor>() {
            if visitor
                .event
                .as_ref()
                .map(|event| event == "async")
                .unwrap_or(false)
            {
                visitor.builder.ph(EventType::AsyncEnd);
            } else {
                visitor.builder.ph(EventType::DurationEnd);
            }

            self.write(
                &mut self.make_writer.make_writer(),
                visitor.builder.build().unwrap(),
            )
            .expect("Failed to write event in tracing-chrometrace");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_stringify() {
        let event = EventType::from_str("DurationBegin").unwrap();
        matches!(event, EventType::DurationBegin);
    }

    #[test]
    fn test_serde() {
        let event = ChromeEvent::builder(SystemTime::now())
            .arg(("a".to_string(), "a".to_string()))
            .ts(1.0)
            .build()
            .unwrap();

        let serialized = serde_json::to_string(&event).unwrap();

        let deserialized: ChromeEvent = serde_json::from_str(&serialized).unwrap();

        assert_eq!(event, deserialized);
    }
}
