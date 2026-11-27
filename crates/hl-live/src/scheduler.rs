//! Real-time pacing around the shared deterministic driver.

use std::io;
use std::time::Duration;

use hl_codec::rawlog::Record;
use hl_recorder::Stamp;
use hl_run::IncrementalDriver;
use hl_strategy::Strategy;
use hl_types::Timestamp;

/// What the writer/converter learned from one admitted record.
#[derive(Debug, Default)]
pub(crate) struct ProcessedRecord {
    pub latest_input: Option<Timestamp>,
    pub shutdown: bool,
}

/// Tracks evidence that closes timestamps without changing decision times.
#[derive(Debug)]
pub(crate) struct Pacing {
    anchor: i64,
    latest_input: Option<Timestamp>,
    closed_through: Option<Timestamp>,
}

impl Pacing {
    pub(crate) fn new(anchor: i64) -> Self {
        Self {
            anchor,
            latest_input: None,
            closed_through: None,
        }
    }

    pub(crate) fn closed_through(&self) -> Option<Timestamp> {
        self.closed_through
    }

    pub(crate) fn anchored(&self, sample: Stamp) -> io::Result<Timestamp> {
        self.anchor
            .checked_add_unsigned(sample.mono_ns)
            .map(Timestamp::from_nanos)
            .ok_or_else(|| io::Error::other("process clock overflow"))
    }

    pub(crate) fn process_records<S: Strategy>(
        &mut self,
        records: Vec<Record>,
        driver: &mut IncrementalDriver<'_, S>,
        mut process: impl FnMut(&mut IncrementalDriver<'_, S>, Record) -> io::Result<ProcessedRecord>,
    ) -> io::Result<bool> {
        let mut shutdown = false;
        for record in records {
            if shutdown {
                return Err(io::Error::other("record admitted after shutdown"));
            }
            let processed = process(driver, record)?;
            if let Some(time) = processed.latest_input {
                self.latest_input = Some(time);
                // IncrementalDriver closes every strictly earlier time when
                // given this input, including quiet intervals.
                if let Some(before) = time.as_nanos().checked_sub(1) {
                    let before = Timestamp::from_nanos(before);
                    self.closed_through =
                        Some(self.closed_through.map_or(before, |old| old.max(before)));
                }
            }
            shutdown = processed.shutdown;
        }
        Ok(shutdown)
    }

    pub(crate) fn close<S: Strategy>(
        &mut self,
        driver: &mut IncrementalDriver<'_, S>,
        anchored_sample: Timestamp,
    ) -> io::Result<()> {
        let Some(before_sample) = anchored_sample.as_nanos().checked_sub(1) else {
            return Ok(());
        };
        let time = Timestamp::from_nanos(before_sample);
        if self.latest_input.is_some_and(|latest| time < latest)
            || self.closed_through.is_some_and(|closed| time <= closed)
        {
            return Ok(());
        }
        driver.close_through(time).map_err(io::Error::other)?;
        self.closed_through = Some(time);
        Ok(())
    }

    pub(crate) fn wait_duration(
        &self,
        sample: Stamp,
        next_deadline: Option<Timestamp>,
        maintenance_remaining: Duration,
    ) -> io::Result<Duration> {
        let now = self.anchored(sample)?.as_nanos();
        let Some(deadline) = next_deadline else {
            return Ok(maintenance_remaining);
        };
        let deadline = self
            .latest_input
            .map_or(deadline, |latest| deadline.max(latest));
        // Sample strictly after the deadline. An equal nanosecond remains
        // open, and a forward wall-clock step may put the input ahead of us.
        let nanos = (i128::from(deadline.as_nanos()) + 1 - i128::from(now)).max(1);
        let nanos = u64::try_from(nanos).unwrap_or(u64::MAX);
        Ok(Duration::from_nanos(nanos).min(maintenance_remaining))
    }
}
