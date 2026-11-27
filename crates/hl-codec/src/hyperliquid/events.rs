//! From venue records to `hl-types` market events.

use core::fmt;

use hl_types::{Bbo, InstrumentId, L2Snapshot, MarketEvent, MarketEventKind, Timestamp, Trade};

use super::ws::WsFrame;

/// The caller's resolver rejected a coin in the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCoin {
    /// The coin name as the venue wrote it.
    pub coin: String,
}

impl fmt::Display for UnknownCoin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "coin {} is not in the instrument registry", self.coin)
    }
}

impl std::error::Error for UnknownCoin {}

/// Appends the market events `frame` stands for to `out`.
///
/// Book, diff and BBO frames give one event; a trades frame gives one event
/// per trade, in the venue's order; acknowledgements, pongs, errors and
/// undecoded channels give none (HL-45, HL-46). `ts_exch` is the block time in the frame (HL-95) and
/// `ts_local` is the receive time the caller passes in.
///
/// `resolve` supplies the id for each selected coin. `Ok(None)` skips that
/// coin. On `Err`, the whole frame is discarded and `out` retains its
/// original contents.
pub fn market_events(
    frame: &WsFrame,
    resolve: impl Fn(&str) -> Result<Option<InstrumentId>, UnknownCoin>,
    ts_local: Timestamp,
    out: &mut Vec<MarketEvent>,
) -> Result<(), UnknownCoin> {
    let start = out.len();
    let mut append = |coin: &str, ts_exch: Timestamp, kind: MarketEventKind| {
        if let Some(instrument) = resolve(coin)? {
            out.push(MarketEvent {
                instrument,
                ts_exch,
                ts_local,
                kind,
            });
        }
        Ok(())
    };

    let result = match frame {
        WsFrame::Book(book) => {
            let snapshot = L2Snapshot {
                bids: book.bids.clone(),
                asks: book.asks.clone(),
            };
            append(&book.coin, book.time, MarketEventKind::L2Snapshot(snapshot))
        }
        WsFrame::BookDiff(change) => append(
            &change.coin,
            change.time,
            MarketEventKind::L2Diff(change.diff.clone()),
        ),
        WsFrame::Bbo(bbo) => {
            let kind = MarketEventKind::Bbo(Bbo {
                bid: bbo.bid,
                ask: bbo.ask,
            });
            append(&bbo.coin, bbo.time, kind)
        }
        WsFrame::Trades(trades) => trades.iter().try_for_each(|trade| {
            let kind = MarketEventKind::Trade(Trade {
                tid: trade.tid,
                px: trade.px,
                qty: trade.qty,
                aggressor: trade.aggressor,
            });
            append(&trade.coin, trade.time, kind)
        }),
        WsFrame::SubscriptionResponse
        | WsFrame::Pong
        | WsFrame::Error(_)
        | WsFrame::Other { .. } => Ok(()),
    };
    if result.is_err() {
        out.truncate(start);
    }
    result
}
