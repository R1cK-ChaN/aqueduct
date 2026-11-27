//! Host-side measurements and wake-up hints. Only the shared driver releases work.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::time::Duration;

use hl_sim::SimConfig;
use hl_strategy::{Actions, Context, Strategy};
use hl_types::{Event, MarketEventKind, Timestamp};

use crate::admission::PaperClock;

const MINUTE_NS: i64 = 60_000_000_000;

/// Minimize the time around the strategy call. The trading strategy receives
/// only its ordinary context, event and action buffer.
pub(crate) struct Observed<'a, S> {
    pub strategy: S,
    pub clock: PaperClock,
    pub sim: SimConfig,
    pub observation: &'a RefCell<Observation>,
}

impl<S: Strategy> Strategy for Observed<'_, S> {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        let market = matches!(event, Event::Market(_));
        let begin = market.then(|| self.clock.now().mono_ns);
        self.strategy.on_event(ctx, event, actions);
        let end = market.then(|| self.clock.now().mono_ns);

        let mut obs = self.observation.borrow_mut();
        if let (Event::Market(input), Some(begin), Some(end)) = (event, begin, end) {
            obs.sample(input.ts_local, begin, end);
            if !matches!(input.kind, MarketEventKind::FeedGap) {
                // Proof at R can make any older outcome due immediately at R.
                obs.wakeups.insert(input.ts_local);
            }
            if matches!(input.kind, MarketEventKind::Trade(_)) {
                obs.wake_after(input.ts_exch, self.sim.report_latency);
            }
        }
        // D2 report times are maxima of R, X + report_latency and an earlier
        // report's due time. These candidates include every determined due
        // time. A candidate for an unproved action merely causes a resample.
        if !actions.order_actions().is_empty() {
            obs.wake_after(ctx.now(), self.sim.order_latency + self.sim.report_latency);
        }
        for timer in actions.timer_requests() {
            obs.wakeups.insert(timer.at.max(ctx.now()));
        }
    }
}

#[derive(Default)]
pub(crate) struct Observation {
    pub admitted_mono_ns: u64,
    next_input: u64,
    minute: Option<Minute>,
    completed: Vec<Minute>,
    wakeups: BTreeSet<Timestamp>,
}

impl Observation {
    fn wake_after(&mut self, time: Timestamp, delay: Duration) {
        // An unproved order may never produce a report. Only representable
        // candidate times can require a wakeup.
        if let Ok(ns) = u64::try_from(delay.as_nanos())
            && let Some(due) = time.as_nanos().checked_add_unsigned(ns)
        {
            self.wakeups.insert(Timestamp::from_nanos(due));
        }
    }

    pub fn next_wakeup(&mut self, closed: Option<Timestamp>) -> Option<Timestamp> {
        while self.wakeups.first().is_some_and(|t| Some(*t) <= closed) {
            self.wakeups.pop_first();
        }
        self.wakeups.first().copied()
    }

    fn sample(&mut self, time: Timestamp, begin: u64, end: u64) {
        let minute = time.as_nanos().div_euclid(MINUTE_NS);
        if self.minute.as_ref().is_some_and(|m| m.index != minute) {
            self.finish_minute();
        }
        let ordinal = self.next_input;
        self.next_input += 1;
        let lag = begin - self.admitted_mono_ns;
        let duration = end - begin;
        let m = self.minute.get_or_insert(Minute {
            index: minute,
            first: ordinal,
            last: ordinal,
            lag: (lag, ordinal),
            duration: (duration, ordinal),
        });
        m.last = ordinal;
        if lag > m.lag.0 {
            m.lag = (lag, ordinal);
        }
        if duration > m.duration.0 {
            m.duration = (duration, ordinal);
        }
    }

    pub fn drain(&mut self, closed: Option<Timestamp>) -> impl Iterator<Item = String> + use<> {
        if let (Some(m), Some(t)) = (&self.minute, closed)
            && i128::from(t.as_nanos()) + 1 >= (i128::from(m.index) + 1) * i128::from(MINUTE_NS)
        {
            self.finish_minute();
        }
        // Sample memory as the writer consumes each completed observation.
        std::mem::take(&mut self.completed)
            .into_iter()
            .map(Minute::line)
    }

    pub fn finish_minute(&mut self) {
        if let Some(minute) = self.minute.take() {
            self.completed.push(minute);
        }
    }
}

struct Minute {
    index: i64,
    first: u64,
    last: u64,
    lag: (u64, u64),
    duration: (u64, u64),
}

impl Minute {
    fn line(self) -> String {
        // Every interpolated field is an integer. Keep the session's field
        // order and its distinctions between sequence strings and counts.
        format!(
            concat!(
                "{{\"kind\":\"diagnostic\",\"minute_start_ns\":\"{}\",\"callbacks\":{},",
                "\"first_input\":\"{}\",\"last_input\":\"{}\",",
                "\"max_lag_ns\":\"{}\",\"max_lag_input\":\"{}\",",
                "\"max_duration_ns\":\"{}\",\"max_duration_input\":\"{}\",",
                "\"resident_memory_bytes\":{}}}\n"
            ),
            i128::from(self.index) * i128::from(MINUTE_NS),
            self.last - self.first + 1,
            self.first,
            self.last,
            self.lag.0,
            self.lag.1,
            self.duration.0,
            self.duration.1,
            resident_memory_bytes().map_or_else(|| "null".to_owned(), |bytes| bytes.to_string())
        )
    }
}

fn resident_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_resident_memory(&status)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_resident_memory(status: &str) -> Option<u64> {
    let rss = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    let kib: u64 = rss.trim().strip_suffix("kB")?.trim().parse().ok()?;
    kib.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_resident_memory_scales_vmrss_to_bytes() {
        assert_eq!(
            parse_resident_memory("VmHWM:\t999 kB\nVmRSS:\t123 kB\n"),
            Some(125_952)
        );
        assert_eq!(parse_resident_memory("VmHWM:\t999 kB\n"), None);
        assert_eq!(
            parse_resident_memory("VmRSS: 18446744073709551615 kB"),
            None
        );
    }

    #[test]
    fn minute_maxima_keep_the_first_input_on_ties_and_flush_once() {
        let mut obs = Observation {
            admitted_mono_ns: 1,
            ..Observation::default()
        };
        obs.sample(Timestamp::from_nanos(0), 10, 12);
        obs.admitted_mono_ns = 4;
        obs.sample(Timestamp::from_nanos(1), 13, 18);
        assert!(
            obs.drain(Some(Timestamp::from_nanos(MINUTE_NS - 2)))
                .next()
                .is_none()
        );
        let rows: Vec<_> = obs
            .drain(Some(Timestamp::from_nanos(MINUTE_NS - 1)))
            .collect();
        assert_eq!(rows.len(), 1);
        let mut row: serde_json::Value = serde_json::from_str(&rows[0]).expect("diagnostic row");
        let resident = row
            .as_object_mut()
            .unwrap()
            .remove("resident_memory_bytes")
            .unwrap();
        if cfg!(target_os = "linux")
            && std::fs::read_to_string("/proc/self/status")
                .is_ok_and(|status| status.lines().any(|line| line.starts_with("VmRSS:")))
        {
            assert!(
                resident
                    .as_u64()
                    .is_some_and(|bytes| bytes > 0 && bytes % 1024 == 0)
            );
        } else {
            assert!(resident.is_null());
        }
        assert_eq!(
            row,
            serde_json::json!({
                "kind":"diagnostic", "minute_start_ns":"0", "callbacks":2,
                "first_input":"0", "last_input":"1",
                "max_lag_ns":"9", "max_lag_input":"0",
                "max_duration_ns":"5", "max_duration_input":"1"
            })
        );
        obs.sample(Timestamp::from_nanos(MINUTE_NS * 2), 20, 21);
        obs.finish_minute();
        let rows: Vec<_> = obs.drain(None).collect();
        assert_eq!(rows.len(), 1);
        let row: serde_json::Value = serde_json::from_str(&rows[0]).expect("final minute");
        assert_eq!(row["first_input"], "2");
        assert_eq!(row["minute_start_ns"], (MINUTE_NS * 2).to_string());
    }
}
