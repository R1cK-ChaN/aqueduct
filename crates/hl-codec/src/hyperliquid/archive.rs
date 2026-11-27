//! The two official archive formats, one JSON object per line (HL-98, HL-106).
//!
//! - `hyperliquid-archive/market_data/<date>/<hour>/l2Book/<coin>.lz4`: one
//!   20-level book per line (HL-97, HL-100).
//! - `hl-mainnet-node-data/node_fills_by_block/hourly/<date>/<hour>.lz4`: one
//!   block per line, holding every fill of every market in that block (HL-105, HL-106, HL-108).
//!
//! The functions here take one decompressed line. Reading files and undoing
//! the lz4 compression is the caller's job.

use std::borrow::Cow;

use hl_types::{
    ClientOrderId, InstrumentId, MarketEvent, MarketEventKind, OrderId, Px, Qty, Side, Timestamp,
    Trade, TradeId, Usd,
};
use serde::Deserialize;

use super::ws::{self, WireBook, WsBook};
use super::{Address, DecodeError, hex_bytes};

/// One line of the L2 archive (HL-98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveBook {
    /// The archiving node's wall clock when it wrote the line. Lines are
    /// written in batches, so this can trail the book's block time by
    /// seconds; it is not a receive time (HL-99, HL-120).
    pub node_time: Timestamp,
    /// The book. Wrap it in [`super::WsFrame::Book`] to turn it into a
    /// market event with [`super::market_events`].
    pub book: WsBook,
}

/// What kind of market a coin name belongs to, judged by its form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarketKind {
    /// A perpetual, on the main dex (`BTC`) or a HIP-3 dex (`xyz:NVDA`). HL-55.
    Perp,
    /// A spot pair: `@<index>` or `BASE/QUOTE` (HL-56).
    Spot,
    /// An outcome market: `#<id>` or `+<id>` (HL-57).
    Outcome,
}

impl MarketKind {
    /// Classifies a coin name as the venue writes it.
    pub fn of(coin: &str) -> Self {
        if coin.starts_with('@') || coin.contains('/') {
            Self::Spot
        } else if coin.starts_with('#') || coin.starts_with('+') {
            Self::Outcome
        } else {
            Self::Perp
        }
    }
}

/// The liquidation a fill was part of (HL-109).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Liquidation {
    /// The account being liquidated (HL-109).
    pub liquidated_user: Address,
    /// Mark price at the time (HL-109).
    pub mark_px: Px,
    /// How the venue liquidated, for example `market` (HL-109).
    pub method: String,
}

/// One side of one trade on a perpetual, as the node recorded it. Every
/// trade appears twice in a block: once for each party (HL-110).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFill {
    /// The account this row belongs to (HL-109).
    pub user: Address,
    /// The venue's coin name.
    pub coin: String,
    /// Block time of the fill, in milliseconds (HL-95).
    pub time: Timestamp,
    /// Trade id, shared by the two rows of a trade. Unique only together
    /// with the coin and time (HL-85, HL-86).
    pub tid: TradeId,
    /// The venue's id of this party's order (HL-109).
    pub oid: OrderId,
    /// This party's own order id, when it set one (HL-109).
    pub cloid: Option<ClientOrderId>,
    /// Side of this party's order (HL-109).
    pub side: Side,
    /// True when this party's order crossed the spread (the taker, HL-110).
    pub crossed: bool,
    /// Execution price (HL-109).
    pub px: Px,
    /// Executed size (HL-109).
    pub qty: Qty,
    /// This party's signed position before the fill (HL-109). The venue writes it
    /// from a floating-point value (HL-112, unverified), so it is rounded to eight decimals here (HL-111).
    pub start_position: Qty,
    /// The venue's description of the fill, for example `Open Long` or
    /// `Long > Short`. Kept as text because the set is not closed (HL-113).
    pub dir: String,
    /// Profit realised by this fill, in the fee token (HL-28, HL-109).
    pub closed_pnl: Usd,
    /// Fee charged to this party, in `fee_token`. Negative for a rebate (HL-28, HL-29, HL-109).
    pub fee: Usd,
    /// Token the fee is charged in. A dollar stablecoin in the sampled perpetuals
    /// (`USDC`, and on some HIP-3 dexes `USDT0`, `USDH`, `USDE`). HL-28, HL-109.
    pub fee_token: String,
    /// Part of the fee that went to a builder (HL-109, HL-115).
    pub builder_fee: Option<Usd>,
    /// The builder that received `builder_fee` (HL-109, HL-115).
    pub builder: Option<Address>,
    /// Part of the fee that went to the HIP-3 deployer. Can be negative (HL-109, HL-116).
    pub deployer_fee: Option<Usd>,
    /// Priority fee paid for the order, in HYPE (HL-31, HL-109).
    pub priority_gas: Option<Qty>,
    /// Id of the TWAP order this fill belongs to (HL-109).
    pub twap_id: Option<u64>,
    /// Set when the fill was part of a liquidation (HL-109).
    pub liquidation: Option<Liquidation>,
    /// Hash of the transaction that caused the fill. All zero when the venue
    /// generated it (HL-88, HL-89 unverified, HL-109).
    pub hash: [u8; 32],
}

/// One line of `node_fills_by_block`: one block (HL-106).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillBlock {
    /// Height of the block (HL-107).
    pub block_number: u64,
    /// Block time, to the nanosecond (HL-95, HL-107).
    pub block_time: Timestamp,
    /// The node's wall clock when it processed the block (HL-95, HL-107).
    pub local_time: Timestamp,
    /// Fills on perpetuals, in file order.
    pub fills: Vec<NodeFill>,
    /// Rows for spot markets, which are not decoded (HL-108).
    pub spot_rows: u32,
    /// Rows for outcome markets, which are not decoded (HL-108).
    pub outcome_rows: u32,
}

#[derive(Deserialize)]
struct WireArchiveLine<'a> {
    #[serde(borrow)]
    time: Cow<'a, str>,
    ver_num: u32,
    #[serde(borrow)]
    raw: WireArchiveRaw<'a>,
}

#[derive(Deserialize)]
struct WireArchiveRaw<'a> {
    #[serde(borrow)]
    channel: Cow<'a, str>,
    #[serde(borrow)]
    data: WireBook<'a>,
}

#[derive(Deserialize)]
struct WireLiquidation<'a> {
    #[serde(borrow, rename = "liquidatedUser")]
    liquidated_user: Cow<'a, str>,
    #[serde(borrow, rename = "markPx")]
    mark_px: Cow<'a, str>,
    #[serde(borrow)]
    method: Cow<'a, str>,
}

#[derive(Deserialize)]
struct WireFill<'a> {
    #[serde(borrow)]
    coin: Cow<'a, str>,
    #[serde(borrow)]
    px: Cow<'a, str>,
    #[serde(borrow)]
    sz: Cow<'a, str>,
    #[serde(borrow)]
    side: Cow<'a, str>,
    time: u64,
    #[serde(borrow, rename = "startPosition")]
    start_position: Cow<'a, str>,
    #[serde(borrow)]
    dir: Cow<'a, str>,
    #[serde(borrow, rename = "closedPnl")]
    closed_pnl: Cow<'a, str>,
    #[serde(borrow)]
    hash: Cow<'a, str>,
    oid: u64,
    crossed: bool,
    #[serde(borrow)]
    fee: Cow<'a, str>,
    tid: u64,
    #[serde(borrow, default)]
    cloid: Option<Cow<'a, str>>,
    #[serde(borrow, rename = "feeToken")]
    fee_token: Cow<'a, str>,
    #[serde(rename = "twapId", default)]
    twap_id: Option<u64>,
    #[serde(borrow, rename = "builderFee", default)]
    builder_fee: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    builder: Option<Cow<'a, str>>,
    #[serde(borrow, rename = "deployerFee", default)]
    deployer_fee: Option<Cow<'a, str>>,
    #[serde(borrow, rename = "priorityGas", default)]
    priority_gas: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    liquidation: Option<WireLiquidation<'a>>,
}

#[derive(Deserialize)]
struct WireFillBlock<'a> {
    #[serde(borrow)]
    local_time: Cow<'a, str>,
    #[serde(borrow)]
    block_time: Cow<'a, str>,
    block_number: u64,
    /// Pairs of account address and fill (HL-107).
    #[serde(borrow)]
    events: Vec<(Cow<'a, str>, WireFill<'a>)>,
}

fn text(line: &[u8]) -> Result<&str, DecodeError> {
    core::str::from_utf8(line).map_err(|error| DecodeError::Json(error.to_string()))
}

fn usd(field: &'static str, text: &str) -> Result<Usd, DecodeError> {
    text.parse().map_err(|error| DecodeError::Number {
        field,
        text: text.to_owned(),
        error,
    })
}

fn optional<T>(
    value: Option<&Cow<'_, str>>,
    parse: impl FnOnce(&str) -> Result<T, DecodeError>,
) -> Result<Option<T>, DecodeError> {
    value.map(|text| parse(text)).transpose()
}

/// Parses a decimal that may carry more than eight decimals, rounding half
/// away from zero to eight. For fields the venue prints from a float, such
/// as `"-2471856.2999999998"` (HL-111, HL-112 unverified).
fn rounded_qty(field: &'static str, text: &str) -> Result<Qty, DecodeError> {
    let fail = |error| DecodeError::Number {
        field,
        text: text.to_owned(),
        error,
    };
    let Some((whole, fraction)) = text.split_once('.') else {
        return text.parse().map_err(fail);
    };
    if fraction.len() <= 8 {
        return text.parse().map_err(fail);
    }
    let (kept, dropped) = fraction.split_at(8);
    if !dropped.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(fail(hl_types::ParseFixedError::InvalidCharacter));
    }
    let truncated: Qty = format!("{whole}.{kept}").parse().map_err(fail)?;
    let step = if dropped.as_bytes()[0] >= b'5' { 1 } else { 0 };
    let step = if text.starts_with('-') { -step } else { step };
    truncated
        .as_e8()
        .checked_add(step)
        .map(Qty::from_e8)
        .ok_or_else(|| fail(hl_types::ParseFixedError::OutOfRange))
}

const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// Days from 1970-01-01 to the given date in the proleptic Gregorian
/// calendar, after Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Parses `YYYY-MM-DDTHH:MM:SS` with an optional fraction of up to nine
/// digits and an optional trailing `Z`. The time is UTC.
fn node_time(field: &'static str, text: &str) -> Result<Timestamp, DecodeError> {
    let invalid = || DecodeError::Invalid {
        field,
        text: text.to_owned(),
    };
    let number = |digits: &str| -> Option<i64> {
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
    };
    let parse = || -> Option<i64> {
        let body = text.strip_suffix('Z').unwrap_or(text);
        let (date_time, fraction) = body.split_once('.').unwrap_or((body, ""));
        let bytes = date_time.as_bytes();
        let separators_ok = date_time.len() == 19
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':';
        if !separators_ok || fraction.len() > 9 || (body.contains('.') && fraction.is_empty()) {
            return None;
        }
        let field = |range: core::ops::Range<usize>| number(date_time.get(range)?);
        let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
        let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
        let in_range = (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60;
        if !in_range {
            return None;
        }
        let nanos = if fraction.is_empty() {
            0
        } else {
            number(fraction)? * 10_i64.pow(9 - u32::try_from(fraction.len()).ok()?)
        };
        let seconds =
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
        seconds.checked_mul(NANOS_PER_SECOND)?.checked_add(nanos)
    };
    parse().map(Timestamp::from_nanos).ok_or_else(invalid)
}

/// Decodes one line of the official L2 archive (HL-97).
///
/// Only format version 1 is known. Any other `ver_num` is an error, so a
/// format change is noticed instead of being misread (HL-98).
pub fn decode_archive_l2_line(line: &[u8]) -> Result<ArchiveBook, DecodeError> {
    let wire: WireArchiveLine<'_> = ws::json(text(line)?)?;
    if wire.ver_num != 1 {
        return Err(DecodeError::Invalid {
            field: "ver_num",
            text: wire.ver_num.to_string(),
        });
    }
    if wire.raw.channel != "l2Book" {
        return Err(DecodeError::Invalid {
            field: "channel",
            text: wire.raw.channel.into_owned(),
        });
    }
    Ok(ArchiveBook {
        node_time: node_time("time", &wire.time)?,
        book: ws::book(&wire.raw.data)?,
    })
}

fn node_fill(user: &str, wire: &WireFill<'_>) -> Result<NodeFill, DecodeError> {
    let side = match wire.side.as_ref() {
        "B" => Side::Buy,
        "A" => Side::Sell,
        other => {
            return Err(DecodeError::Invalid {
                field: "side",
                text: other.to_owned(),
            });
        }
    };
    let address = |field, text: &str| hex_bytes(field, text).map(Address);
    let liquidation = wire
        .liquidation
        .as_ref()
        .map(|wire| {
            Ok(Liquidation {
                liquidated_user: address("liquidatedUser", &wire.liquidated_user)?,
                mark_px: ws::px("markPx", &wire.mark_px)?,
                method: wire.method.to_string(),
            })
        })
        .transpose()?;
    Ok(NodeFill {
        user: address("user", user)?,
        coin: wire.coin.to_string(),
        time: ws::time("time", wire.time)?,
        tid: TradeId(wire.tid),
        oid: OrderId(wire.oid),
        cloid: optional(wire.cloid.as_ref(), |text| {
            hex_bytes("cloid", text)
                .map(|bytes| ClientOrderId::from_u128(u128::from_be_bytes(bytes)))
        })?,
        side,
        crossed: wire.crossed,
        px: ws::px("px", &wire.px)?,
        qty: ws::qty("sz", &wire.sz)?,
        start_position: rounded_qty("startPosition", &wire.start_position)?,
        dir: wire.dir.to_string(),
        closed_pnl: usd("closedPnl", &wire.closed_pnl)?,
        fee: usd("fee", &wire.fee)?,
        fee_token: wire.fee_token.to_string(),
        builder_fee: optional(wire.builder_fee.as_ref(), |text| usd("builderFee", text))?,
        builder: optional(wire.builder.as_ref(), |text| address("builder", text))?,
        deployer_fee: optional(wire.deployer_fee.as_ref(), |text| usd("deployerFee", text))?,
        priority_gas: optional(wire.priority_gas.as_ref(), |text| {
            ws::qty("priorityGas", text)
        })?,
        twap_id: wire.twap_id,
        liquidation,
        hash: hex_bytes("hash", &wire.hash)?,
    })
}

/// Decodes one line of `node_fills_by_block` (HL-106).
///
/// Fills on perpetuals are decoded in full. Rows for spot and outcome
/// markets are counted and otherwise skipped: they are outside the project's
/// scope, and some of their amounts need more than eight decimals (HL-117).
pub fn decode_fill_block(line: &[u8]) -> Result<FillBlock, DecodeError> {
    let wire: WireFillBlock<'_> = ws::json(text(line)?)?;
    let mut block = FillBlock {
        block_number: wire.block_number,
        block_time: node_time("block_time", &wire.block_time)?,
        local_time: node_time("local_time", &wire.local_time)?,
        fills: Vec::with_capacity(wire.events.len()),
        spot_rows: 0,
        outcome_rows: 0,
    };
    for (user, fill) in &wire.events {
        match MarketKind::of(&fill.coin) {
            MarketKind::Perp => block.fills.push(node_fill(user, fill)?),
            MarketKind::Spot => block.spot_rows += 1,
            MarketKind::Outcome => block.outcome_rows += 1,
        }
    }
    Ok(block)
}

/// Appends trade events to `out` in fill order, using the ids from `resolve`.
///
/// Only crossed rows are offered to the resolver (HL-83, HL-108, HL-110).
/// `None` skips the coin. Each event carries the fill time and `ts_local`.
pub fn trade_events(
    block: &FillBlock,
    resolve: impl Fn(&str) -> Option<InstrumentId>,
    ts_local: Timestamp,
    out: &mut Vec<MarketEvent>,
) {
    for fill in block.fills.iter().filter(|fill| fill.crossed) {
        let Some(instrument) = resolve(&fill.coin) else {
            continue;
        };
        let trade = Trade {
            tid: fill.tid,
            px: fill.px,
            qty: fill.qty,
            aggressor: fill.side,
        };
        out.push(MarketEvent {
            instrument,
            ts_exch: fill.time,
            ts_local,
            kind: MarketEventKind::Trade(trade),
        });
    }
}

#[cfg(test)]
mod tests;
