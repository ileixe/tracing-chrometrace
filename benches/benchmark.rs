#![allow(unused)]
use std::{marker::PhantomData, thread::{self, ThreadId}, time::{Duration, Instant, SystemTime}};

use tokio::runtime::Runtime;
use criterion::{criterion_group, criterion_main, Criterion, black_box};
use tracing::{info, Subscriber};
use tracing_chrome::{ChromeLayerBuilder, TraceStyle};
use tracing_chrometrace::ChromeLayer;
use tracing_subscriber::{prelude::*, Layer};
use tracing::field::valuable;


// #[tracing::instrument(target = "chrome_layer", fields(name = valuable(&"hello"), tid = valuable(&1)))]
#[tracing::instrument(target = "chrome_layer", fields(name = &"hello", tid = &1))]
fn hello() {}

fn only_span() {
	tracing::info_span!("span test");
}

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
    c.bench_function("instrument", |b| b.iter(|| hello()));
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
    c.bench_function("instrument", |b| b.iter(|| hello()));
}

fn chrometrace(c: &mut Criterion) {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::sink());

    let (writer, guard) = ChromeLayer::with_writer(non_blocking);

    tracing_subscriber::registry().with(writer).init();

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(|| hello()));
}

fn chrometrace_parallel(c: &mut Criterion) {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::sink());

    let (writer, guard) = ChromeLayer::with_writer(non_blocking);

    tracing_subscriber::registry().with(writer).init();

    std::thread::spawn(|| {
        loop {
            info!(target = "chrome_layer", name = "hello", tid = 2);
            std::thread::sleep(Duration::from_nanos(1));
        }
    });

    c.bench_function("info", |b| {
        b.iter(|| info!(target = "chrome_layer", name = "hello", tid = 1))
    });
    c.bench_function("instrument", |b| b.iter(|| hello()));
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
    c.bench_function("instrument", |b| b.iter(|| hello()));
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
            // let begin = SystemTime::now();
            only_span();
            // queue.push(Profile {
            //     dur: begin.elapsed().unwrap(),
            //     thread_id: std::thread::current().id(),
            // })
        })
    });
}

criterion_group!(
    benches,
    // fmt, /* 1 us */
    // chrome, /* 3.22 us */
    // emptylayer, /* 200 ns */
    // manual /* 77 ns */
    chrometrace, /* 2 us */
    // chrometrace_parallel /* info: 2.5, instrument: 3.9 us */
);
criterion_main!(benches);
