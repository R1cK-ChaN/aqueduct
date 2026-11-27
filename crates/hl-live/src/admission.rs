//! The only boundary at which source records enter the paper timeline.

use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use hl_codec::rawlog::{Record, RecordKind};
use hl_recorder::{Clock, SourceSink, Stamp};

#[derive(Debug, Clone)]
pub(crate) enum PaperClock {
    Real(Clock),
    #[cfg(test)]
    Controlled {
        anchor_ns: i64,
        stamps: Arc<Mutex<Stamp>>,
    },
}

impl PaperClock {
    #[cfg(test)]
    pub(crate) fn controlled(anchor_ns: i64, stamps: Arc<Mutex<Stamp>>) -> Self {
        Self::Controlled { anchor_ns, stamps }
    }

    pub(crate) fn start_wall_ns(&self) -> i64 {
        match self {
            Self::Real(clock) => clock.start_wall_ns(),
            #[cfg(test)]
            Self::Controlled { anchor_ns, .. } => *anchor_ns,
        }
    }

    pub(crate) fn now(&self) -> Stamp {
        match self {
            Self::Real(clock) => clock.now(),
            #[cfg(test)]
            Self::Controlled { stamps, .. } => *stamps.lock().expect("test clock poisoned"),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    records: Vec<Record>,
    data_count: usize,
    dropped: u64,
    failed: bool,
}

/// An admitted prefix and the clock sample taken under its admission lock.
#[derive(Debug)]
pub(crate) struct Prefix {
    pub records: Vec<Record>,
    pub sample: Stamp,
}

/// Shared admission state for one source and its scheduler.
#[derive(Debug)]
pub(crate) struct Admission {
    clock: PaperClock,
    capacity: usize,
    state: Mutex<State>,
    ready: Condvar,
}

impl Admission {
    pub(crate) fn new(clock: PaperClock, capacity: usize) -> Self {
        Self {
            clock,
            capacity,
            state: Mutex::default(),
            ready: Condvar::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_clock(clock: Arc<Mutex<Stamp>>, capacity: usize) -> Self {
        Self::new(PaperClock::controlled(0, clock), capacity)
    }

    pub(crate) fn sink(self: &Arc<Self>) -> AdmissionSink {
        AdmissionSink {
            admission: Arc::clone(self),
        }
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("admission lock poisoned"))
    }

    fn handoff(&self, state: &mut State) -> io::Result<Prefix> {
        if state.failed {
            return Err(io::Error::other("admission drop counter overflow"));
        }
        // Sampling and taking the entire prefix are inseparable. A source
        // paused after stamping still owns this same mutex.
        let sample = self.clock.now();
        let records = std::mem::take(&mut state.records);
        state.data_count = 0;
        Ok(Prefix { records, sample })
    }

    pub(crate) fn take_prefix(&self) -> io::Result<Prefix> {
        let mut state = self.lock()?;
        self.handoff(&mut state)
    }

    #[cfg(test)]
    pub(crate) fn try_take_prefix(&self) -> io::Result<Option<Prefix>> {
        match self.state.try_lock() {
            Ok(mut state) => self.handoff(&mut state).map(Some),
            Err(std::sync::TryLockError::WouldBlock) => Ok(None),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                Err(io::Error::other("admission lock poisoned"))
            }
        }
    }

    pub(crate) fn wait(&self, duration: Duration) -> io::Result<()> {
        let state = self.lock()?;
        // Checking under the admission mutex prevents a missed notification
        // between taking a prefix and beginning this wait.
        let (_guard, _timeout) = self
            .ready
            .wait_timeout_while(state, duration, |state| {
                state.records.is_empty() && !state.failed
            })
            .map_err(|_| io::Error::other("admission lock poisoned"))?;
        Ok(())
    }
}

/// A source sink holds the admission guard from stamp through enqueue.
#[derive(Debug)]
pub(crate) struct AdmissionSink {
    admission: Arc<Admission>,
}

fn record(kind: RecordKind, stamp: Stamp, payload: Vec<u8>) -> Record {
    Record {
        kind,
        recv_wall_ns: stamp.wall_ns,
        recv_mono_ns: stamp.mono_ns,
        payload,
    }
}

impl SourceSink for AdmissionSink {
    fn begin(&mut self, source: usize, kind: RecordKind) -> impl FnOnce(Vec<u8>) + '_ {
        assert_eq!(source, 0, "paper admits exactly one source");
        let admission = &self.admission;
        let mut state = admission.state.lock().expect("admission lock poisoned");
        let stamp = admission.clock.now();
        move |payload| {
            let is_data = kind != RecordKind::Meta;
            if is_data && state.data_count >= admission.capacity {
                match state.dropped.checked_add(1) {
                    Some(count) => state.dropped = count,
                    None => state.failed = true,
                }
            } else {
                // Preserve marker order while data slots are full. Publish
                // losses before the next accepted data item or shutdown.
                let shutdown = kind == RecordKind::Meta && payload == br#"{"event":"shutdown"}"#;
                if state.dropped > 0 && (is_data || shutdown) {
                    let payload =
                        format!(r#"{{"count":{},"event":"dropped"}}"#, state.dropped).into_bytes();
                    state.records.push(record(RecordKind::Meta, stamp, payload));
                    state.dropped = 0;
                }
                state.data_count += usize::from(is_data);
                state.records.push(record(kind, stamp, payload));
            }
            drop(state);
            admission.ready.notify_one();
        }
    }
}
