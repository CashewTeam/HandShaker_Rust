use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, Result};

pub(crate) const MAX_UPSTREAM_PAYLOAD: usize = 0x40_0000;
pub(crate) const MAX_DOWNSTREAM_CHUNK: usize = 32_761;

#[derive(Clone, Copy)]
pub(crate) enum WireDirection {
    Out,
    In,
}

pub(crate) struct WireLog {
    file: Mutex<std::fs::File>,
}

impl WireLog {
    pub fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        set_mode(&mut options);
        let file = options.open(path).map_err(|error| {
            Error::LocalIo(format!("创建线路日志 {} 失败：{error}", path.display()))
        })?;
        set_file_permissions(&file, path)?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    pub fn record(&self, direction: WireDirection, note: &str, data: &[u8]) {
        let direction = match direction {
            WireDirection::Out => ">>",
            WireDirection::In => "<<",
        };
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{direction} {note} len={}", data.len());
            for chunk in data.chunks(32) {
                for byte in chunk {
                    let _ = write!(file, "{byte:02x} ");
                }
                let _ = writeln!(file);
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
            Error::LocalIo(format!("设置线路日志 {} 权限失败：{error}", path.display()))
        })
}

#[cfg(not(unix))]
fn set_file_permissions(_file: &std::fs::File, _path: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn encode_upstream(sid: u32, flag: u8, payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() > MAX_UPSTREAM_PAYLOAD {
        return Err(Error::Protocol(format!(
            "上行 payload {} 字节，超过 {} 字节限制",
            payload.len(),
            MAX_UPSTREAM_PAYLOAD
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
    writer
        .write_all(&frame)
        .await
        .map_err(|error| Error::Transport(format!("写入 SSP 帧失败：{error}")))?;
    writer
        .flush()
        .await
        .map_err(|error| Error::Transport(format!("刷新 SSP 帧失败：{error}")))?;
    Ok(frame)
}

pub(crate) async fn read_downstream<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<(u32, Vec<u8>, [u8; 6])> {
    let mut header = [0_u8; 6];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|error| Error::Transport(format!("读取下行帧头失败：{error}")))?;
    let sid = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
    let chunk_len = u16::from_be_bytes(header[4..].try_into().expect("two bytes")) as usize;
    if chunk_len > MAX_DOWNSTREAM_CHUNK {
        return Err(Error::Protocol(format!(
            "下行分块 {} 字节，超过 {} 字节限制",
            chunk_len, MAX_DOWNSTREAM_CHUNK
        )));
    }
    let mut chunk = vec![0_u8; chunk_len];
    reader
        .read_exact(&mut chunk)
        .await
        .map_err(|error| Error::Transport(format!("读取下行数据失败：{error}")))?;
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
        let _log = WireLog::open(&path).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
