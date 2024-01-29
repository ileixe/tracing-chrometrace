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
use derivative::Derivative;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::Write;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Instant;
use std::{collections::HashMap, io};
use strum::AsRefStr;
use strum_macros::EnumString;
use tracing::Subscriber;
use tracing::{span, Event};
use tracing_subscriber::{fmt::MakeWriter, layer::Context, registry::LookupSpan, Layer};

pub mod minimal;

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
    #[builder(default)]
    #[builder(setter(into))]
    pub name: Cow<'static, str>,
    #[builder(default)]
    #[builder(setter(into))]
    pub cat: Cow<'static, str>,
    #[builder(default)]
    pub ph: EventType,
    #[builder(default = "0f64")]
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

    #[builder(default = "1")]
    pub pid: u64,

    #[builder(default = "1")]
    pub tid: u64,

    #[builder(default, setter(each = "arg"))]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub args: HashMap<String, String>,
}

impl ChromeEvent {
    pub fn builder(start: Instant) -> ChromeEventBuilder {
        let ts = Instant::now().duration_since(start).as_nanos() as f64 / 1000.0;
        ChromeEventBuilder {
            // start: Some(start),
            ts: Some(ts),
            ..ChromeEventBuilder::create_empty()
        }
    }
}

#[derive(Debug, Default, Builder)]
pub struct ChromeLayerConfig {
    pub batch_size: usize,
}

#[derive(Debug)]
pub struct ChromeLayer<S> {
    _inner: PhantomData<S>,
    tx: crossbeam::channel::Sender<Message>,
    buffer: Arc<RwLock<Vec<SpanInfo>>>,
    map: Arc<RwLock<HashMap<span::Id, SpanInfo>>>,
    config: ChromeLayerConfig,
}

pub struct ChromeWriterGuard {
    handle: Option<JoinHandle<()>>,
    tx: crossbeam::channel::Sender<Message>,
}

impl ChromeWriterGuard {
    fn new(handle: JoinHandle<()>, tx: crossbeam::channel::Sender<Message>) -> Self {
        Self {
            handle: Some(handle),
            tx,
        }
    }
}

impl Drop for ChromeWriterGuard {
    fn drop(&mut self) {
        self.tx.send(Message::Drop).unwrap();
        let handle = self.handle.take().unwrap();
        handle.join().unwrap();
    }
}

#[derive(Debug)]
enum Message {
    Span(Vec<SpanInfo>),
    Drop,
}

fn write_spans<W>(mut writer: &mut W, spans: Vec<SpanInfo>, start: Instant)
where
    W: io::Write,
{
    for span in spans {
        let Some(start_time) = span.start else {
            eprintln!("no start time: {span:?}");
            continue;
        };
        let Some(duration) = start_time.checked_duration_since(start) else {
            eprintln!("duration failed: {start:?} {span:?}");
            continue;
        };
        let ts = duration.as_nanos() as f64 / 1000.0;
        let name = span.name.unwrap_or("no name".to_string());

        let begin_event = ChromeEvent::builder(start_time)
            .name(name.clone())
            .ts(ts)
            .ph(EventType::DurationBegin)
            .build()
            .expect("failed to build event");
        serde_json::to_writer(&mut writer, &begin_event).unwrap();

        let ts = span.end.unwrap().duration_since(start).as_nanos() as f64 / 1000.0;
        let end_event = ChromeEvent::builder(start_time)
            .name(name.clone())
            .ts(ts)
            .ph(EventType::DurationEnd)
            .build()
            .expect("failed to build event");
        serde_json::to_writer(&mut writer, &end_event).unwrap();
        writer.write(b",\n").unwrap();

        let end = serde_json::json!({
            "name" : name,
            "cat" : "some category",
            "ph": "E",
            "pid": 1,
            "tid": 1,
            "ts": ts,
        });
        serde_json::to_writer(&mut writer, &end).unwrap();
        writer.write(b",\n").unwrap();
    }
    writer.flush().unwrap();
}

impl<S> ChromeLayer<S> {
    pub fn with_writer<W>(make_writer: W) -> (ChromeLayer<S>, ChromeWriterGuard)
    where
        W: Clone + for<'writer> MakeWriter<'writer> + 'static + std::marker::Send,
    {
        let cloned_make_writer = make_writer.clone();
        let (tx, rx) = crossbeam::channel::unbounded::<Message>();
        let start = Instant::now();
        let buffer = Arc::new(RwLock::new(Vec::new()));
        let handle = {
            let buffer = buffer.clone();
            std::thread::spawn(move || {
                let mut writer = cloned_make_writer.make_writer();
                writer.write(b"[").unwrap();

                while let Ok(msg) = rx.recv() {
                    match msg {
                        Message::Span(v) => {
                            write_spans(&mut writer, v, start);
                        }
                        Message::Drop => {
                            let remain: Vec<_> = buffer.write().unwrap().drain(..).collect();
                            write_spans(&mut writer, remain, start);
                            writer.write(b"]").unwrap();
                            break;
                        }
                    }
                }
            })
        };

        let guard = ChromeWriterGuard::new(handle, tx.clone());
        (
            ChromeLayer {
                _inner: PhantomData,
                tx,
                buffer,
                map: Arc::new(RwLock::new(HashMap::new())),
                config: ChromeLayerConfig { batch_size: 10000 },
            },
            guard,
        )
    }
}

struct SpanInfoVisitor<'a> {
    span_info: &'a mut SpanInfo,
}

impl tracing_subscriber::field::Visit for SpanInfoVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "name" => {
                self.span_info.name = Some(format!("{value:?}"));
            }
            "cat" => {
                self.span_info.cat = Some(format!("{value:?}"));
            }
            _ => {}
        }
    }
}

#[derive(Default, Debug)]
struct SpanInfo {
    name: Option<String>,
    cat: Option<String>,
    start: Option<Instant>,
    end: Option<Instant>,
}

impl<S> Layer<S> for ChromeLayer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, _ctx: Context<'_, S>) {
        let mut span_info = SpanInfo::default();
        attrs.record(&mut SpanInfoVisitor {
            span_info: &mut span_info,
        });
        self.map.write().unwrap().insert(id.clone(), span_info);
    }

    fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {}

    fn on_enter(&self, id: &span::Id, _ctx: Context<'_, S>) {
        let mut locked_map = self.map.write().unwrap();
        let locked_span = locked_map.get_mut(id).unwrap();
        if locked_span.start.is_none() {
            locked_span.start = Some(Instant::now());
        }
    }

    fn on_exit(&self, _id: &span::Id, _ctx: Context<'_, S>) {}

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        let mut spaninfo = self.map.write().unwrap().remove(&id).unwrap();
        spaninfo.end = Some(Instant::now());
        self.buffer.write().unwrap().push(spaninfo);
        if self.buffer.read().unwrap().len() > self.config.batch_size {
            let drained: Vec<_> = self.buffer.write().unwrap().drain(..).collect();
            self.tx.try_send(Message::Span(drained)).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_stringify() {
        use std::str::FromStr;
        let event = EventType::from_str("DurationBegin").unwrap();
        matches!(event, EventType::DurationBegin);
    }

    #[test]
    fn test_serde() {
        let event = ChromeEvent::builder(Instant::now())
            .arg(("a".to_string(), "a".to_string()))
            .ts(1.0)
            .build()
            .unwrap();

        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: ChromeEvent = serde_json::from_str(&serialized).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn my_test() {
        use tracing_subscriber::prelude::*;
        let (writer, _guard) = ChromeLayer::with_writer(std::io::stdout);
        tracing_subscriber::registry().with(writer).init();
    }
}
