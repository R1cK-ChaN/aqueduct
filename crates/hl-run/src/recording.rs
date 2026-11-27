//! Convert raw records into receipt-ordered input and an inclusive replay stop.
//!
//! Each record has a raw wall time and an anchored process time under
//! RFC 0008 D4 in `docs/rfc/0008-paper-trading.md`. The trade cutoff follows
//! HL-81 in `docs/venues/hyperliquid.md`.

use std::fmt;

use hl_codec::hyperliquid::{
    Network, TradeCutoff, WsFrame, decode_ws_frame, instrument_id, market_events,
};
use hl_codec::rawlog::{Record, RecordKind};
use hl_types::{InstrumentId, MarketEvent, MarketEventKind, Timestamp};
use serde_json::Value;

/// Number of emitted gaps for each recorded cause.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GapCounts {
    /// `disconnected` metadata records.
    pub disconnected: u64,
    /// `shutdown` metadata records.
    pub shutdown: u64,
    /// `dropped` metadata records.
    pub dropped: u64,
    /// Boundaries between recorder processes, including legacy file boundaries.
    pub process_change: u64,
    /// Files ending inside a record or compressed frame.
    pub truncated: u64,
}

/// Counts accumulated while converting recordings into market events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaderStats {
    /// BBO events for the requested coin.
    pub bbo: u64,
    /// Trade events admitted by the connection cutoff.
    pub trades_live: u64,
    /// Emitted `FeedGap` events.
    pub feed_gaps: u64,
    /// Trades excluded by the connection cutoff.
    pub trades_replayed: u64,
    /// Text frames the codec could not decode.
    pub decode_errors: u64,
    /// Files ending inside a record or compressed frame.
    pub truncated_files: u64,
    /// Gap counts by cause.
    pub gaps: GapCounts,
    /// Events whose recorded receive time precedes their exchange time.
    pub negative_latency: u64,
    /// Events raised further to preserve the preceding event's local time.
    pub local_time_raised: u64,
    /// Events with an exchange time below an earlier event's exchange time.
    pub exchange_time_out_of_order: u64,
    /// Events whose process time exceeds both raw wall time and exchange time.
    pub process_clock_raised: u64,
    /// Largest such excess in nanoseconds, before applying the previous local time.
    pub process_clock_max_raise_ns: u64,
}

/// A raw record's location in the ordered input manifest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct RawPosition {
    /// Zero-based file index in the manifest.
    pub file_index: usize,
    /// Zero-based record index in that file, including `file_open` at zero.
    pub record_index: u64,
}

/// The recorded cause of an emitted feed gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapCause {
    /// The connection closed.
    Disconnected,
    /// The recorder shut down.
    Shutdown,
    /// The recorder dropped incoming frames.
    Dropped,
    /// The next file belongs to a different recorder process.
    ProcessChange,
    /// The file ended inside a record or compressed frame.
    Truncated,
}

impl GapCause {
    /// Stable session-file spelling of the cause.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Shutdown => "shutdown",
            Self::Dropped => "dropped",
            Self::ProcessChange => "process_change",
            Self::Truncated => "truncated",
        }
    }
}

/// Raw provenance carried alongside one converted market event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventOrigin {
    /// Position of the record that produced this event.
    pub position: RawPosition,
    /// Present exactly when the event is a feed gap.
    pub gap_cause: Option<GapCause>,
}

#[derive(Debug, Clone, Copy)]
struct Receipt {
    wall: Timestamp,
    process: Timestamp,
}

impl Receipt {
    fn new(wall_ns: i64, anchor_ns: i64, mono_ns: u64) -> Result<Self, ConversionError> {
        let process_ns = anchor_ns
            .checked_add_unsigned(mono_ns)
            .ok_or(ConversionError::ClockOverflow)?;
        Ok(Self {
            wall: Timestamp::from_nanos(wall_ns),
            process: Timestamp::from_nanos(process_ns),
        })
    }
}

#[derive(Debug)]
struct FileClock {
    anchor_ns: i64,
    last: Receipt,
}

/// An invalid recording clock, file boundary or metadata record.
#[derive(Debug)]
pub enum ConversionError {
    /// The metadata payload could not be parsed as JSON.
    InvalidMetadata(serde_json::Error),
    /// A second file header appeared among the records.
    UnexpectedFileOpen,
    /// A record or truncation was supplied before the first file header.
    MissingFileOpen,
    /// The process anchor or the anchored timestamp exceeds the timestamp range.
    ClockOverflow,
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadata(error) => write!(f, "invalid metadata JSON: {error}"),
            Self::UnexpectedFileOpen => f.write_str("unexpected additional file_open"),
            Self::MissingFileOpen => f.write_str("expected file_open before recording input"),
            Self::ClockOverflow => f.write_str("process clock overflow"),
        }
    }
}

impl std::error::Error for ConversionError {}

/// Converts one coin's records into receive-ordered market events.
///
/// The caller supplies file starts, complete records, process changes and
/// truncations in input order. Each event is appended to the caller's output
/// and never changes after emission. Its matching provenance is available
/// through [`Self::origins`] and can be drained with [`Self::take_origins`].
///
/// [`TradeCutoff`] classifies trades across connection and file boundaries.
/// The converter counts excluded trades and emits gaps for each [`GapCause`].
///
/// Local time is the maximum of raw wall time, the process anchor plus the
/// record's monotonic stamp, exchange time when present, and the preceding
/// event's local time. [`Self::stop_time`] includes complete records that
/// produce no market event, such as controls or another coin's frames.
#[derive(Debug)]
pub struct RecordConverter<'a> {
    coin: &'a str,
    instrument: InstrumentId,
    trade_cutoff: TradeCutoff,
    max_exchange: Option<Timestamp>,
    last_local: Option<Timestamp>,
    clock: Option<FileClock>,
    stats: ReaderStats,
    position: Option<RawPosition>,
    origins: Vec<EventOrigin>,
}

impl<'a> RecordConverter<'a> {
    /// Starts conversion for one network and coin with an unknown connection.
    pub fn new(network: Network, coin: &'a str) -> Self {
        Self {
            coin,
            instrument: instrument_id(network, coin),
            trade_cutoff: TradeCutoff::default(),
            max_exchange: None,
            last_local: None,
            clock: None,
            stats: ReaderStats::default(),
            position: None,
            origins: Vec::new(),
        }
    }

    /// Installs the header's process anchor and its two receipt stamps.
    ///
    /// For a legacy header without an anchor, subtract its monotonic stamp
    /// from its wall stamp. A change of chain emits a gap at this new header.
    ///
    /// # Errors
    ///
    /// Returns an error if the anchor or anchored time overflows.
    pub fn file_start(
        &mut self,
        recv_wall_ns: i64,
        recv_mono_ns: u64,
        process_start_wall_ns: Option<i64>,
        process_changed: bool,
        events: &mut Vec<MarketEvent>,
    ) -> Result<(), ConversionError> {
        let anchor_ns = match process_start_wall_ns {
            Some(anchor) => anchor,
            None => recv_wall_ns
                .checked_sub_unsigned(recv_mono_ns)
                .ok_or(ConversionError::ClockOverflow)?,
        };
        let last = Receipt::new(recv_wall_ns, anchor_ns, recv_mono_ns)?;
        self.position = Some(RawPosition {
            file_index: self.position.map_or(0, |position| position.file_index + 1),
            record_index: 0,
        });
        self.clock = Some(FileClock { anchor_ns, last });
        if process_changed {
            self.trade_cutoff.end_connection();
            self.gap(last, GapCause::ProcessChange, events);
        }
        Ok(())
    }

    /// Counts a truncated file and emits its gap at the last complete record.
    ///
    /// If only the header was complete, its receive time from
    /// [`Self::file_start`] is used instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the caller has yet to supply a complete header.
    pub fn truncated(&mut self, events: &mut Vec<MarketEvent>) -> Result<(), ConversionError> {
        let last = self
            .clock
            .as_ref()
            .ok_or(ConversionError::MissingFileOpen)?
            .last;
        self.stats.truncated_files += 1;
        self.trade_cutoff.end_connection();
        self.gap(last, GapCause::Truncated, events);
        Ok(())
    }

    /// Counters accumulated across all inputs supplied so far.
    pub fn stats(&self) -> &ReaderStats {
        &self.stats
    }

    /// Provenance in the same order as the emitted market events.
    pub fn origins(&self) -> &[EventOrigin] {
        &self.origins
    }

    /// Drains the emitted provenance for an incremental caller.
    pub fn take_origins(&mut self) -> Vec<EventOrigin> {
        std::mem::take(&mut self.origins)
    }

    /// Position of the last complete record, including a header-only file.
    pub fn last_position(&self) -> Option<RawPosition> {
        self.position
    }

    /// Local time of the final complete record, including a file header.
    ///
    /// Uses the later of its wall and process times, raised to the last
    /// emitted event's local time. Returns `None` before the first header.
    pub fn stop_time(&self) -> Option<Timestamp> {
        self.clock.as_ref().map(|clock| {
            let received = clock.last.wall.max(clock.last.process);
            self.last_local
                .map_or(received, |local| local.max(received))
        })
    }

    /// Earliest logical time covered by the just-converted raw record.
    ///
    /// Pass only the events emitted by this record. A frame containing trades
    /// on both sides of midnight belongs to the earlier checkpoint's extent.
    /// Headers and other records without events use their own receipt floors.
    pub fn checkpoint_record_time(&self, events: &[MarketEvent]) -> Option<Timestamp> {
        events
            .first()
            .map(|event| event.ts_local)
            .or_else(|| self.stop_time())
    }
}

impl RecordConverter<'_> {
    fn push(
        &mut self,
        mut event: MarketEvent,
        process_time: Timestamp,
        gap_cause: Option<GapCause>,
        events: &mut Vec<MarketEvent>,
    ) {
        let ts_exch = event.ts_exch;
        let mut ts_local = event.ts_local.max(ts_exch);
        let stats = &mut self.stats;
        if event.ts_local < ts_exch {
            stats.negative_latency += 1;
        }
        if process_time > ts_local {
            let raise_ns = process_time.as_nanos().abs_diff(ts_local.as_nanos());
            stats.process_clock_raised += 1;
            stats.process_clock_max_raise_ns = stats.process_clock_max_raise_ns.max(raise_ns);
            ts_local = process_time;
        }
        if let Some(previous) = self.last_local
            && ts_local < previous
        {
            ts_local = previous;
            stats.local_time_raised += 1;
        }
        if self.max_exchange.is_some_and(|maximum| ts_exch < maximum) {
            stats.exchange_time_out_of_order += 1;
        }
        self.max_exchange = Some(self.max_exchange.map_or(ts_exch, |time| time.max(ts_exch)));
        match event.kind {
            MarketEventKind::Bbo(_) => stats.bbo += 1,
            MarketEventKind::Trade(_) => stats.trades_live += 1,
            MarketEventKind::FeedGap => stats.feed_gaps += 1,
            _ => unreachable!("reader emits BBO, trades and gaps only"),
        }
        self.last_local = Some(ts_local);
        self.origins.push(EventOrigin {
            position: self.position.expect("converted input has a file header"),
            gap_cause,
        });
        event.ts_local = ts_local;
        events.push(event);
    }

    fn gap(&mut self, receipt: Receipt, cause: GapCause, events: &mut Vec<MarketEvent>) {
        let gaps = &mut self.stats.gaps;
        match cause {
            GapCause::Disconnected => gaps.disconnected += 1,
            GapCause::Shutdown => gaps.shutdown += 1,
            GapCause::Dropped => gaps.dropped += 1,
            GapCause::ProcessChange => gaps.process_change += 1,
            GapCause::Truncated => gaps.truncated += 1,
        }
        self.push(
            MarketEvent {
                instrument: self.instrument,
                ts_exch: receipt.wall,
                ts_local: receipt.wall,
                kind: MarketEventKind::FeedGap,
            },
            receipt.process,
            Some(cause),
            events,
        );
    }

    fn text_frame(&mut self, record: &Record, receipt: Receipt, events: &mut Vec<MarketEvent>) {
        let frame = match decode_ws_frame(&record.payload) {
            Ok(frame @ (WsFrame::Bbo(_) | WsFrame::Trades(_))) => frame,
            Ok(_) => return,
            Err(_) => {
                self.stats.decode_errors += 1;
                return;
            }
        };
        let mut converted = Vec::new();
        market_events(
            &frame,
            |coin| Ok((coin == self.coin).then_some(self.instrument)),
            receipt.wall,
            &mut converted,
        )
        .expect("the recording resolver only selects or skips coins");
        for event in converted {
            if matches!(event.kind, MarketEventKind::Trade(_))
                && !self.trade_cutoff.is_live(event.ts_exch)
            {
                self.stats.trades_replayed += 1;
                continue;
            }
            self.push(event, receipt.process, None, events);
        }
    }

    /// Converts one complete record and appends the events it produces.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing header, a process clock overflow,
    /// invalid metadata JSON or an additional `file_open` record.
    /// File headers must be supplied through [`Self::file_start`].
    pub fn record(
        &mut self,
        record: &Record,
        events: &mut Vec<MarketEvent>,
    ) -> Result<(), ConversionError> {
        let anchor_ns = self
            .clock
            .as_ref()
            .ok_or(ConversionError::MissingFileOpen)?
            .anchor_ns;
        let receipt = Receipt::new(record.recv_wall_ns, anchor_ns, record.recv_mono_ns)?;
        self.position
            .as_mut()
            .expect("a file clock also has a raw position")
            .record_index += 1;
        match record.kind {
            RecordKind::Meta => {
                let meta: Value = serde_json::from_slice(&record.payload)
                    .map_err(ConversionError::InvalidMetadata)?;
                let event = meta
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.trade_cutoff.observe_meta(event, receipt.wall);
                match event {
                    "disconnected" => self.gap(receipt, GapCause::Disconnected, events),
                    "shutdown" => self.gap(receipt, GapCause::Shutdown, events),
                    "dropped" => self.gap(receipt, GapCause::Dropped, events),
                    "file_open" => return Err(ConversionError::UnexpectedFileOpen),
                    _ => {}
                }
            }
            RecordKind::RecvText => self.text_frame(record, receipt, events),
            RecordKind::RecvBinary | RecordKind::SentText => {}
        }
        self.clock = Some(FileClock {
            anchor_ns,
            last: receipt,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hl_types::Bbo;

    fn bbo(received: i64, exchange: i64) -> MarketEvent {
        MarketEvent {
            instrument: instrument_id(Network::Mainnet, "BTC"),
            ts_exch: Timestamp::from_nanos(exchange),
            ts_local: Timestamp::from_nanos(received),
            kind: MarketEventKind::Bbo(Bbo::default()),
        }
    }

    fn ignored_record(recv_wall_ns: i64) -> Record {
        Record {
            kind: RecordKind::SentText,
            recv_wall_ns,
            recv_mono_ns: 0,
            payload: Vec::new(),
        }
    }

    #[test]
    fn stop_time_uses_the_final_complete_record_even_without_an_event() {
        let mut reader = RecordConverter::new(Network::Mainnet, "BTC");
        let mut events = Vec::new();
        assert_eq!(reader.stop_time(), None);
        reader
            .file_start(10, 0, Some(10), false, &mut events)
            .unwrap();
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(10)));
        reader.push(bbo(20, 30), Timestamp::from_nanos(10), None, &mut events);
        reader.record(&ignored_record(25), &mut events).unwrap();
        // The last event was raised to max(receive 20, exchange 30)=30.
        // A final ignored record at 25 therefore stops at max(25,30)=30.
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(30)));
        reader.record(&ignored_record(40), &mut events).unwrap();
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(40)));
        // The preceding event is still at 30. The final record has
        // wall=35 and P=10+0, giving max(35,10,30)=35.
        reader.record(&ignored_record(35), &mut events).unwrap();
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(35)));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn header_only_and_truncated_recordings_keep_a_reproducible_stop() {
        let mut reader = RecordConverter::new(Network::Mainnet, "BTC");
        let mut events = Vec::new();
        reader
            .file_start(10, 0, Some(10), false, &mut events)
            .unwrap();
        reader.truncated(&mut events).unwrap();
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(10)));
        reader
            .file_start(5, 0, Some(5), false, &mut events)
            .unwrap();
        // The new header has wall=P=5. The preceding gap at 10 raises
        // its stop to max(5,5,10)=10.
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(10)));
        reader.record(&ignored_record(15), &mut events).unwrap();
        reader.truncated(&mut events).unwrap();
        assert_eq!(reader.stop_time(), Some(Timestamp::from_nanos(15)));
        assert_eq!(events.last().unwrap().ts_local, Timestamp::from_nanos(15));
    }

    #[test]
    fn time_counters_compare_against_the_full_exchange_history() {
        let mut reader = RecordConverter::new(Network::Mainnet, "BTC");
        let mut events = Vec::new();
        reader
            .file_start(0, 0, Some(0), false, &mut events)
            .unwrap();
        for (received, exchange) in [(10, 30), (12, 10), (15, 20), (35, 35)] {
            reader.push(
                bbo(received, exchange),
                Timestamp::from_nanos(0),
                None,
                &mut events,
            );
        }
        assert_eq!(reader.stats.negative_latency, 2);
        assert_eq!(reader.stats.local_time_raised, 2);
        assert_eq!(reader.stats.exchange_time_out_of_order, 2);
        let local_times: Vec<_> = events
            .iter()
            .map(|event| event.ts_local.as_nanos())
            .collect();
        assert_eq!(local_times, [30, 30, 30, 35]);
    }

    #[test]
    fn origins_count_headers_ignored_records_and_truncation_in_manifest_order() {
        let mut reader = RecordConverter::new(Network::Mainnet, "BTC");
        let mut events = Vec::new();
        reader
            .file_start(0, 0, Some(0), false, &mut events)
            .unwrap();
        reader.record(&ignored_record(1), &mut events).unwrap();
        reader
            .record(
                &Record {
                    kind: RecordKind::Meta,
                    recv_wall_ns: 2,
                    recv_mono_ns: 0,
                    payload: br#"{"event":"dropped","count":1}"#.to_vec(),
                },
                &mut events,
            )
            .unwrap();
        assert_eq!(
            reader.take_origins(),
            [EventOrigin {
                position: RawPosition {
                    file_index: 0,
                    record_index: 2,
                },
                gap_cause: Some(GapCause::Dropped),
            }]
        );
        assert!(reader.origins().is_empty());

        reader.file_start(3, 0, Some(3), true, &mut events).unwrap();
        reader.record(&ignored_record(4), &mut events).unwrap();
        reader.truncated(&mut events).unwrap();
        assert_eq!(
            reader.origins(),
            [
                EventOrigin {
                    position: RawPosition {
                        file_index: 1,
                        record_index: 0,
                    },
                    gap_cause: Some(GapCause::ProcessChange),
                },
                EventOrigin {
                    position: RawPosition {
                        file_index: 1,
                        record_index: 1,
                    },
                    gap_cause: Some(GapCause::Truncated),
                },
            ]
        );
        assert_eq!(
            reader.last_position(),
            Some(RawPosition {
                file_index: 1,
                record_index: 1,
            })
        );
        assert_eq!(events.len(), 3);
    }
}
