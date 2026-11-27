//! Records raw WebSocket frames to hourly compressed files.
//!
//! The recorder stores every frame exactly as received, with a wall-clock
//! and a monotonic receive timestamp, and never parses a payload. Venue
//! details (URL, subscription messages, ping text) come from the
//! configuration file. The optional [`public_v1`] send policy checks them
//! against the rules for that dataset version. The file format and design are
//! in `docs/rfc/0002-raw-recording-format.md`.
//!
//! Threads: one per source reads its socket and pushes frames into a bounded
//! queue. The thread that called [`run`] drains the queue and writes. A full
//! queue drops frames and counts them. A terminal policy rejection waits
//! for queue space so the writer can record the reason for stopping.
//!
//! The public dataset configuration is specified in
//! `docs/rfc/0010-public-recording-dataset.md` D2 and D3.

mod clock;
mod config;
mod inspect;
mod log;
pub mod public_v1;
mod source;
mod writer;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

pub use clock::{Clock, Stamp, UtcHour};
pub use config::{Config, ConfigError, SYMBOL_PLACEHOLDER, SourceConfig};
pub use hl_codec::rawlog::RecordKind;
pub use inspect::{InspectReport, inspect, open_recording};
pub use source::{SourceSink, run_source};
pub use writer::{FINISHED_SUFFIX, LogWriter, PARTIAL_SUFFIX, SourceInfo, TRUNCATED_SUFFIX};

/// How often open files are flushed and drop counts are written.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// One stamped record on its way to the writer.
#[derive(Debug)]
struct Item {
    source: usize,
    kind: RecordKind,
    stamp: Stamp,
    payload: Vec<u8>,
}

struct RecorderSink {
    clock: Clock,
    queue: SyncSender<Item>,
    /// Items dropped by this source since the writer last checked.
    dropped: Arc<AtomicU64>,
}

impl SourceSink for RecorderSink {
    fn begin(&mut self, source: usize, kind: RecordKind) -> impl FnOnce(Vec<u8>) + '_ {
        let stamp = self.clock.now();
        move |payload| {
            let item = Item {
                source,
                kind,
                stamp,
                payload,
            };
            match self.queue.try_send(item) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
                // The writer is gone and the recorder is stopping.
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    fn terminal_meta(&mut self, source: usize, payload: Vec<u8>) {
        // The source has stopped sending. The writer either drains this record
        // before sealing the file or disconnects the queue on a write error.
        let _ = self.queue.send(Item {
            source,
            kind: RecordKind::Meta,
            stamp: self.clock.now(),
            payload,
        });
    }
}

fn install_tls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Another source may install the process-wide provider concurrently.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Records until `shutdown` becomes true or a send policy stops its source.
///
/// A rejected send returns an error after the queue drains and files are
/// finished. On a write error, open files keep their `.part` suffix.
pub fn run(config: &Config, shutdown: &Arc<AtomicBool>) -> io::Result<()> {
    if config.sources.iter().any(|s| s.send_policy().is_some()) {
        // Library callers can construct or change a Config after parsing it.
        config
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    }
    record_sources(config, shutdown, source::run_source_checked)
}

fn record_sources(
    config: &Config,
    shutdown: &Arc<AtomicBool>,
    read_source: fn(usize, &SourceConfig, &AtomicBool, &mut RecorderSink) -> io::Result<()>,
) -> io::Result<()> {
    install_tls_provider();

    let clock = Clock::start();
    let infos = config
        .sources
        .iter()
        .map(|source| SourceInfo {
            name: source.name.clone(),
            url: source.url.clone(),
        })
        .collect();
    let (mut writer, recovered) = LogWriter::new(
        &config.out_dir,
        config.zstd_level,
        clock.start_wall_ns(),
        infos,
    )?;
    for path in recovered {
        log::warn("recovered_partial_file", json!({ "path": path }));
    }

    let (queue, received) = sync_channel::<Item>(config.queue_capacity);
    let dropped: Vec<Arc<AtomicU64>> = config
        .sources
        .iter()
        .map(|_| Arc::new(AtomicU64::new(0)))
        .collect();
    let mut threads = Vec::new();
    for (index, source) in config.sources.iter().enumerate() {
        let source = source.clone();
        let shutdown = Arc::clone(shutdown);
        let mut sink = RecorderSink {
            clock,
            queue: queue.clone(),
            dropped: Arc::clone(&dropped[index]),
        };
        let thread = thread::Builder::new()
            .name(format!("source-{}", source.name))
            .spawn(move || read_source(index, &source, &shutdown, &mut sink))?;
        threads.push(thread);
    }
    // Only the source threads hold senders now, so the queue disconnects
    // when the last of them exits.
    drop(queue);
    log::info(
        "started",
        json!({ "sources": config.sources.len(), "out_dir": config.out_dir }),
    );

    let mut result = write_until_disconnected(&mut writer, &received, &dropped, &clock);
    if let Err(error) = &result {
        log::error("write_failed", json!({ "error": error.to_string() }));
        shutdown.store(true, Ordering::Relaxed);
    }
    drop(received);
    for (source, thread) in config.sources.iter().zip(threads) {
        match thread.join() {
            Ok(source_result) => result = result.and(source_result),
            Err(_) if source.send_policy().is_some() => {
                result = result.and(Err(io::Error::other("policy source panicked")));
            }
            Err(_) => {}
        }
    }
    log::info("stopped", json!({ "clean": result.is_ok() }));
    result
}

fn log_sealed(writer: &mut LogWriter) {
    for path in writer.take_sealed() {
        log::info("file_sealed", json!({ "path": path }));
    }
}

/// Writes one `dropped` record per source that lost frames since the last
/// call.
fn write_drop_counts(
    writer: &mut LogWriter,
    dropped: &[Arc<AtomicU64>],
    stamp: Stamp,
) -> io::Result<()> {
    for (source, counter) in dropped.iter().enumerate() {
        let count = counter.swap(0, Ordering::Relaxed);
        if count > 0 {
            let record = json!({ "event": "dropped", "count": count });
            writer.write(
                source,
                RecordKind::Meta,
                stamp,
                record.to_string().as_bytes(),
            )?;
            log::warn("dropped", json!({ "source": source, "count": count }));
        }
    }
    Ok(())
}

fn write_until_disconnected(
    writer: &mut LogWriter,
    received: &Receiver<Item>,
    dropped: &[Arc<AtomicU64>],
    clock: &Clock,
) -> io::Result<()> {
    let mut last_flush = Instant::now();
    loop {
        let wait = FLUSH_INTERVAL.saturating_sub(last_flush.elapsed());
        match received.recv_timeout(wait) {
            Ok(item) => writer.write(item.source, item.kind, item.stamp, &item.payload)?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_flush.elapsed() >= FLUSH_INTERVAL {
            last_flush = Instant::now();
            let now = clock.now();
            write_drop_counts(writer, dropped, now)?;
            writer.tick(now.wall_ns)?;
            log_sealed(writer);
        }
    }
    write_drop_counts(writer, dropped, clock.now())?;
    writer.finish()?;
    log_sealed(writer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_revalidates_policy_sources_before_opening_the_output() {
        let mut config = Config::from_toml(
            r#"
            out_dir = "unused"
            [[source]]
            name = "hl-public"
            send_policy = "hl-public-v1"
            url = "wss://api.hyperliquid.xyz/ws"
            ping_text = '{"method":"ping"}'
            subscribe_once = ['{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}']
            "#,
        )
        .unwrap();
        config.out_dir =
            std::env::temp_dir().join(format!("hl-recorder-invalid-policy-{}", std::process::id()));
        let shutdown = Arc::new(AtomicBool::new(true));
        config.sources.push(config.sources[0].clone());
        let error = run(&config, &shutdown).unwrap_err();
        assert!(error.to_string().contains("exactly one [[source]]"));

        config.sources.truncate(1);
        config.sources[0].url = "ws://127.0.0.1:1/ws".into();
        let error = run(&config, &shutdown).unwrap_err();
        assert!(error.to_string().contains("url"));
        assert!(!config.out_dir.exists());
    }

    #[test]
    fn terminal_meta_survives_a_full_recorder_queue() {
        let (queue, received) = sync_channel(1);
        let mut sink = RecorderSink {
            clock: Clock::start(),
            queue,
            dropped: Default::default(),
        };
        sink.begin(0, RecordKind::RecvText)(b"earlier frame".to_vec());
        let (done, finished) = std::sync::mpsc::channel();
        let terminal = br#"{"event":"send_policy_rejected"}"#;
        let producer = thread::spawn(move || {
            sink.terminal_meta(0, terminal.to_vec());
            done.send(()).unwrap();
        });
        assert_eq!(
            finished.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        );
        assert_eq!(received.recv().unwrap().payload, b"earlier frame");
        let item = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(item.kind, RecordKind::Meta);
        assert_eq!(item.payload, terminal);
        producer.join().unwrap();
    }

    #[test]
    fn a_full_queue_drops_the_frame_and_counts_it() {
        let (queue, received) = sync_channel(2);
        let (finished, completion) = std::sync::mpsc::channel();
        let producer = thread::spawn(move || {
            let mut sink = RecorderSink {
                clock: Clock::start(),
                queue,
                dropped: Default::default(),
            };
            for payload in [b"a", b"b", b"c", b"d"] {
                sink.begin(3, RecordKind::RecvText)(payload.to_vec());
            }
            finished.send(sink).expect("test is waiting");
        });
        let mut context = completion
            .recv_timeout(Duration::from_secs(5))
            .expect("producer finishes while the queue is still full");
        producer.join().expect("producer thread");

        assert_eq!(context.dropped.load(Ordering::Relaxed), 2);
        let kept: Vec<Item> = received.try_iter().collect();
        assert_eq!(kept.len(), 2);
        assert_eq!(
            (kept[0].source, kept[0].payload.as_slice()),
            (3, b"a".as_slice())
        );
        assert_eq!(kept[1].payload, b"b");

        drop(received);
        context.begin(3, RecordKind::RecvText)(b"e".to_vec());
        assert_eq!(context.dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn drop_counts_are_written_into_the_data_and_reset() {
        let dir = std::env::temp_dir().join(format!("hl-recorder-drops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let infos = ["a", "b"]
            .map(|name| SourceInfo {
                name: name.to_owned(),
                url: "ws://test".to_owned(),
            })
            .to_vec();
        let (mut writer, _) = LogWriter::new(&dir, 3, 0, infos).unwrap();
        let dropped = [Arc::new(AtomicU64::new(5)), Arc::new(AtomicU64::new(0))];
        let stamp = Stamp {
            wall_ns: 1_795_514_400_000_000_000,
            mono_ns: 1,
        };

        write_drop_counts(&mut writer, &dropped, stamp).unwrap();
        write_drop_counts(&mut writer, &dropped, stamp).unwrap();
        writer.finish().unwrap();

        assert_eq!(dropped[0].load(Ordering::Relaxed), 0);
        let sealed = writer.take_sealed();
        assert_eq!(
            sealed.len(),
            1,
            "a source that dropped nothing gets no file"
        );
        let payloads: Vec<String> = open_recording(&sealed[0])
            .unwrap()
            .map(|record| String::from_utf8(record.unwrap().payload).unwrap())
            .collect();
        assert_eq!(payloads.len(), 2, "file header and one drop record");
        assert_eq!(payloads[1], r#"{"count":5,"event":"dropped"}"#);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
