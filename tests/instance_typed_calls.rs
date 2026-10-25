mod common;

use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::Result;
use composable_runtime::{ComponentInstance, ComponentState, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, ComponentType, FutureConsumer, FutureReader, Instance, Lift, Source, StreamConsumer,
    StreamReader, StreamResult, TypedFunc,
};

const EVENTS: &str = "modulewise:test/events@0.1.0";

// Exports `modulewise:test/events` with `watch: async func(count: u32) ->
// output`, where `output` is a record of a `stream<event>` and a
// `future<result<summary, string>>`. `watch` returns both, then writes events
// `n` = 1 to `count` to the stream after returning. Each write is synchronous,
// so it completes only once the reader takes the event, and the writing stops
// early if the reader drops the stream. Then the future gets `ok` with the
// number of events written.
fn events_wasm() -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (core module $Memory (memory (export "mem") 1))
            (core instance $memory (instantiate $Memory))
            (core module $m
                (import "" "mem" (memory 1))
                (import "" "task.return" (func $task.return (param i32 i32)))
                (import "" "stream.new" (func $stream.new (result i64)))
                (import "" "stream.write" (func $stream.write (param i32 i32 i32) (result i32)))
                (import "" "stream.drop-writable" (func $stream.drop-writable (param i32)))
                (import "" "future.new" (func $future.new (result i64)))
                (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
                (import "" "future.drop-writable" (func $future.drop-writable (param i32)))

                (func (export "watch") (param $count i32) (result i32)
                    (local $pair i64) (local $ret i32) (local $written i32)
                    (local $events-rx i32) (local $events-tx i32)
                    (local $summary-rx i32) (local $summary-tx i32)

                    (local.set $pair (call $stream.new))
                    (local.set $events-rx (i32.wrap_i64 (local.get $pair)))
                    (local.set $events-tx (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))
                    (local.set $pair (call $future.new))
                    (local.set $summary-rx (i32.wrap_i64 (local.get $pair)))
                    (local.set $summary-tx (i32.wrap_i64 (i64.shr_u (local.get $pair) (i64.const 32))))

                    (call $task.return (local.get $events-rx) (local.get $summary-rx))

                    (block $done
                        (loop $write
                            (br_if $done (i32.ge_u (local.get $written) (local.get $count)))
                            (i32.store (i32.const 16) (i32.add (local.get $written) (i32.const 1)))
                            (local.set $ret
                                (call $stream.write (local.get $events-tx) (i32.const 16) (i32.const 1)))
                            ;; The count of items written is in the upper bits.
                            (local.set $written
                                (i32.add (local.get $written) (i32.shr_u (local.get $ret) (i32.const 4))))
                            ;; DROPPED (1): the reader is gone.
                            (br_if $done (i32.eq (i32.and (local.get $ret) (i32.const 0xf)) (i32.const 1)))
                            (br $write)
                        )
                    )
                    (call $stream.drop-writable (local.get $events-tx))

                    ;; ok({{ written }})
                    (i32.store8 (i32.const 32) (i32.const 0))
                    (i32.store (i32.const 36) (local.get $written))
                    (drop (call $future.write (local.get $summary-tx) (i32.const 32)))
                    (call $future.drop-writable (local.get $summary-tx))

                    (i32.const 0) ;; EXIT
                )
                (func (export "watch-callback") (param i32 i32 i32) (result i32)
                    unreachable
                )
            )
            (type $event (record (field "n" u32)))
            (type $summary (record (field "written" u32)))
            (type $events (stream $event))
            (type $summary-future (future (result $summary (error string))))
            (type $output (record (field "events" $events) (field "summary" $summary-future)))
            (core func $task.return (canon task.return (result $output) (memory (core memory $memory "mem"))))
            (core func $stream.new (canon stream.new $events))
            (core func $stream.write (canon stream.write $events (memory (core memory $memory "mem"))))
            (core func $stream.drop-writable (canon stream.drop-writable $events))
            (core func $future.new (canon future.new $summary-future))
            (core func $future.write (canon future.write $summary-future (memory (core memory $memory "mem"))))
            (core func $future.drop-writable (canon future.drop-writable $summary-future))
            (core instance $i (instantiate $m
                (with "" (instance
                    (export "mem" (memory $memory "mem"))
                    (export "task.return" (func $task.return))
                    (export "stream.new" (func $stream.new))
                    (export "stream.write" (func $stream.write))
                    (export "stream.drop-writable" (func $stream.drop-writable))
                    (export "future.new" (func $future.new))
                    (export "future.write" (func $future.write))
                    (export "future.drop-writable" (func $future.drop-writable))
                ))
            ))
            (func $watch async (param "count" u32) (result $output)
                (canon lift (core func $i "watch") async
                    (memory (core memory $memory "mem"))
                    (callback (core func $i "watch-callback"))
                )
            )
            (instance $instance
                (export "event" (type $event))
                (export "summary" (type $summary))
                (export "output" (type $output))
                (export "watch" (func $watch))
            )
            (export "{EVENTS}" (instance $instance))
        )
        "#
    ))
}

#[derive(ComponentType, Lift, Debug, PartialEq)]
#[component(record)]
struct Event {
    n: u32,
}

#[derive(ComponentType, Lift, Debug, PartialEq)]
#[component(record)]
struct Summary {
    written: u32,
}

#[derive(ComponentType, Lift)]
#[component(record)]
struct Output {
    events: StreamReader<Event>,
    summary: FutureReader<Result<Summary, String>>,
}

// Takes one event per free slot in a bounded channel. While the channel is
// full it returns `Pending`, so the component's write stays pending.
struct EventSink(PollSender<Event>);

impl<D> StreamConsumer<D> for EventSink {
    type Item = Event;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        mut source: Source<'_, Event>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        match self.0.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                let mut event = None;
                source.read(store, &mut event)?;
                if let Some(event) = event
                    && self.0.send_item(event).is_err()
                {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

// Hands the future's value to a oneshot.
struct SummarySlot(Option<oneshot::Sender<Result<Summary, String>>>);

impl<D> FutureConsumer<D> for SummarySlot {
    type Item = Result<Summary, String>;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        mut source: Source<'_, Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let mut value = None;
        source.read(store, &mut value)?;
        if let (Some(value), Some(sender)) = (value, self.0.take()) {
            let _ = sender.send(value);
        }
        Poll::Ready(Ok(()))
    }
}

async fn runtime(toml: &str) -> Runtime {
    let toml_file = common::create_toml_test_file(toml);
    Runtime::builder()
        .from_path(&*toml_file)
        .build()
        .await
        .expect("runtime")
}

// Instantiates `events` through the host, as a service would.
async fn instantiate_events(events: &common::TestFile) -> (Runtime, ComponentInstance) {
    let runtime = runtime(&format!(
        r#"
        [component.events]
        uri = "{}"
        "#,
        events.display()
    ))
    .await;
    let instance = runtime
        .host()
        .instantiate("events", None)
        .await
        .expect("instance");
    (runtime, instance)
}

// `watch`, looked up inside its interface.
fn watch_func(
    accessor: &Accessor<ComponentState>,
    instance: Instance,
) -> Result<TypedFunc<(u32,), (Output,)>> {
    accessor.with(|mut store| {
        let interface = instance
            .get_export_index(&mut store, None, EVENTS)
            .ok_or_else(|| anyhow::anyhow!("no {EVENTS} export"))?;
        let watch = instance
            .get_export_index(&mut store, Some(&interface), "watch")
            .ok_or_else(|| anyhow::anyhow!("no watch function"))?;
        Ok(instance.get_typed_func::<(u32,), (Output,)>(&mut store, &watch)?)
    })
}

/// The receiving end of the summary's oneshot.
type SummaryReceiver = oneshot::Receiver<Result<Summary, String>>;

// Pipes `output` into a channel holding at most `capacity` events, and a
// oneshot for the summary.
fn pipe(
    accessor: &Accessor<ComponentState>,
    output: Output,
    capacity: usize,
) -> Result<(mpsc::Receiver<Event>, SummaryReceiver)> {
    let (events_tx, events_rx) = mpsc::channel(capacity);
    let (summary_tx, summary_rx) = oneshot::channel();
    accessor.with(|mut store| -> Result<()> {
        output
            .events
            .pipe(&mut store, EventSink(PollSender::new(events_tx)))?;
        output
            .summary
            .pipe(&mut store, SummarySlot(Some(summary_tx)))?;
        Ok(())
    })?;
    Ok((events_rx, summary_rx))
}

#[tokio::test]
async fn events_written_after_the_export_returns_are_read_as_they_arrive() {
    let events = events_wasm();
    let (_runtime, mut instance) = instantiate_events(&events).await;

    let (received, summary) = instance
        .run_concurrent(async |accessor, instance| -> Result<_> {
            let (output,) = watch_func(accessor, instance)?
                .call_concurrent(accessor, (3,))
                .await?;
            let (mut events_rx, summary_rx) = pipe(accessor, output, 1)?;
            // The loop drives the component only while this closure runs, so
            // read to the stream's end and the future's value here.
            let mut received = Vec::new();
            while let Some(event) = events_rx.recv().await {
                received.push(event);
            }
            Ok((received, summary_rx.await?))
        })
        .await
        .expect("run_concurrent")
        .expect("watch");

    assert_eq!(
        received,
        vec![Event { n: 1 }, Event { n: 2 }, Event { n: 3 }]
    );
    assert_eq!(summary, Ok(Summary { written: 3 }));
}

#[tokio::test]
async fn a_bounded_consumer_applies_backpressure_to_stream_writes() {
    let events = events_wasm();
    let (_runtime, mut instance) = instantiate_events(&events).await;

    let (first, summary) = instance
        .run_concurrent(async |accessor, instance| -> Result<_> {
            let (output,) = watch_func(accessor, instance)?
                .call_concurrent(accessor, (100,))
                .await?;
            let (mut events_rx, summary_rx) = pipe(accessor, output, 1)?;
            let first = events_rx.recv().await;
            // Stop reading: the component's next write returns DROPPED.
            drop(events_rx);
            Ok((first, summary_rx.await?))
        })
        .await
        .expect("run_concurrent")
        .expect("watch");

    assert_eq!(first, Some(Event { n: 1 }));
    // The channel holds one event, so by the time the reader went away the
    // component could have written the event read and at most one more. An
    // unbounded reader would have let it write all 100.
    let written = summary.expect("ok").written;
    assert!(written <= 2, "written: {written}");
}
