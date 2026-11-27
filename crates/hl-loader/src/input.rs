//! Opening input files.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use data_encoding::HEXLOWER;
use sha2::{Digest, Sha256};

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4d, 0x18];

/// Opens `path` for reading, undoing zstd or lz4 frame compression when the
/// file starts with the matching magic number. Any other file is read as is.
pub fn open_decompressed(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let mut file = BufReader::with_capacity(1 << 20, File::open(path)?);
    let head = file.fill_buf()?;
    Ok(if head.starts_with(&ZSTD_MAGIC) {
        Box::new(BufReader::with_capacity(
            1 << 20,
            zstd::Decoder::with_buffer(file)?,
        ))
    } else if head.starts_with(&LZ4_FRAME_MAGIC) {
        Box::new(BufReader::with_capacity(
            1 << 20,
            lz4_flex::frame::FrameDecoder::new(file),
        ))
    } else {
        Box::new(file)
    })
}

/// The SHA-256 of a file's bytes as stored, in lower-case hex, and its size.
pub fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((HEXLOWER.encode(&hasher.finalize()), size))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("hl-loader-{name}-{}", std::process::id()));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    fn read_all(path: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        open_decompressed(path)
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[test]
    fn hashes_files() {
        let path = temp_file("sha", b"abc");
        assert_eq!(
            sha256_file(&path).unwrap(),
            (
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned(),
                3
            )
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn detects_compression_by_magic_number() {
        let text = b"line one\nline two\n".repeat(50);

        let plain = temp_file("plain", &text);
        assert_eq!(read_all(&plain), text);

        let zstd = temp_file("zstd", &zstd::encode_all(&text[..], 3).unwrap());
        assert_eq!(read_all(&zstd), text);

        let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
        encoder.write_all(&text).unwrap();
        let lz4 = temp_file("lz4", &encoder.finish().unwrap());
        assert_eq!(read_all(&lz4), text);

        let empty = temp_file("empty", b"");
        assert!(read_all(&empty).is_empty());

        for path in [plain, zstd, lz4, empty] {
            std::fs::remove_file(path).unwrap();
        }
    }
}
