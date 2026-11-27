//! The ClickHouse side: connection, migrations, inserts and the ingest log.

use std::collections::BTreeMap;
use std::fmt;

use clickhouse::{Client, Row, RowOwned, RowWrite};
use data_encoding::HEXLOWER;
use hl_codec::hyperliquid::dex;
use hl_types::InstrumentId;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::rows::{IngestLogRow, InstrumentRow, RawlogStateRow};
use crate::sources::RawlogChain;

/// Rows per `INSERT`. Each insert is one deduplication unit.
const CHUNK_ROWS: usize = 50_000;

/// A migration embedded in the loader. The loader's row types are written
/// against these exact files, so they ship together.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Order of application; also the number in the file name.
    pub version: u32,
    /// File name without extension.
    pub name: &'static str,
    /// The file's text.
    pub sql: &'static str,
}

/// Every migration, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_init",
        sql: include_str!("../../../schema/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "0002_quality",
        sql: include_str!("../../../schema/0002_quality.sql"),
    },
    Migration {
        version: 3,
        name: "0003_rawlog_state",
        sql: include_str!("../../../schema/0003_rawlog_state.sql"),
    },
];

const BOOTSTRAP: [&str; 2] = [
    "CREATE DATABASE IF NOT EXISTS hl",
    "CREATE TABLE IF NOT EXISTS hl.schema_migrations (version UInt32, name String, sha256 String, \
     applied_at DateTime64(3, 'UTC')) ENGINE = MergeTree ORDER BY version",
];

/// What went wrong on the database side.
#[derive(Debug)]
pub enum DbError {
    /// The client or server reported an error.
    ClickHouse(clickhouse::error::Error),
    /// A migration file breaks the format rules.
    BadMigration(String),
    /// A migration that was already applied has different text now.
    ChangedMigration(String),
    /// The database and this loader disagree on which migrations exist: the
    /// database has one this loader does not include, or lacks one of its own.
    SchemaMismatch(String),
    /// Two different names hash to the same instrument id.
    InstrumentCollision(String),
    /// An argument from the caller is not valid.
    BadArgument(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClickHouse(error) => write!(f, "clickhouse: {error}"),
            Self::BadMigration(message) => write!(f, "migration file: {message}"),
            Self::ChangedMigration(name) => {
                write!(
                    f,
                    "migration {name} was applied with different text; add a new migration instead"
                )
            }
            Self::SchemaMismatch(message) => write!(f, "schema: {message}"),
            Self::InstrumentCollision(message) => write!(f, "instrument id collision: {message}"),
            Self::BadArgument(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DbError {}

impl From<clickhouse::error::Error> for DbError {
    fn from(error: clickhouse::error::Error) -> Self {
        Self::ClickHouse(error)
    }
}

/// Splits a migration file into statements.
///
/// Lines starting with `--` are comments. A statement ends with `;` at the
/// end of a line. `?` is refused anywhere, because the client would treat it
/// as a bind placeholder.
pub fn statements(sql: &str) -> Result<Vec<String>, DbError> {
    let mut statements = Vec::new();
    let mut current = String::new();
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        if trimmed.contains('?') {
            return Err(DbError::BadMigration(format!(
                "'?' is not allowed: {trimmed}"
            )));
        }
        if let Some(body) = trimmed.strip_suffix(';') {
            if body.contains(';') {
                return Err(DbError::BadMigration(format!(
                    "one statement per line end: {trimmed}"
                )));
            }
            current.push_str(body);
            statements.push(std::mem::take(&mut current).trim().to_owned());
        } else {
            if trimmed.contains(';') {
                return Err(DbError::BadMigration(format!(
                    "';' inside a statement: {trimmed}"
                )));
            }
            current.push_str(trimmed);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        return Err(DbError::BadMigration(
            "the last statement does not end with ';'".to_owned(),
        ));
    }
    Ok(statements)
}

fn sha256_hex(text: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(text.as_bytes()))
}

#[derive(Debug, Row, Deserialize)]
struct AppliedMigration {
    version: u32,
    name: String,
    sha256: String,
}

/// How the migrations recorded in the database relate to the embedded ones.
#[derive(Debug)]
enum SchemaState<'a> {
    /// The same list, with the same text.
    Current,
    /// The database lacks these embedded migrations, in order of version.
    Behind(Vec<&'a Migration>),
}

/// Compares the migrations recorded in the database with `known`.
///
/// Every applied migration must be one of `known`, with the same name and
/// text. A migration the database has but `known` lacks means the database
/// was migrated by a newer loader; this loader's row types do not describe
/// it, so the comparison fails rather than reporting the database as current.
fn compare_schema<'a>(
    applied: &[AppliedMigration],
    known: &'a [Migration],
) -> Result<SchemaState<'a>, DbError> {
    for done in applied {
        let Some(migration) = known.iter().find(|m| m.version == done.version) else {
            return Err(DbError::SchemaMismatch(format!(
                "the database has migration {}, which this loader does not include; \
                 use a loader built from a commit that has it",
                done.name
            )));
        };
        if migration.name != done.name || sha256_hex(migration.sql) != done.sha256 {
            return Err(DbError::ChangedMigration(migration.name.to_owned()));
        }
    }
    let missing: Vec<&Migration> = known
        .iter()
        .filter(|migration| !applied.iter().any(|done| done.version == migration.version))
        .collect();
    Ok(if missing.is_empty() {
        SchemaState::Current
    } else {
        SchemaState::Behind(missing)
    })
}

#[derive(Debug, Row, serde::Serialize)]
struct MigrationRow {
    version: u32,
    name: String,
    sha256: String,
    applied_at: i64,
}

/// The data tables whose keys are checked for conflicting rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictTable {
    /// `hl.l2_snapshot`.
    L2Snapshot,
    /// `hl.fills`.
    Fills,
    /// `hl.bbo`.
    Bbo,
    /// `hl.ws_trades`.
    WsTrades,
}

struct ConflictSpec {
    name: &'static str,
    /// The table's sort key, which identifies a row.
    key: &'static str,
    /// The key columns other than instrument and time, as text.
    row_key: &'static str,
    /// A hash of the columns that should be equal for equal keys.
    content_hash: &'static str,
}

impl ConflictTable {
    fn spec(self) -> ConflictSpec {
        match self {
            Self::L2Snapshot => ConflictSpec {
                name: "l2_snapshot",
                key: "instrument_id, source, ts_exch, ts_node",
                row_key: "concat(toString(source), ' ts_node=', toString(ts_node))",
                content_hash: "row_hash",
            },
            Self::Fills => ConflictSpec {
                name: "fills",
                key: "instrument_id, ts_exch, tid, side",
                row_key: "concat('tid=', toString(tid), ' side=', toString(side))",
                content_hash: "row_hash",
            },
            Self::Bbo => ConflictSpec {
                name: "bbo",
                key: "instrument_id, source, ts_exch",
                row_key: "toString(source)",
                content_hash: "cityHash64(bid_px, bid_sz, bid_n, ask_px, ask_sz, ask_n)",
            },
            Self::WsTrades => ConflictSpec {
                name: "ws_trades",
                key: "instrument_id, ts_exch, tid",
                row_key: "concat('tid=', toString(tid))",
                content_hash: "cityHash64(aggressor, px, sz, buyer, seller, hash)",
            },
        }
    }
}

/// A connection to the research database.
#[derive(Clone)]
pub struct Db {
    client: Client,
}

impl fmt::Debug for Db {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

impl Db {
    /// Connects using `HLMM_CLICKHOUSE_URL` (default `http://127.0.0.1:8123`),
    /// `HLMM_CLICKHOUSE_USER` (default `default`) and
    /// `HLMM_CLICKHOUSE_PASSWORD` (default empty). The password is never
    /// printed.
    pub fn from_env() -> Self {
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
        let client = Client::default()
            .with_url(var("HLMM_CLICKHOUSE_URL", "http://127.0.0.1:8123"))
            .with_user(var("HLMM_CLICKHOUSE_USER", "default"))
            .with_password(var("HLMM_CLICKHOUSE_PASSWORD", ""));
        Self { client }
    }

    /// The underlying client, for ad-hoc queries.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The migrations recorded in the database; none if it has no
    /// migration table yet.
    async fn applied_migrations(&self) -> Result<Vec<AppliedMigration>, DbError> {
        let tables: u64 = self
            .client
            .query(
                "SELECT count() FROM system.tables \
                 WHERE database = 'hl' AND name = 'schema_migrations'",
            )
            .fetch_one()
            .await?;
        if tables == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .client
            .query("SELECT version, name, sha256 FROM hl.schema_migrations ORDER BY version")
            .fetch_all()
            .await?)
    }

    /// Fails unless the database has exactly the migrations embedded in this
    /// loader, with the same text. Commands that write data call this first:
    /// rows built by a loader from another commit can fail to insert, or
    /// insert with new columns silently left at their defaults.
    pub async fn require_current_schema(&self) -> Result<(), DbError> {
        match compare_schema(&self.applied_migrations().await?, MIGRATIONS)? {
            SchemaState::Current => Ok(()),
            SchemaState::Behind(missing) => Err(DbError::SchemaMismatch(format!(
                "migration {} has not been applied; run `hl-loader migrate`",
                missing[0].name
            ))),
        }
    }

    /// Applies every embedded migration that has not been applied yet and
    /// returns the names of those it applied. Refuses to continue if an
    /// applied migration's text has changed, or if the database has a
    /// migration this loader does not include.
    pub async fn migrate(&self, now_ms: i64) -> Result<Vec<&'static str>, DbError> {
        for statement in BOOTSTRAP {
            self.client.query_raw(statement).execute().await?;
        }
        let applied = self.applied_migrations().await?;
        let SchemaState::Behind(missing) = compare_schema(&applied, MIGRATIONS)? else {
            return Ok(Vec::new());
        };

        let mut newly_applied = Vec::new();
        for migration in missing {
            let checksum = sha256_hex(migration.sql);
            for statement in statements(migration.sql)? {
                self.client.query_raw(&statement).execute().await?;
            }
            let mut insert = self
                .client
                .insert_unescaped::<MigrationRow>("hl.schema_migrations")
                .await?;
            insert
                .write(&MigrationRow {
                    version: migration.version,
                    name: migration.name.to_owned(),
                    sha256: checksum,
                    applied_at: now_ms,
                })
                .await?;
            insert.end().await?;
            newly_applied.push(migration.name);
        }
        Ok(newly_applied)
    }

    /// True when a file with this SHA-256 has been loaded successfully.
    pub async fn is_loaded(&self, sha256: &str) -> Result<bool, DbError> {
        Ok(self.last_done_ingest(sha256).await?.is_some())
    }

    /// Latest successful load of these file bytes.
    pub(crate) async fn last_done_ingest(&self, sha256: &str) -> Result<Option<u64>, DbError> {
        Ok(self
            .client
            .query(
                "SELECT ingest_id FROM hl.ingest_log WHERE sha256 = ? AND status = 'done' \
                 ORDER BY ingest_id DESC LIMIT 1",
            )
            .bind(sha256)
            .fetch_optional()
            .await?)
    }

    /// The most recent stored decoder state for one file in a source chain.
    /// Its market-data writes finished before the state was inserted, so a
    /// successor can use it even if the final ingest-log write was interrupted.
    pub async fn rawlog_state(
        &self,
        chain: &RawlogChain,
        file_seq: u64,
    ) -> Result<Option<RawlogStateRow>, DbError> {
        Ok(self
            .client
            .query(
                "SELECT source, process_start_wall_ns, file_seq, sha256, state_json, ingest_id \
                 FROM hl.rawlog_state FINAL \
                 WHERE source = ? AND process_start_wall_ns = ? AND file_seq = ?",
            )
            .bind(&chain.source)
            .bind(chain.process_start_wall_ns)
            .bind(file_seq)
            .fetch_optional()
            .await?)
    }

    /// Stores state after all market-data writes have completed.
    /// Every load attempt has a distinct token, so retrying a load whose
    /// final ingest-log write failed can replace the previous state's id.
    pub async fn store_rawlog_state(&self, row: &RawlogStateRow) -> Result<(), DbError> {
        self.insert(
            "hl.rawlog_state",
            std::slice::from_ref(row),
            &format!("rawlog_state:{}:{}", row.sha256, row.ingest_id),
        )
        .await
    }

    /// Records the instruments of a batch, after checking that no id is
    /// already used by a different name.
    pub async fn add_instruments(
        &self,
        instruments: &BTreeMap<(String, String), InstrumentId>,
        ingest_id: u64,
    ) -> Result<(), DbError> {
        let known: Vec<InstrumentRow> = self
            .client
            .query("SELECT venue, coin, instrument_id, dex, ingest_id FROM hl.instruments FINAL")
            .fetch_all()
            .await?;
        let new_rows = new_instrument_rows(known, instruments, ingest_id)?;
        self.insert(
            "hl.instruments",
            &new_rows,
            &format!("instruments:{ingest_id}"),
        )
        .await
    }

    /// Inserts `rows` into `table`, a `database.table` name written by this
    /// crate (it is not quoted). Rows go in chunks; each carries the deduplication token
    /// `{token}:{table}:{chunk number}`, so retrying a failed load does not
    /// write a chunk twice.
    pub async fn insert<T: RowOwned + RowWrite>(
        &self,
        table: &str,
        rows: &[T],
        token: &str,
    ) -> Result<(), DbError> {
        for (number, chunk) in rows.chunks(CHUNK_ROWS).enumerate() {
            let mut insert = self
                .client
                .insert_unescaped::<T>(table)
                .await?
                .with_setting(
                    "insert_deduplication_token",
                    format!("{token}:{table}:{number}"),
                );
            for row in chunk {
                insert.write(row).await?;
            }
            insert.end().await?;
        }
        Ok(())
    }

    /// Finds keys of `table` that now hold rows with different content, among
    /// `instruments` between `from_ms` and `to_ms`, records them in
    /// `hl.ingest_conflict` under `ingest_id`, and returns how many there are.
    pub async fn record_conflicts(
        &self,
        table: ConflictTable,
        instruments: &[u32],
        (from_ms, to_ms): (i64, i64),
        ingest_id: u64,
        now_ms: i64,
    ) -> Result<u64, DbError> {
        if instruments.is_empty() {
            return Ok(0);
        }
        let spec = table.spec();
        let ids = instruments
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let insert = format!(
            "INSERT INTO hl.ingest_conflict (detected_at, ingest_id, table_name, instrument_id, ts_exch, \
             row_key, ingest_ids, row_hashes) \
             SELECT fromUnixTimestamp64Milli({{now:Int64}}), {{ingest:UInt64}}, '{name}', instrument_id, ts_exch, \
             {row_key}, groupUniqArray(ingest_id), groupUniqArray({hash}) \
             FROM hl.{name} \
             WHERE ts_exch >= fromUnixTimestamp64Milli({{from:Int64}}) \
               AND ts_exch <= fromUnixTimestamp64Milli({{to:Int64}}) AND instrument_id IN ({ids}) \
             GROUP BY {key} HAVING uniqExact({hash}) > 1",
            name = spec.name,
            row_key = spec.row_key,
            hash = spec.content_hash,
            key = spec.key,
        );
        self.client
            .query(&insert)
            .param("now", now_ms)
            .param("ingest", ingest_id)
            .param("from", from_ms)
            .param("to", to_ms)
            .execute()
            .await?;
        let count: u64 = self
            .client
            .query(&format!(
                "SELECT count() FROM hl.ingest_conflict \
                 WHERE ingest_id = {{ingest:UInt64}} AND table_name = '{}'",
                spec.name
            ))
            .param("ingest", ingest_id)
            .fetch_one()
            .await?;
        Ok(count)
    }

    /// Appends one row to the ingest log.
    pub async fn log(&self, row: &IngestLogRow) -> Result<(), DbError> {
        let mut insert = self
            .client
            .insert_unescaped::<IngestLogRow>("hl.ingest_log")
            .await?;
        insert.write(row).await?;
        insert.end().await?;
        Ok(())
    }
}

fn new_instrument_rows(
    known: Vec<InstrumentRow>,
    instruments: &BTreeMap<(String, String), InstrumentId>,
    ingest_id: u64,
) -> Result<Vec<InstrumentRow>, DbError> {
    let mut owners: BTreeMap<_, _> = known
        .into_iter()
        .map(|row| (row.instrument_id, (row.venue, row.coin)))
        .collect();
    let mut rows = Vec::new();
    for (name, &id) in instruments {
        let (venue, coin) = name;
        if let Some(existing) = owners.get(&id.get()) {
            if existing == name {
                continue;
            }
            return Err(DbError::InstrumentCollision(format!(
                "{venue}:{coin} and {}:{} both hash to {id}",
                existing.0, existing.1
            )));
        }
        owners.insert(id.get(), name.clone());
        rows.push(InstrumentRow {
            venue: venue.clone(),
            coin: coin.clone(),
            instrument_id: id.get(),
            dex: dex(coin).unwrap_or_default().to_owned(),
            ingest_id,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hl_codec::hyperliquid::{Network, instrument_id};

    fn batch_instruments(coins: &[&str]) -> BTreeMap<(String, String), InstrumentId> {
        let network = Network::Mainnet;
        coins
            .iter()
            .map(|&coin| {
                (
                    (network.venue().to_owned(), coin.to_owned()),
                    instrument_id(network, coin),
                )
            })
            .collect()
    }

    #[test]
    fn rejects_colliding_names_in_a_batch_and_against_stored_rows() {
        let first = "fixture-139599";
        let second = "fixture-322382";
        let batch = batch_instruments(&[first, second]);
        let stored = new_instrument_rows(Vec::new(), &batch_instruments(&[first]), 1).unwrap();
        for result in [
            new_instrument_rows(Vec::new(), &batch, 2),
            new_instrument_rows(stored, &batch_instruments(&[second]), 2),
        ] {
            let Err(DbError::InstrumentCollision(message)) = result else {
                panic!("expected an instrument id collision");
            };
            assert!(
                message.contains(first) && message.contains(second),
                "{message}"
            );
        }
    }

    #[test]
    fn writes_dex_values_once_per_instrument() {
        let batch = batch_instruments(&["BTC", "xyz:NVDA"]);
        let rows = new_instrument_rows(Vec::new(), &batch, 7).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].coin.as_str(), rows[0].dex.as_str()), ("BTC", ""));
        assert_eq!(
            (rows[1].coin.as_str(), rows[1].dex.as_str()),
            ("xyz:NVDA", "xyz")
        );
        assert!(new_instrument_rows(rows, &batch, 8).unwrap().is_empty());
    }

    #[test]
    fn splits_statements_and_drops_comments() {
        let sql = "-- header\nCREATE TABLE a\n(\n    x UInt8\n);\n\n-- next\nCREATE VIEW b AS\nSELECT 1;\n";
        assert_eq!(
            statements(sql).unwrap(),
            [
                "CREATE TABLE a\n(\nx UInt8\n)",
                "CREATE VIEW b AS\nSELECT 1"
            ]
        );
    }

    #[test]
    fn refuses_migrations_that_break_the_rules() {
        let cases = [
            "SELECT 1 WHERE x = ?;",
            "SELECT 1; SELECT 2;",
            "SELECT ';'\n;",
            "SELECT 1",
        ];
        for sql in cases {
            assert!(
                matches!(statements(sql), Err(DbError::BadMigration(_))),
                "{sql:?}"
            );
        }
    }

    fn migration(version: u32, name: &'static str, sql: &'static str) -> Migration {
        Migration { version, name, sql }
    }

    fn applied(migration: &Migration) -> AppliedMigration {
        AppliedMigration {
            version: migration.version,
            name: migration.name.to_owned(),
            sha256: sha256_hex(migration.sql),
        }
    }

    fn known() -> [Migration; 2] {
        [
            migration(
                1,
                "0001_init",
                "CREATE TABLE hl.a (x UInt8) ENGINE = Memory;",
            ),
            migration(
                2,
                "0002_more",
                "CREATE TABLE hl.b (x UInt8) ENGINE = Memory;",
            ),
        ]
    }

    fn missing_names(state: SchemaState<'_>) -> Vec<&'static str> {
        match state {
            SchemaState::Current => Vec::new(),
            SchemaState::Behind(missing) => missing.iter().map(|m| m.name).collect(),
        }
    }

    #[test]
    fn the_same_migrations_are_current() {
        let known = known();
        let state = compare_schema(&[applied(&known[0]), applied(&known[1])], &known).unwrap();
        assert!(matches!(state, SchemaState::Current));
    }

    #[test]
    fn missing_migrations_are_reported_in_order() {
        let known = known();
        assert_eq!(
            missing_names(compare_schema(&[], &known).unwrap()),
            ["0001_init", "0002_more"]
        );
        assert_eq!(
            missing_names(compare_schema(&[applied(&known[0])], &known).unwrap()),
            ["0002_more"]
        );
    }

    #[test]
    fn a_migration_this_loader_does_not_have_is_refused() {
        let known = known();
        let newer = migration(
            3,
            "0003_newer",
            "CREATE TABLE hl.c (x UInt8) ENGINE = Memory;",
        );
        let result = compare_schema(
            &[applied(&known[0]), applied(&known[1]), applied(&newer)],
            &known,
        );
        match result {
            Err(DbError::SchemaMismatch(message)) => {
                assert!(message.contains("0003_newer"), "{message}")
            }
            other => panic!("expected a schema mismatch, got {other:?}"),
        }
    }

    #[test]
    fn an_applied_migration_with_other_text_is_refused() {
        let known = known();
        let mut edited = applied(&known[1]);
        edited.sha256 = sha256_hex("something else");
        assert!(matches!(
            compare_schema(&[applied(&known[0]), edited], &known),
            Err(DbError::ChangedMigration(name)) if name == "0002_more"
        ));

        let mut renamed = applied(&known[1]);
        renamed.name = "0002_other".to_owned();
        assert!(matches!(
            compare_schema(&[applied(&known[0]), renamed], &known),
            Err(DbError::ChangedMigration(_))
        ));
    }

    #[test]
    fn the_embedded_migrations_parse() {
        let versions: Vec<u32> = MIGRATIONS
            .iter()
            .map(|migration| migration.version)
            .collect();
        assert_eq!(versions, (1..=MIGRATIONS.len() as u32).collect::<Vec<_>>());
        for migration in MIGRATIONS {
            assert!(
                migration
                    .name
                    .starts_with(&format!("{:04}_", migration.version))
            );
            let statements = statements(migration.sql).unwrap();
            assert!(statements.iter().all(|statement| statement.contains("hl.")));
        }
        assert_eq!(statements(MIGRATIONS[0].sql).unwrap().len(), 7);
        assert_eq!(statements(MIGRATIONS[1].sql).unwrap().len(), 3);
        assert_eq!(statements(MIGRATIONS[2].sql).unwrap().len(), 1);
    }
}
