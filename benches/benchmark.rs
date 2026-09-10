#![allow(unused)]
use std::{
    marker::PhantomData,
    thread::{self, ThreadId},
    time::{Duration, Instant, SystemTime},
};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tokio::runtime::Runtime;
use tracing::{info, Subscriber};
use tracing_chrome::{ChromeLayerBuilder, TraceStyle};
use tracing_chrometrace::ChromeLayer;
use tracing_subscriber::{prelude::*, Layer};

#[tracing::instrument(target = "chrome_layer", fields(name = "hello", tid = 1))]
fn hello() {}

fn fmt(c: &mut Criterion) {
    let format = tracing_subscriber::fmt::format()
        .without_time()
        .with_target(false)
        .with_level(false)
        .with_ansi(false)
        .compact();

    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::sink());

    let fmt = tracing_subscriber::fmt::Layer::default()
        .event_format(format)
        .with_writer(non_blocking);

    tracing_subscriber::registry().with(fmt).init();

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(hello));
}

fn chrome(c: &mut Criterion) {
    let (chrome, _guard) = ChromeLayerBuilder::new()
        .include_args(true)
        .trace_style(TraceStyle::Async)
        .build();

    tracing_subscriber::registry().with(chrome).init();

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(hello));
}

fn chrometrace(c: &mut Criterion) {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::sink());

    let (writer, guard) = ChromeLayer::with_writer(non_blocking);

    tracing_subscriber::registry().with(writer).init();

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(hello));
}

fn chrometrace_parallel(c: &mut Criterion) {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::sink());

    let (writer, guard) = ChromeLayer::with_writer(non_blocking);

    tracing_subscriber::registry().with(writer).init();

    std::thread::spawn(|| loop {
        info!(target = "chrome_layer", name = "hello", tid = 2);
        std::thread::sleep(Duration::from_nanos(1));
    });

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(hello));
}

// Needs no subscriber, so it measures the pieces without the once-per-process `init`.
fn pieces(c: &mut Criterion) {
    c.bench_function("process_id", |b| b.iter(|| black_box(std::process::id())));
    c.bench_function("instant_now", |b| b.iter(|| black_box(Instant::now())));
    c.bench_function("alloc_a_name", |b| {
        b.iter(|| black_box(String::from("hello")))
    });
}

fn chrometrace_sink(c: &mut Criterion) {
    let (writer, guard) = ChromeLayer::with_writer(std::io::sink);

    tracing_subscriber::registry().with(writer).init();

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(hello));
}

/// What a device backend emits: a span naming its own phase and carrying the cycle counts the
/// device reported, so nothing is derived from the clock.
fn npu(c: &mut Criterion) {
    // The writer is held out of the measurement, so that what moves is the layer.
    let (writer, _trace) = ChromeLayer::with_writer(std::io::sink);

    tracing_subscriber::registry().with(writer).init();

    c.bench_function("launch", |b| {
        b.iter(|| {
            let span = tracing::info_span!(
                "NPU",
                cat = "NPU",
                name = "Task",
                ph = "Complete",
                ts = 2012429,
                dur = 7306,
                begin_cycle = 2012429u64,
                end_cycle = 2019735u64,
            );
            let _entered = span.enter();
        })
    });
}

fn emptylayer(c: &mut Criterion) {
    struct EmptyLayer<S> {
        _inner: PhantomData<S>,
    };

    impl<S> Layer<S> for EmptyLayer<S> where S: Subscriber {}

    let empty = EmptyLayer {
        _inner: PhantomData,
    };

    tracing_subscriber::registry().with(empty).init();

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(hello));
}

fn manual(c: &mut Criterion) {
    struct Profile {
        dur: Duration,
        thread_id: ThreadId,
    }

    let queue: crossbeam_queue::SegQueue<Profile> = Default::default();

    fn hello() {}

    c.bench_function("instrument", move |b| {
        b.iter(|| {
            let begin = SystemTime::now();
            hello();
            queue.push(Profile {
                dur: begin.elapsed().unwrap(),
                thread_id: std::thread::current().id(),
            })
        })
    });
}

criterion_group!(
    benches,
    // One is picked at a time, because a subscriber is installed once per process.
    // fmt, /* 1.03 us */
    // chrome, /* 3.22 us */
    // emptylayer, /* 6.7 ns an event, 72.7 ns a span */
    // manual, /* 77 ns */
    // chrometrace, /* 203 ns an event, 575 ns a span */
    npu
);
criterion_main!(benches);
