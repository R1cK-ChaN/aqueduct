use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use hl_codec::rawlog::{RecordKind, write_record};
use hl_recorder::public_v1;
use serde_json::{Value, json};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
pub const MIDNIGHT_MS: i64 = 1_795_651_200_000;
pub const PROCESS_NS: i64 = MIDNIGHT_MS * 1_000_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const BTC_BBO: &str =
    r#"{"channel":"bbo","data":{"coin":"BTC","time":1,"bbo":[{"px":"10","sz":"2","n":1},null]}}"#;
pub const ETH_L2: &str = r#"{"channel":"l2","data":{"s":{"coin":"ETH","time":2,"levels":[[],[{"px":"12","sz":"3","n":1}]]}}}"#;
pub const SOL_CONTEXT: &str =
    r#"{"channel":"activeAssetCtx","data":{"coin":"SOL","ctx":{"funding":"0.001"}}}"#;
pub const BTC_TRADE: &str = r#"{"channel":"trades","data":[{"coin":"BTC","side":"B","px":"11","sz":"1","time":3,"hash":"0x0000000000000000000000000000000000000000000000000000000000000000","tid":4,"users":["0x0000000000000000000000000000000000000001","0x0000000000000000000000000000000000000002"]}]}"#;

/// A private synthetic staging directory and a separate local output directory.
pub struct Day {
    pub root: PathBuf,
    pub date: &'static str,
    pub midnight_ms: i64,
}

impl Day {
    pub fn today() -> Self {
        Self::new("2026-11-26", MIDNIGHT_MS)
    }

    pub fn yesterday() -> Self {
        Self::new("2026-11-25", MIDNIGHT_MS - 24 * HOUR_MS)
    }

    fn new(date: &'static str, midnight_ms: i64) -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "hl-publish-synthetic-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create private test directory");
        let day = Self {
            root,
            date,
            midnight_ms,
        };
        fs::create_dir_all(day.source_dir()).expect("create staged UTC day");
        fs::create_dir(day.root.join("out")).expect("create output directory");
        fs::write(day.quality_file(), quality(date).to_string()).expect("write synthetic quality");
        day
    }

    pub fn source_dir(&self) -> PathBuf {
        self.root
            .join("staging")
            .join(public_v1::SOURCE_NAME)
            .join(self.date.replace('-', ""))
    }

    pub fn quality_file(&self) -> PathBuf {
        self.root.join("quality.json")
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join("out/manifest.json")
    }

    pub fn command(&self) -> Command {
        self.expiring("2027-11-21")
    }

    pub fn expiring(&self, expires: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hl-publish"));
        command
            .args(["seal", "--day", self.date, "--staging-dir"])
            .arg(self.root.join("staging"))
            .arg("--output-dir")
            .arg(self.root.join("out"))
            .args(["--expires", expires]);
        command
    }

    pub fn with_quality(&self) -> Command {
        let mut command = self.command();
        command.arg("--quality-results").arg(self.quality_file());
        command
    }

    pub fn hour_ms(&self, hour: u8) -> i64 {
        self.midnight_ms + i64::from(hour) * HOUR_MS
    }

    pub fn name(&self, hour: u8) -> String {
        format!(
            "{}-{}T{hour:02}-{}.hlrec.zst",
            public_v1::SOURCE_NAME,
            self.date.replace('-', ""),
            self.hour_ms(hour)
        )
    }

    pub fn put(
        &self,
        hour: u8,
        process_ns: i64,
        seq: u64,
        frames: &[(RecordKind, &str)],
    ) -> String {
        let open_ms = self.hour_ms(hour);
        let records = encode(open_ms, process_ns, header(process_ns, seq), frames);
        let name = self.name(hour);
        fs::write(self.source_dir().join(&name), compress(&records, true))
            .expect("stage synthetic recording");
        name
    }
}

impl Drop for Day {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove synthetic test directory");
    }
}

pub fn quality(day: &str) -> Value {
    json!({
        "day": day,
        "complete": true,
        "loader_revision": "synthetic",
        "checks": [{
            "name": "coverage", "subject": "bbo", "status": "info",
            "compared": 1, "violations": 0
        }, {
            "name": "ws_trades_vs_fills", "subject": "BTC", "status": "skip",
            "compared": 0, "violations": 0
        }]
    })
}

pub fn header(process_ns: i64, seq: u64) -> Value {
    json!({
        "event": "file_open", "format": 1, "source": public_v1::SOURCE_NAME,
        "url": public_v1::URL, "recorder_version": "synthetic",
        "process_start_wall_ns": process_ns, "file_seq": seq
    })
}

/// Frame timestamps are one second apart, beginning with file_open at open_ms.
pub fn encode(
    open_ms: i64,
    process_ns: i64,
    open: Value,
    frames: &[(RecordKind, &str)],
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let open = open.to_string();
    let all = std::iter::once((RecordKind::Meta, open.as_str())).chain(frames.iter().copied());
    for (i, (kind, payload)) in all.enumerate() {
        let ns =
            open_ms * 1_000_000 + i64::try_from(i).expect("fixture record index") * 1_000_000_000;
        let mono = u64::try_from(ns - process_ns).expect("fixture process started");
        write_record(&mut bytes, kind, ns, mono, payload.as_bytes())
            .expect("encode RFC 0002 record");
    }
    bytes
}

pub fn compress(records: &[u8], checksum: bool) -> Vec<u8> {
    let mut encoder = zstd::Encoder::new(Vec::new(), 3).expect("zstd encoder");
    encoder
        .include_checksum(checksum)
        .expect("configure fixture checksum");
    encoder.write_all(records).expect("compress fixture");
    encoder.finish().expect("finish fixture frame")
}

pub fn accepted(index: usize, name: &str) -> String {
    format!("accepted\t{index}\t{name}\n")
}

pub fn withheld(index: usize, reason: &str) -> String {
    format!("withheld\t{index}\t{reason}\n")
}

pub fn run(command: &mut Command) -> Output {
    command.output().expect("execute hl-publish seal")
}

/// Check the command boundary and independently hash the compressed files named in its manifest.
pub fn sealed(day: &Day, command: &mut Command, report: &str) -> Value {
    let output = run(command);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::str::from_utf8(&output.stdout).expect("UTF-8 report"),
        report
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(day.manifest()).expect("read manifest");
    let manifest: Value = serde_json::from_slice(&bytes).expect("UTF-8 JSON manifest");
    let quality: Value =
        serde_json::from_slice(&fs::read(day.quality_file()).expect("quality input"))
            .expect("quality JSON");
    assert_eq!(manifest["quality"], quality);
    for file in manifest["files"].as_array().expect("files array") {
        let path = Path::new(file["path"].as_str().expect("file path"));
        let name = path.file_name().expect("object basename");
        let staged = day.source_dir().join(name);
        assert_eq!(
            file["size_bytes"],
            fs::metadata(&staged).expect("sealed input").len()
        );
        let digest = Command::new("python3")
            .args(["-I", "-c", "import hashlib, pathlib, sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())"])
            .arg(staged)
            .output().expect("compute independent SHA-256 of synthetic input");
        assert!(digest.status.success());
        assert_eq!(
            file["sha256"],
            std::str::from_utf8(&digest.stdout)
                .expect("hex digest")
                .trim()
        );
        assert_eq!(
            path,
            Path::new("v1/hl-public")
                .join(day.date.replace('-', ""))
                .join(name)
        );
    }
    manifest
}

pub fn snapshot(day: &Day, expected: &str) {
    let actual = fs::read_to_string(day.manifest()).expect("UTF-8 manifest bytes");
    assert!(
        actual == expected,
        "generated manifest:\n{actual}end of generated manifest"
    );
}
