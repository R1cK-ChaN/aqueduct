use hl_types::Timestamp;

/// The cutoff for classifying a recorded trade by its exchange time
/// (HL-81 in `docs/venues/hyperliquid.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TradeCutoff {
    /// The connection start has yet to be observed. Every trade is live.
    #[default]
    UnknownStart,
    /// Trades at or after the connection's raw receipt wall time are live.
    ConnectedSince(Timestamp),
    /// The connection ended. Trades are suppressed until the next connection.
    Disconnected,
}

impl TradeCutoff {
    /// A `connected` event sets the cutoff to its raw receipt wall time.
    /// A disconnect or shutdown ends the connection. Other metadata,
    /// including `dropped`, preserves the current cutoff.
    pub fn observe_meta(&mut self, event: &str, recv_wall: Timestamp) {
        match event {
            "connected" => *self = Self::ConnectedSince(recv_wall),
            "disconnected" | "shutdown" => self.end_connection(),
            _ => {}
        }
    }

    /// Ends the connection, including at a recorder process change or truncation.
    pub fn end_connection(&mut self) {
        *self = Self::Disconnected;
    }

    /// Classifies the exchange time of one recorded trade.
    pub fn is_live(self, trade_time: Timestamp) -> bool {
        match self {
            Self::UnknownStart => true,
            Self::ConnectedSince(start) => trade_time >= start,
            Self::Disconnected => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trades_at_the_connection_time_are_live() {
        let before = Timestamp::from_nanos(999);
        let at = Timestamp::from_nanos(1_000);
        let after = Timestamp::from_nanos(1_001);
        let mut cutoff = TradeCutoff::default();
        assert!(cutoff.is_live(Timestamp::from_nanos(i64::MIN)));
        cutoff.observe_meta("connected", at);
        assert!(!cutoff.is_live(before));
        assert!(cutoff.is_live(at));
        assert!(cutoff.is_live(after));
        cutoff.end_connection();
        assert!(!cutoff.is_live(Timestamp::from_nanos(i64::MAX)));
    }

    #[test]
    fn recorder_events_update_the_cutoff_from_each_state() {
        let receipt = Timestamp::from_nanos(10);
        for initial in [
            TradeCutoff::UnknownStart,
            TradeCutoff::ConnectedSince(Timestamp::EPOCH),
            TradeCutoff::Disconnected,
        ] {
            for (event, expected) in [
                ("connected", TradeCutoff::ConnectedSince(receipt)),
                ("disconnected", TradeCutoff::Disconnected),
                ("shutdown", TradeCutoff::Disconnected),
                ("dropped", initial),
                ("file_open", initial),
                ("connecting", initial),
                ("connect_failed", initial),
            ] {
                let mut cutoff = initial;
                cutoff.observe_meta(event, receipt);
                assert_eq!(cutoff, expected, "{initial:?} / {event}");
            }
            let mut cutoff = initial;
            cutoff.end_connection();
            assert_eq!(cutoff, TradeCutoff::Disconnected);
        }
    }
}
