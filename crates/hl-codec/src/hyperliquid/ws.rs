//! WebSocket frames from the Hyperliquid market-data feed.

use std::borrow::Cow;

use data_encoding::BASE64;
use hl_types::{L2Diff, Level, Px, Qty, Side, Timestamp, TradeId};
use serde::Deserialize;
use serde_json::value::RawValue;

use super::{Address, DecodeError, hex_bytes};

/// Largest size a compressed payload may inflate to. The 104 compressed `l2`
/// diffs in the PONS excerpt inflated to 144–1,095 bytes (HL-69).
/// The cap only bounds memory on corrupt input.
const MAX_INFLATED_LEN: usize = 16 * 1024 * 1024;

/// A full book: the first message of the `l2` channel after subscribing, or
/// any message of the `l2Book` channel (HL-66, HL-76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsBook {
    /// The venue's coin name.
    pub coin: String,
    /// Block time of the book (HL-95).
    pub time: Timestamp,
    /// Bid levels, highest price first (HL-77).
    pub bids: Vec<Level>,
    /// Ask levels, lowest price first (HL-77).
    pub asks: Vec<Level>,
}

/// A change to the book on the `l2` channel (HL-66).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsBookDiff {
    /// The venue's coin name.
    pub coin: String,
    /// Block time of the change (HL-95).
    pub time: Timestamp,
    /// What changed. Levels set by a diff have an order count of 0, because
    /// the venue does not send one (HL-74).
    pub diff: L2Diff,
}

/// A best-bid-offer update on the `bbo` channel (HL-78).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsBbo {
    /// The venue's coin name.
    pub coin: String,
    /// Block time of the update (HL-95).
    pub time: Timestamp,
    /// Best bid, or `None` when there are no bids (HL-78).
    pub bid: Option<Level>,
    /// Best ask, or `None` when there are no asks (HL-78).
    pub ask: Option<Level>,
}

/// One trade on the `trades` channel (HL-90).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsTrade {
    /// The venue's coin name.
    pub coin: String,
    /// Block time of the trade (HL-95).
    pub time: Timestamp,
    /// Trade id. Not unique on its own across the whole venue (HL-86).
    pub tid: TradeId,
    /// Execution price.
    pub px: Px,
    /// Executed size.
    pub qty: Qty,
    /// Side of the order that crossed the spread (HL-83).
    pub aggressor: Side,
    /// Hash of the transaction that caused the trade. All zero for trades
    /// the venue generates itself (HL-88, HL-89 unverified).
    pub hash: [u8; 32],
    /// Account that bought (HL-84).
    pub buyer: Address,
    /// Account that sold (HL-84).
    pub seller: Address,
}

/// One decoded WebSocket text frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsFrame {
    /// A full book.
    Book(WsBook),
    /// A change to the book.
    BookDiff(WsBookDiff),
    /// A best-bid-offer update.
    Bbo(WsBbo),
    /// Trades, oldest first. The first message after subscribing replays
    /// recent trades, so a consumer must expect trades it has seen before (HL-80, HL-81).
    Trades(Vec<WsTrade>),
    /// The venue acknowledged a subscription (HL-45).
    SubscriptionResponse,
    /// The venue answered a ping (HL-43).
    Pong,
    /// The venue reported an error, for example a refused subscription (HL-46).
    Error(String),
    /// A frame on a channel this codec does not decode yet.
    Other {
        /// The frame's channel name.
        channel: String,
    },
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    channel: Cow<'a, str>,
    #[serde(borrow, default)]
    data: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct WireLevel<'a> {
    #[serde(borrow)]
    px: Cow<'a, str>,
    #[serde(borrow)]
    sz: Cow<'a, str>,
    n: u32,
}

#[derive(Deserialize)]
pub(super) struct WireBook<'a> {
    #[serde(borrow)]
    coin: Cow<'a, str>,
    time: u64,
    #[serde(borrow)]
    levels: (Vec<WireLevel<'a>>, Vec<WireLevel<'a>>),
}

#[derive(Deserialize)]
struct WireDiffLevel<'a> {
    #[serde(borrow)]
    p: Cow<'a, str>,
    #[serde(borrow)]
    s: Cow<'a, str>,
}

#[derive(Deserialize)]
struct WireDiff<'a> {
    #[serde(borrow)]
    c: Cow<'a, str>,
    t: u64,
    /// Levels to set: bids, then asks (HL-70).
    #[serde(borrow, default)]
    l: (Vec<WireDiffLevel<'a>>, Vec<WireDiffLevel<'a>>),
    /// Positions to remove: bids, then asks (HL-70).
    #[serde(default)]
    r: (Vec<u32>, Vec<u32>),
}

/// The `l2` channel wraps its payload under one of three keys (HL-67).
#[derive(Deserialize)]
struct WireL2<'a> {
    /// Snapshot (HL-67).
    #[serde(borrow, default)]
    s: Option<WireBook<'a>>,
    /// Diff, as plain JSON (HL-67).
    #[serde(borrow, default)]
    u: Option<WireDiff<'a>>,
    /// Diff, as base64 of a raw DEFLATE stream holding the same JSON (HL-67, HL-68).
    #[serde(borrow, default)]
    c: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct WireBbo<'a> {
    #[serde(borrow)]
    coin: Cow<'a, str>,
    time: u64,
    #[serde(borrow)]
    bbo: (Option<WireLevel<'a>>, Option<WireLevel<'a>>),
}

#[derive(Deserialize)]
struct WireTrade<'a> {
    #[serde(borrow)]
    coin: Cow<'a, str>,
    #[serde(borrow)]
    side: Cow<'a, str>,
    #[serde(borrow)]
    px: Cow<'a, str>,
    #[serde(borrow)]
    sz: Cow<'a, str>,
    time: u64,
    #[serde(borrow)]
    hash: Cow<'a, str>,
    tid: u64,
    /// Buyer, then seller (HL-84).
    #[serde(borrow)]
    users: (Cow<'a, str>, Cow<'a, str>),
}

pub(super) fn json<'a, T: Deserialize<'a>>(text: &'a str) -> Result<T, DecodeError> {
    serde_json::from_str(text).map_err(|error| DecodeError::Json(error.to_string()))
}

pub(super) fn px(field: &'static str, text: &str) -> Result<Px, DecodeError> {
    text.parse().map_err(|error| DecodeError::Number {
        field,
        text: text.to_owned(),
        error,
    })
}

pub(super) fn qty(field: &'static str, text: &str) -> Result<Qty, DecodeError> {
    text.parse().map_err(|error| DecodeError::Number {
        field,
        text: text.to_owned(),
        error,
    })
}

pub(super) fn time(field: &'static str, millis: u64) -> Result<Timestamp, DecodeError> {
    // Nanoseconds since the epoch must fit in an i64.
    i64::try_from(millis)
        .ok()
        .filter(|millis| *millis <= i64::MAX / 1_000_000)
        .map(Timestamp::from_millis)
        .ok_or_else(|| DecodeError::Invalid {
            field,
            text: millis.to_string(),
        })
}

fn levels(wire: &[WireLevel<'_>]) -> Result<Vec<Level>, DecodeError> {
    wire.iter().map(level).collect()
}

fn level(wire: &WireLevel<'_>) -> Result<Level, DecodeError> {
    Ok(Level {
        px: px("px", &wire.px)?,
        qty: qty("sz", &wire.sz)?,
        orders: wire.n,
    })
}

fn diff_levels(wire: &[WireDiffLevel<'_>]) -> Result<Vec<Level>, DecodeError> {
    wire.iter()
        .map(|wire| {
            Ok(Level {
                px: px("p", &wire.p)?,
                qty: qty("s", &wire.s)?,
                orders: 0,
            })
        })
        .collect()
}

pub(super) fn book(wire: &WireBook<'_>) -> Result<WsBook, DecodeError> {
    Ok(WsBook {
        coin: wire.coin.to_string(),
        time: time("time", wire.time)?,
        bids: levels(&wire.levels.0)?,
        asks: levels(&wire.levels.1)?,
    })
}

fn book_diff(wire: WireDiff<'_>) -> Result<WsBookDiff, DecodeError> {
    Ok(WsBookDiff {
        coin: wire.c.to_string(),
        time: time("t", wire.t)?,
        diff: L2Diff {
            set_bids: diff_levels(&wire.l.0)?,
            set_asks: diff_levels(&wire.l.1)?,
            remove_bids: wire.r.0,
            remove_asks: wire.r.1,
        },
    })
}

/// Unpacks base64 of a raw DEFLATE stream into the JSON text it holds (HL-68).
fn inflate(text: &str) -> Result<String, DecodeError> {
    let packed = BASE64
        .decode(text.as_bytes())
        .map_err(|_| DecodeError::Compressed("base64"))?;
    let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(&packed, MAX_INFLATED_LEN)
        .map_err(|_| DecodeError::Compressed("inflate"))?;
    String::from_utf8(bytes).map_err(|_| DecodeError::Compressed("utf-8"))
}

fn l2(data: &str) -> Result<WsFrame, DecodeError> {
    let wire: WireL2<'_> = json(data)?;
    match (wire.s, wire.u, wire.c) {
        (Some(snapshot), None, None) => Ok(WsFrame::Book(book(&snapshot)?)),
        (None, Some(diff), None) => Ok(WsFrame::BookDiff(book_diff(diff)?)),
        (None, None, Some(packed)) => {
            let text = inflate(&packed)?;
            Ok(WsFrame::BookDiff(book_diff(json(&text)?)?))
        }
        _ => Err(DecodeError::Json(
            "l2 data must have exactly one of s, u, c".to_owned(),
        )),
    }
}

fn bbo(data: &str) -> Result<WsFrame, DecodeError> {
    let wire: WireBbo<'_> = json(data)?;
    Ok(WsFrame::Bbo(WsBbo {
        coin: wire.coin.to_string(),
        time: time("time", wire.time)?,
        bid: wire.bbo.0.as_ref().map(level).transpose()?,
        ask: wire.bbo.1.as_ref().map(level).transpose()?,
    }))
}

fn trade(wire: &WireTrade<'_>) -> Result<WsTrade, DecodeError> {
    let aggressor = match wire.side.as_ref() {
        "B" => Side::Buy,
        "A" => Side::Sell,
        other => {
            return Err(DecodeError::Invalid {
                field: "side",
                text: other.to_owned(),
            });
        }
    };
    Ok(WsTrade {
        coin: wire.coin.to_string(),
        time: time("time", wire.time)?,
        tid: TradeId(wire.tid),
        px: px("px", &wire.px)?,
        qty: qty("sz", &wire.sz)?,
        aggressor,
        hash: hex_bytes("hash", &wire.hash)?,
        buyer: Address(hex_bytes("users", &wire.users.0)?),
        seller: Address(hex_bytes("users", &wire.users.1)?),
    })
}

fn trades(data: &str) -> Result<WsFrame, DecodeError> {
    let wire: Vec<WireTrade<'_>> = json(data)?;
    Ok(WsFrame::Trades(
        wire.iter().map(trade).collect::<Result<_, _>>()?,
    ))
}

/// Decodes the payload of one text frame received from the Hyperliquid
/// WebSocket feed.
///
/// A field this codec reads must be present and well-formed; unknown fields
/// are ignored. A channel that is not decoded yet is returned as
/// [`WsFrame::Other`]. No input makes this function panic.
pub fn decode_ws_frame(payload: &[u8]) -> Result<WsFrame, DecodeError> {
    let text =
        core::str::from_utf8(payload).map_err(|error| DecodeError::Json(error.to_string()))?;
    let envelope: Envelope<'_> = json(text)?;
    let data = || {
        envelope
            .data
            .map(RawValue::get)
            .ok_or_else(|| DecodeError::Json(format!("channel {} has no data", envelope.channel)))
    };
    match envelope.channel.as_ref() {
        "l2" => l2(data()?),
        "l2Book" => Ok(WsFrame::Book(book(&json(data()?)?)?)),
        "bbo" => bbo(data()?),
        "trades" => trades(data()?),
        "subscriptionResponse" => Ok(WsFrame::SubscriptionResponse),
        "pong" => Ok(WsFrame::Pong),
        "error" => Ok(WsFrame::Error(json(data()?)?)),
        other => Ok(WsFrame::Other {
            channel: other.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests;
