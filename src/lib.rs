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

#![feature(derive_default_enum)]
#![feature(thread_id_value)]

use derive_builder::Builder;

use serde::Serialize;
use std::borrow::Cow;
use std::marker::PhantomData;
use std::str::FromStr;
use std::{collections::HashMap, io, time::Instant};
use strum::AsRefStr;
use strum_macros::EnumString;
use tracing::Subscriber;
use tracing::{span, Event};
use tracing_subscriber::{fmt::MakeWriter, layer::Context, registry::LookupSpan, Layer};

#[derive(Debug, Clone, Default, EnumString, AsRefStr)]
pub enum EventType {
    DurationBegin,
    DurationEnd,
    Complete,
    #[default]
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

#[derive(Serialize, Builder, Debug)]
#[builder(custom_constructor)]
#[builder(derive(Debug))]
pub struct ChromeEvent {
    #[builder(setter(custom))]
    #[serde(skip)]
    #[allow(unused)]
    start: Instant,
    #[builder(setter(into))]
    pub name: Cow<'static, str>,
    #[builder(setter(into))]
    pub cat: Cow<'static, str>,
    #[builder(default)]
    pub ph: EventType,
    #[builder(
        default = "Instant::now().duration_since(self.start.unwrap()).as_nanos() as f64 / 1000.0"
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
    #[serde(skip_serializing_if = "str::is_empty")]
    pub id: Cow<'static, str>,
    #[builder(default = "std::process::id().into()")]
    pub pid: u64,
    #[builder(default = "std::thread::current().id().as_u64().into()")]
    pub tid: u64,
    #[builder(default, setter(each = "arg"))]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub args: HashMap<&'static str, String>,
}

impl ChromeEvent {
    fn builder(start: Instant) -> ChromeEventBuilder {
        ChromeEventBuilder {
            start: Some(start),
            ..ChromeEventBuilder::create_empty()
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

    fn write(&self, writer: &mut dyn io::Write, event: ChromeEvent) -> io::Result<()> {
        // For faster String concat: https://users.rust-lang.org/t/fast-string-concatenation/4425/3
        let event = serde_json::to_string(&event).unwrap();
        let mut buf = String::with_capacity(1 + event.len() + 1 + 1);
        buf.push(',');
        buf.push_str(&event);
        buf.push('\n');

        io::Write::write_all(writer, buf.as_bytes())
    }
}

struct ChromeEventVisitor {
    builder: ChromeEventBuilder,
    event: Option<String>,
}

impl<'a> tracing_subscriber::field::Visit for ChromeEventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let value = format!("{:?}", value);
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
            "ph" | "ts" | "dur" | "tts" | "pid" | "tid" | "event" => {
                let value = value.trim_matches('"');

                match name {
                    "ph" => {
                        self.builder.ph(EventType::from_str(value)
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
                    // Special keyword to annotate event type
                    "event" => {
                        self.event = Some(value.to_string());
                    }
                    _ => unreachable!(),
                }
            }
            arg => {
                self.builder.arg((arg, value));
            }
        }
    }
}

struct AsyncEntered(bool);

impl<S, W> Layer<S> for ChromeLayer<S, W>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + 'static,
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
}
