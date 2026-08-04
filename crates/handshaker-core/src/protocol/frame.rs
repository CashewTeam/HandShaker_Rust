use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::i18n;

pub(crate) const MAX_UPSTREAM_PAYLOAD: usize = 0x40_0000;
pub(crate) const MAX_DOWNSTREAM_CHUNK: usize = 32_761;

#[derive(Clone, Copy)]
pub(crate) enum WireDirection {
    Out,
    In,
}

/// Wire log bounds (P2-4): the log may contain clipboard text, paths and
/// media bytes, so it stays opt-in, defaults to header-only (lengths and
/// frame types, no payload bytes), and rotates at a fixed size instead of
/// growing unbounded.
pub(crate) const MAX_WIRE_LOG_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct WireLog {
    path: PathBuf,
    /// When false only the header line (direction, note, byte length) is
    /// written — payload hex dump requires an explicit opt-in.
    payload: bool,
    /// Rotation threshold; tests use a small value via `open_with_max`.
    max_bytes: u64,
    file: Mutex<std::fs::File>,
}

impl WireLog {
    pub fn open(path: &Path, payload: bool) -> Result<Self> {
        Self::open_with_max(path, payload, MAX_WIRE_LOG_BYTES)
    }

    /// `open` with an explicit rotation threshold (tests).
    pub fn open_with_max(path: &Path, payload: bool, max_bytes: u64) -> Result<Self> {
        let file = Self::open_file(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            payload,
            max_bytes,
            file: Mutex::new(file),
        })
    }

    fn open_file(path: &Path) -> Result<std::fs::File> {
        let mut options = OpenOptions::new();
        options.create(true).append(true).write(true);
        set_mode(&mut options);
        let file = options.open(path).map_err(|error| {
            Error::LocalIo(i18n::format(
                "frame.wire_log_create_failed",
                &[&path.display().to_string(), &error.to_string()],
            ))
        })?;
        set_file_permissions(&file, path)?;
        Ok(file)
    }

    /// Rotate when the log exceeds `max_bytes`: rename the current file to
    /// `<path>.1` (one generation kept) and start a fresh file. Caller
    /// must hold the file lock.
    fn maybe_rotate(&self, file: &mut std::fs::File) {
        let over_limit = file
            .metadata()
            .map(|meta| meta.len() >= self.max_bytes)
            .unwrap_or(false);
        if !over_limit {
            return;
        }
        let rotated = self.path.with_extension(format!(
            "{}.1",
            self.path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("log")
        ));
        let _ = fs::rename(&self.path, &rotated);
        if let Ok(new_file) = Self::open_file(&self.path) {
            *file = new_file;
        }
    }

    pub fn record(&self, direction: WireDirection, note: &str, data: &[u8]) {
        let direction = match direction {
            WireDirection::Out => ">>",
            WireDirection::In => "<<",
        };
        if let Ok(mut file) = self.file.lock() {
            self.maybe_rotate(&mut file);
            let _ = writeln!(
                file,
                "{}",
                i18n::format(
                    "wire.record_header",
                    &[direction, note, &data.len().to_string()],
                )
            );
            // P2-4: payload bytes are only dumped when explicitly opted
            // in; the default header-only mode never touches payload.
            if self.payload {
                for chunk in data.chunks(32) {
                    for byte in chunk {
                        let _ = write!(file, "{byte:02x} ");
                    }
                    let _ = writeln!(file);
                }
            }
            let _ = file.flush();
        }
    }
}

#[cfg(unix)]
fn set_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_file_permissions(file: &std::fs::File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| {
            Error::LocalIo(i18n::format(
                "frame.wire_log_permission_failed",
                &[&path.display().to_string(), &error.to_string()],
            ))
        })
}

#[cfg(not(unix))]
fn set_file_permissions(_file: &std::fs::File, _path: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn encode_upstream(sid: u32, flag: u8, payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() > MAX_UPSTREAM_PAYLOAD {
        return Err(Error::Protocol(i18n::format(
            "frame.upstream_too_large",
            &[
                &payload.len().to_string(),
                &MAX_UPSTREAM_PAYLOAD.to_string(),
            ],
        )));
    }
    let mut frame = Vec::with_capacity(9 + payload.len());
    frame.extend_from_slice(&sid.to_be_bytes());
    frame.push(flag);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub(crate) async fn write_upstream<W: AsyncWrite + Unpin>(
    writer: &mut W,
    sid: u32,
    flag: u8,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let frame = encode_upstream(sid, flag, payload)?;
    writer.write_all(&frame).await.map_err(|error| {
        Error::Transport(i18n::format("frame.write_failed", &[&error.to_string()]))
    })?;
    writer.flush().await.map_err(|error| {
        Error::Transport(i18n::format("frame.flush_failed", &[&error.to_string()]))
    })?;
    Ok(frame)
}

pub(crate) async fn read_downstream<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<(u32, Vec<u8>, [u8; 6])> {
    let mut header = [0_u8; 6];
    reader.read_exact(&mut header).await.map_err(|error| {
        Error::Transport(i18n::format(
            "frame.header_read_failed",
            &[&error.to_string()],
        ))
    })?;
    let sid = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
    let chunk_len = u16::from_be_bytes(header[4..].try_into().expect("two bytes")) as usize;
    if chunk_len > MAX_DOWNSTREAM_CHUNK {
        return Err(Error::Protocol(i18n::format(
            "frame.downstream_too_large",
            &[&chunk_len.to_string(), &MAX_DOWNSTREAM_CHUNK.to_string()],
        )));
    }
    let mut chunk = vec![0_u8; chunk_len];
    reader.read_exact(&mut chunk).await.map_err(|error| {
        Error::Transport(i18n::format(
            "frame.data_read_failed",
            &[&error.to_string()],
        ))
    })?;
    Ok((sid, chunk, header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn upstream_frame_uses_big_endian_header() {
        let frame = encode_upstream(0x8000_0001, 1, b"abc").expect("frame");
        assert_eq!(&frame[..9], &[0x80, 0, 0, 1, 1, 0, 0, 0, 3]);
        assert_eq!(&frame[9..], b"abc");
    }

    #[test]
    fn upstream_rejects_oversized_payload() {
        let payload = vec![0; MAX_UPSTREAM_PAYLOAD + 1];
        assert!(encode_upstream(1, 1, &payload).is_err());
    }

    #[tokio::test]
    async fn downstream_accepts_capture_verified_maximum_chunk() {
        let (mut writer, mut reader) = tokio::io::duplex(MAX_DOWNSTREAM_CHUNK + 6);
        let task = tokio::spawn(async move {
            writer.write_all(&7_u32.to_be_bytes()).await.unwrap();
            writer
                .write_all(&(MAX_DOWNSTREAM_CHUNK as u16).to_be_bytes())
                .await
                .unwrap();
            writer
                .write_all(&vec![0x5a; MAX_DOWNSTREAM_CHUNK])
                .await
                .unwrap();
        });
        let (sid, chunk, _) = read_downstream(&mut reader).await.expect("frame");
        assert_eq!(sid, 7);
        assert_eq!(chunk.len(), MAX_DOWNSTREAM_CHUNK);
        assert!(chunk.iter().all(|byte| *byte == 0x5a));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn downstream_rejects_chunk_above_verified_limit() {
        let (mut writer, mut reader) = tokio::io::duplex(6);
        writer.write_all(&7_u32.to_be_bytes()).await.unwrap();
        writer
            .write_all(&((MAX_DOWNSTREAM_CHUNK + 1) as u16).to_be_bytes())
            .await
            .unwrap();
        assert!(read_downstream(&mut reader).await.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn wire_log_forces_existing_file_to_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wire.log");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _log = WireLog::open(&path, false).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn wire_log_header_only_mode_never_dumps_payload() {
        // P2-4: the default mode records direction/note/length but no
        // payload bytes.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wire.log");
        let log = WireLog::open(&path, false).unwrap();
        log.record(WireDirection::Out, "test", b"\x00\x01secret\xff");
        drop(log);

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains(">> test"), "header line: {contents}");
        assert!(
            contents.contains("9"),
            "header carries the byte length: {contents}"
        );
        assert!(
            !contents.contains("00 01"),
            "no payload hex allowed in header-only mode"
        );
        assert!(
            !contents.contains("secret"),
            "payload bytes must never appear"
        );
    }

    #[test]
    fn wire_log_payload_mode_dumps_hex() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wire.log");
        let log = WireLog::open(&path, true).unwrap();
        log.record(WireDirection::In, "test", b"\x00\x01secret\xff");
        drop(log);

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("<< test"), "header line: {contents}");
        assert!(
            contents.contains("00 01"),
            "payload hex must be present in payload mode"
        );
        assert!(contents.contains("ff"), "trailing byte hex present");
    }

    #[test]
    fn wire_log_rotates_at_size_cap() {
        // P2-4: bounded growth — past the cap the log renames to
        // <path>.1 and starts a fresh file (small cap for the test).
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wire.log");
        let log = WireLog::open_with_max(&path, true, 1024).unwrap();
        let big = vec![0xabu8; 4096];
        log.record(WireDirection::Out, "big", &big);
        // Rotation is checked before each record: the second write sees
        // the file past the cap and rotates first. Keep the second record
        // small so the fresh file stays under the cap.
        log.record(WireDirection::Out, "after", b"small");
        drop(log);

        assert!(
            path.with_extension("log.1").exists(),
            "rotated generation must exist"
        );
        assert!(
            std::fs::metadata(&path)
                .map(|meta| meta.len() < 1024)
                .unwrap_or(false),
            "fresh log must be under the cap"
        );
    }
}
