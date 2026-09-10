#![doc = include_str!("../README.md")]

use std::borrow::Cow;
use std::fmt::Debug;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum::{AsRefStr, EnumString};
use tracing::field::{Field, Visit};
use tracing::{span, Event, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// The `ph` of a Chrome Trace event. Both spellings parse, the name below and the letter the
/// format writes, so a caller may say either `"Complete"` or `"X"`.
#[derive(
    Debug, Copy, Clone, Default, PartialEq, Eq, EnumString, AsRefStr, Serialize, Deserialize,
)]
pub enum Phase {
    #[strum(to_string = "B", serialize = "DurationBegin")]
    #[serde(rename = "B")]
    DurationBegin,
    #[strum(to_string = "E", serialize = "DurationEnd")]
    #[serde(rename = "E")]
    DurationEnd,
    #[strum(to_string = "X", serialize = "Complete")]
    #[serde(rename = "X")]
    Complete,
    #[default]
    #[strum(to_string = "i", serialize = "Instant")]
    #[serde(rename = "i")]
    Instant,
    #[strum(to_string = "C", serialize = "Counter")]
    #[serde(rename = "C")]
    Counter,
    #[strum(to_string = "b", serialize = "AsyncStart")]
    #[serde(rename = "b")]
    AsyncStart,
    #[strum(to_string = "n", serialize = "AsyncInstant")]
    #[serde(rename = "n")]
    AsyncInstant,
    #[strum(to_string = "e", serialize = "AsyncEnd")]
    #[serde(rename = "e")]
    AsyncEnd,
    #[strum(to_string = "s", serialize = "FlowStart")]
    #[serde(rename = "s")]
    FlowStart,
    #[strum(to_string = "t", serialize = "FlowStep")]
    #[serde(rename = "t")]
    FlowStep,
    #[strum(to_string = "f", serialize = "FlowEnd")]
    #[serde(rename = "f")]
    FlowEnd,
    #[strum(to_string = "p", serialize = "Sample")]
    #[serde(rename = "p")]
    Sample,
    #[strum(to_string = "N", serialize = "ObjectCreated")]
    #[serde(rename = "N")]
    ObjectCreated,
    #[strum(to_string = "O", serialize = "ObjectSnapshot")]
    #[serde(rename = "O")]
    ObjectSnapshot,
    #[strum(to_string = "D", serialize = "ObjectDestroyed")]
    #[serde(rename = "D")]
    ObjectDestroyed,
    #[strum(to_string = "M", serialize = "Metadata")]
    #[serde(rename = "M")]
    Metadata,
    #[strum(to_string = "V", serialize = "MemoryDumpGlobal")]
    #[serde(rename = "V")]
    MemoryDumpGlobal,
    #[strum(to_string = "v", serialize = "MemoryDumpProcess")]
    #[serde(rename = "v")]
    MemoryDumpProcess,
    #[strum(to_string = "R", serialize = "Mark")]
    #[serde(rename = "R")]
    Mark,
    #[strum(to_string = "c", serialize = "ClockSync")]
    #[serde(rename = "c")]
    ClockSync,
    #[strum(to_string = "(", serialize = "ContextBegin")]
    #[serde(rename = "(")]
    ContextBegin,
    #[strum(to_string = ")", serialize = "ContextEnd")]
    #[serde(rename = ")")]
    ContextEnd,
}

/// The arguments an event carries, in the order they were recorded. The format writes an object,
/// and a map would cost a table on the first insert, so the pairs are kept and written as one.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Args(pub Vec<(Cow<'static, str>, Value)>);

impl Args {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value recorded under a name, if the event carried one.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(recorded, _)| recorded == name)
            .map(|(_, value)| value)
    }

    fn push(&mut self, name: &'static str, value: Value) {
        self.0.push((Cow::Borrowed(name), value));
    }
}

impl Serialize for Args {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(name, value)| (name, value)))
    }
}

impl<'de> Deserialize<'de> for Args {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Pairs;

        impl<'de> serde::de::Visitor<'de> for Pairs {
            type Value = Args;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of arguments")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(pair) = map.next_entry()? {
                    pairs.push(pair);
                }
                Ok(Args(pairs))
            }
        }

        deserializer.deserialize_map(Pairs)
    }
}

/// One Chrome Trace event, as the format serialises it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromeEvent {
    pub name: Cow<'static, str>,
    pub cat: Cow<'static, str>,
    pub ph: Phase,
    pub ts: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts: Option<f64>,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub id: Cow<'static, str>,
    pub pid: u64,
    pub tid: u64,
    #[serde(default, skip_serializing_if = "Args::is_empty")]
    pub args: Args,
}

/// The reporting process. Read once, because `getpid` is a call and an event is meant to cost
/// a fraction of the microsecond launch it reports on.
static PID: LazyLock<u64> = LazyLock::new(|| std::process::id().into());

/// One number per thread, read from the `Debug` spelling ("ThreadId(42)") because
/// `ThreadId::as_u64` is unstable; a counter keeps the promise if that spelling changes.
fn thread_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    thread_local! {
        static ID: u64 = format!("{:?}", std::thread::current().id())
            .strip_prefix("ThreadId(")
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|digits| digits.parse().ok())
            .unwrap_or_else(|| NEXT.fetch_add(1, Ordering::Relaxed));
    }
    ID.with(|id| *id)
}

/// What a span or event said. Each is optional because the caller chooses which the format takes
/// from it and which it derives.
#[derive(Debug, Default, Clone)]
struct Fields {
    name: Option<String>,
    cat: Option<String>,
    ph: Option<Phase>,
    ts: Option<f64>,
    dur: Option<f64>,
    tts: Option<f64>,
    id: Option<String>,
    pid: Option<u64>,
    tid: Option<u64>,
    args: Args,
    /// Whether an enter already opened the span, so that a span polled again opens once.
    opened: bool,
    /// `event = "async"` asks for the asynchronous pair rather than the duration pair.
    asynchronous: bool,
}

impl Fields {
    fn text(&mut self, name: &'static str, value: &str) {
        match name {
            "name" => self.name = Some(value.to_owned()),
            "cat" => self.cat = Some(value.to_owned()),
            "id" => self.id = Some(value.to_owned()),
            "ph" => self.ph = Phase::from_str(value).ok(),
            "event" => self.asynchronous = value == "async",
            _ => self.args.push(name, Value::from(value)),
        }
    }

    /// Reports whether the name is one the event declares, so that a caller holding a narrower
    /// type can keep it rather than widen an integer into a float.
    fn number(&mut self, name: &str, value: f64) -> bool {
        match name {
            "ts" => self.ts = Some(value),
            "dur" => self.dur = Some(value),
            "tts" => self.tts = Some(value),
            "pid" => self.pid = Some(value as u64),
            "tid" => self.tid = Some(value as u64),
            _ => return false,
        }
        true
    }

    fn take(&mut self, name: &'static str, value: Value) {
        match &value {
            Value::String(text) => {
                let text = text.clone();
                self.text(name, &text);
            }
            Value::Number(number) => match number.as_f64() {
                Some(as_float) if self.number(name, as_float) => {}
                _ => self.args.push(name, value),
            },
            _ => self.args.push(name, value),
        }
    }

    /// The event this reports at `ph`, timed against `start` when the caller named no `ts`.
    fn event(self, ph: Phase, start: Instant) -> ChromeEvent {
        ChromeEvent {
            name: self.name.map(Cow::Owned).unwrap_or_default(),
            cat: self.cat.map(Cow::Owned).unwrap_or_default(),
            ph,
            ts: self
                .ts
                .unwrap_or_else(|| start.elapsed().as_nanos() as f64 / 1000.0),
            dur: self.dur,
            tts: self.tts,
            id: self.id.map(Cow::Owned).unwrap_or_default(),
            pid: self.pid.unwrap_or(*PID),
            tid: self.tid.unwrap_or_else(thread_id),
            args: self.args,
        }
    }

    /// The pair a span reports when it names no phase; a named phase is a whole event, reported
    /// once at close.
    fn pair(&self) -> Option<(Phase, Phase)> {
        let pair = if self.asynchronous {
            (Phase::AsyncStart, Phase::AsyncEnd)
        } else {
            (Phase::DurationBegin, Phase::DurationEnd)
        };
        self.ph.is_none().then_some(pair)
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.text(field.name(), value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if !self.number(field.name(), value) {
            self.args.push(field.name(), Value::from(value));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !self.number(field.name(), value as f64) {
            self.args.push(field.name(), Value::from(value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !self.number(field.name(), value as f64) {
            self.args.push(field.name(), Value::from(value));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.take(field.name(), Value::from(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.text(field.name(), format!("{value:?}").trim_matches('"'));
    }
}

/// The events recorded but not yet written. A profiler formats late: an event costs a push here,
/// and the serialising happens a batch at a time, off the path being measured.
/// Writes the event as the format reads it. Written out rather than derived because a profile put
/// a fifth of the recording cost in the generic serialiser, for a shape that is fixed and known.
/// `agrees_with_the_derived_serialiser` holds the two to the same output.
fn write_event(line: &mut Vec<u8>, event: &ChromeEvent) {
    line.extend_from_slice(b"{\"name\":");
    write_text(line, &event.name);
    line.extend_from_slice(b",\"cat\":");
    write_text(line, &event.cat);
    line.extend_from_slice(b",\"ph\":\"");
    line.extend_from_slice(event.ph.as_ref().as_bytes());
    line.extend_from_slice(b"\",\"ts\":");
    write_number(line, event.ts);
    if let Some(dur) = event.dur {
        line.extend_from_slice(b",\"dur\":");
        write_number(line, dur);
    }
    if let Some(tts) = event.tts {
        line.extend_from_slice(b",\"tts\":");
        write_number(line, tts);
    }
    if !event.id.is_empty() {
        line.extend_from_slice(b",\"id\":");
        write_text(line, &event.id);
    }
    line.extend_from_slice(b",\"pid\":");
    write_integer(line, event.pid);
    line.extend_from_slice(b",\"tid\":");
    write_integer(line, event.tid);
    if !event.args.is_empty() {
        line.extend_from_slice(b",\"args\":{");
        for (at, (name, value)) in event.args.0.iter().enumerate() {
            if at > 0 {
                line.push(b',');
            }
            write_text(line, name);
            line.push(b':');
            write_value(line, value);
        }
        line.push(b'}');
    }
    line.push(b'}');
}

/// A device reports counts, so an argument is written the same way the event's own fields are,
/// and only a nested value is left to the derived serialiser.
fn write_value(line: &mut Vec<u8>, value: &Value) {
    match value {
        Value::String(text) => write_text(line, text),
        Value::Number(number) => match number.as_u64() {
            Some(whole) => write_integer(line, whole),
            None => write_number(line, number.as_f64().unwrap_or(f64::NAN)),
        },
        Value::Bool(set) => line.extend_from_slice(if *set { b"true" } else { b"false" }),
        Value::Null => line.extend_from_slice(b"null"),
        nested => serde_json::to_writer(line, nested).expect("a recorded value serialises"),
    }
}

/// A name or value that needs no escaping, which is the usual one, is copied straight through.
fn write_text(line: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    if bytes
        .iter()
        .all(|byte| *byte >= 0x20 && *byte != b'"' && *byte != b'\\' && *byte < 0x7f)
    {
        line.push(b'"');
        line.extend_from_slice(bytes);
        line.push(b'"');
    } else {
        serde_json::to_writer(line, text).expect("a string serialises");
    }
}

/// A digit at a time costs a division each; `itoa` takes two at a time from a table, and it is
/// already in the tree because the derived serialiser formats its own numbers with it.
fn write_integer(line: &mut Vec<u8>, value: u64) {
    let mut digits = itoa::Buffer::new();
    line.extend_from_slice(digits.format(value).as_bytes());
}

/// A trace stamps microseconds taken from integer nanoseconds, so a value is whole or exact to a
/// thousandth, and neither spelling needs the shortest-float search that formatting would run.
/// Anything else still goes the long way.
fn write_number(line: &mut Vec<u8>, value: f64) {
    const ROOM: std::ops::Range<f64> = 0.0..9.0e15;

    if !value.is_finite() {
        line.extend_from_slice(b"null");
        return;
    }
    if value.fract() == 0.0 && ROOM.contains(&value) {
        write_integer(line, value as u64);
        return;
    }
    let thousandths = value * 1000.0;
    if thousandths.fract() == 0.0 && ROOM.contains(&thousandths) {
        let scaled = thousandths as u64;
        write_integer(line, scaled / 1000);
        let mut left = scaled % 1000;
        if left > 0 {
            let mut digits = [b'0'; 3];
            for digit in digits.iter_mut().rev() {
                *digit = b'0' + (left % 10) as u8;
                left /= 10;
            }
            let last = digits.iter().rposition(|digit| *digit != b'0');
            line.push(b'.');
            line.extend_from_slice(&digits[..=last.expect("a non-zero remainder has a digit")]);
        }
        return;
    }
    let _ = write!(line, "{value}");
}

#[derive(Debug)]
struct Sink<W> {
    make_writer: W,
    line: Mutex<Line>,
}

/// The text of the events recorded since the last write, and whether one has been written yet,
/// which is what decides the separator.
#[derive(Debug, Default)]
struct Line {
    bytes: Vec<u8>,
    started: bool,
}

/// Bytes a line holds before it is written. Larger trades memory for fewer writer calls.
const FLUSH: usize = 64 * 1024;

impl<W> Sink<W>
where
    W: for<'writer> MakeWriter<'writer>,
{
    fn record(&self, event: ChromeEvent) {
        let Ok(mut line) = self.line.lock() else {
            return;
        };
        if std::mem::replace(&mut line.started, true) {
            line.bytes.extend_from_slice(b",\n");
        }
        write_event(&mut line.bytes, &event);
        if line.bytes.len() >= FLUSH {
            self.flush(&mut line);
        }
    }

    fn flush(&self, line: &mut Line) {
        if line.bytes.is_empty() {
            return;
        }
        // A layer reports; it does not decide that a full disk ends the process.
        let _ = self.make_writer.make_writer().write_all(&line.bytes);
        line.bytes.clear();
    }

    fn write(&self) {
        if let Ok(mut line) = self.line.lock() {
            self.flush(&mut line);
        }
    }
}

/// The open trace. Dropping it writes what is pending and closes the array.
#[derive(Debug)]
pub struct Trace<W>(Arc<Sink<W>>)
where
    W: for<'writer> MakeWriter<'writer>;

impl<W> Drop for Trace<W>
where
    W: for<'writer> MakeWriter<'writer>,
{
    fn drop(&mut self) {
        self.0.write();
        let _ = self.0.make_writer.make_writer().write_all(b"\n]\n");
    }
}

#[derive(Debug)]
pub struct ChromeLayer<S, W = fn() -> io::Stdout> {
    start: Instant,
    sink: Arc<Sink<W>>,
    _subscriber: PhantomData<S>,
}

impl<S, W> ChromeLayer<S, W>
where
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    /// The layer and the trace it writes. The trace is complete once that value drops.
    pub fn with_writer(make_writer: W) -> (Self, Trace<W>) {
        let _ = make_writer.make_writer().write_all(b"[\n");
        let sink = Arc::new(Sink {
            make_writer,
            line: Mutex::new(Line {
                // Room past the mark, so that the event crossing it needs no growth.
                bytes: Vec::with_capacity(FLUSH * 2),
                started: false,
            }),
        });
        (
            Self {
                start: Instant::now(),
                sink: Arc::clone(&sink),
                _subscriber: PhantomData,
            },
            Trace(sink),
        )
    }

    /// When the layer started, for a caller stamping its own `ts` against the same origin.
    pub fn start(&self) -> SystemTime {
        SystemTime::now() - self.start.elapsed()
    }
}

impl<S, W> Layer<S> for ChromeLayer<S, W>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut fields = Fields::default();
            attrs.record(&mut fields);
            span.extensions_mut().insert(fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let ph = fields.ph.unwrap_or(Phase::Instant);
        self.sink.record(fields.event(ph, self.start));
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let opening = {
            let mut extensions = span.extensions_mut();
            let Some(fields) = extensions.get_mut::<Fields>() else {
                return;
            };
            // A span re-entered on every poll, as an async span is, opens once.
            if std::mem::replace(&mut fields.opened, true) {
                return;
            }
            let Some((begin, _)) = fields.pair() else {
                return;
            };
            // The pair times itself, so the opening event keeps the fields for the close to
            // report, and the span is left unlocked before anything is written.
            fields.clone().event(begin, self.start)
        };
        self.sink.record(opening);
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let closing = span.extensions_mut().remove::<Fields>();
        let Some(fields) = closing else {
            return;
        };
        let ph = match fields.pair() {
            Some((_, end)) => end,
            None => fields.ph.expect("a span without a pair named its phase"),
        };
        self.sink.record(fields.event(ph, self.start));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(named: &[(&'static str, Value)]) -> Fields {
        let mut fields = Fields::default();
        for (name, value) in named {
            fields.take(name, value.clone());
        }
        fields
    }

    #[test]
    fn parses_phase_by_name_or_letter() {
        assert_eq!(Phase::from_str("Complete").unwrap(), Phase::Complete);
        assert_eq!(Phase::from_str("X").unwrap(), Phase::Complete);
        assert_eq!(Phase::from_str("b").unwrap(), Phase::AsyncStart);
    }

    #[test]
    fn drops_an_unknown_phase() {
        assert_eq!(fields(&[("ph", "nonsense".into())]).ph, None);
    }

    #[test]
    fn reports_once_for_a_named_phase() {
        assert_eq!(fields(&[("ph", "X".into())]).pair(), None);
    }

    #[test]
    fn reports_a_pair_otherwise() {
        assert_eq!(
            fields(&[]).pair(),
            Some((Phase::DurationBegin, Phase::DurationEnd)),
        );
    }

    #[test]
    fn pairs_an_async_span() {
        assert_eq!(
            fields(&[("event", "async".into())]).pair(),
            Some((Phase::AsyncStart, Phase::AsyncEnd)),
        );
    }

    #[test]
    fn keeps_a_named_timestamp() {
        let event =
            fields(&[("ts", 17.into()), ("dur", 5.into())]).event(Phase::Complete, Instant::now());
        assert_eq!(event.ts, 17.0);
        assert_eq!(event.dur, Some(5.0));
    }

    #[test]
    fn keeps_unnamed_fields_as_args() {
        let event = fields(&[("begin_cycle", 42.into())]).event(Phase::Complete, Instant::now());
        assert_eq!(
            event.args.0,
            [(Cow::Borrowed("begin_cycle"), Value::from(42))]
        );
    }

    /// `Visit` pushes a field at a time and `Deserialize` pulls one, so nothing derives the
    /// mapping below from the event; this is what keeps the two from drifting apart.
    /// A whole number is written without the trailing `.0` the derived serialiser keeps, which is
    /// the same number to a reader and is what lets a timestamp skip float formatting. Every other
    /// difference still has to show.
    fn as_numbers(value: Value) -> Value {
        match value {
            Value::Number(number) => number
                .as_f64()
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Null, Value::Number),
            Value::Array(values) => Value::Array(values.into_iter().map(as_numbers).collect()),
            Value::Object(entries) => Value::Object(
                entries
                    .into_iter()
                    .map(|(name, value)| (name, as_numbers(value)))
                    .collect(),
            ),
            other => other,
        }
    }

    #[test]
    fn agrees_with_the_derived_serialiser() {
        let awkward = [
            ("name", Value::from("a \"quoted\" \\ name\n")),
            ("cat", Value::from("NPU")),
            ("ts", Value::from(2012429)),
            ("dur", Value::from(7306.5)),
            ("tts", Value::from(2012429.317)),
            ("id", Value::from("7")),
            ("pid", Value::from(1903372)),
            ("tid", Value::from(2)),
            ("begin_cycle", Value::from(2012429)),
            ("a string arg", Value::from("plain")),
            ("a real arg", Value::from(0.25)),
            ("a thousandth", Value::from(0.05)),
            ("a tenth", Value::from(0.5)),
            ("a long real", Value::from(1.0 / 3.0)),
            ("a bool arg", Value::from(true)),
            ("a negative arg", Value::from(-7)),
            ("a nested arg", serde_json::json!({ "slice": [1, 2] })),
        ];
        for take in [awkward.len(), 3, 0] {
            let event = fields(&awkward[..take]).event(Phase::Complete, Instant::now());

            let mut hand = Vec::new();
            write_event(&mut hand, &event);

            assert_eq!(
                as_numbers(serde_json::from_slice(&hand).expect("the hand written bytes are JSON")),
                as_numbers(serde_json::to_value(&event).unwrap()),
                "with {take} fields set",
            );
        }
    }

    #[test]
    fn writes_arguments_as_an_object() {
        let event =
            fields(&[("begin_cycle", Value::from(42))]).event(Phase::Complete, Instant::now());
        let json = serde_json::to_value(&event).unwrap();

        // The pairs are held in a list to keep them cheap to record, but the format reads an object.
        assert_eq!(json["args"], serde_json::json!({ "begin_cycle": 42 }));
    }

    #[test]
    fn recognises_every_field_the_event_declares() {
        // Every field is named, so adding one to the event fails to build until it is probed too.
        let populated = ChromeEvent {
            name: "n".into(),
            cat: "c".into(),
            ph: Phase::Complete,
            ts: 1.0,
            dur: Some(1.0),
            tts: Some(1.0),
            id: "1".into(),
            pid: 1,
            tid: 1,
            args: Args::default(),
        };
        let json = serde_json::to_value(&populated).unwrap();
        for (key, value) in json.as_object().unwrap() {
            if key == "args" {
                continue;
            }
            // A field is probed with its own kind of value, which is how one actually arrives.
            let probe = match value {
                Value::String(_) => Value::from("Complete"),
                _ => Value::from(0),
            };
            let mut fields = Fields::default();
            fields.take(key.clone().leak(), probe);
            assert!(
                fields.args.is_empty(),
                "`{key}` is a field of the event and arrived as an argument",
            );
        }
    }

    #[test]
    fn survives_a_round_trip() {
        let event = fields(&[
            ("name", "Task".into()),
            ("cat", "NPU".into()),
            ("ts", 1.into()),
            // An unrecognised field, so that the arguments are carried through the trip too.
            ("begin_cycle", 2012429.into()),
        ])
        .event(Phase::Complete, Instant::now());
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<ChromeEvent>(&json).unwrap(), event);
    }
}
