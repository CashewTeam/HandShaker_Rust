use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest as _, Md5};
use prost::Message;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout};

use crate::error::{Error, Result};
use crate::protocol::crypto::SessionKeys;
use crate::protocol::frame::{WireDirection, WireLog, read_downstream, write_upstream};
use crate::protocol::handshake::HandshakeStrategy;
use crate::protocol::proto::{SspHeartBeatRequest, SspRequestType};

type ChunkReceiver = mpsc::UnboundedReceiver<Result<Vec<u8>>>;

const PHASE_NORMAL: u8 = 0;
const PHASE_DOWNLOAD_RAW: u8 = 1;

#[derive(Clone)]
struct PendingRequest {
    sender: mpsc::UnboundedSender<Result<Vec<u8>>>,
}

struct WriteCommand {
    sid: u32,
    flag: u8,
    payload: Vec<u8>,
    completion: oneshot::Sender<Result<Vec<u8>>>,
}

struct SessionInner {
    writer: mpsc::Sender<WriteCommand>,
    pending: Mutex<HashMap<u32, PendingRequest>>,
    next_sid: AtomicU32,
    keys: Arc<SessionKeys>,
    request_timeout: Duration,
    closed: Arc<AtomicBool>,
    wire_log: Option<Arc<WireLog>>,
}

pub(crate) struct OpenRequest {
    pub sid: u32,
    receiver: ChunkReceiver,
    inner: Arc<SessionInner>,
    phase: u8,
    completed: bool,
}

impl OpenRequest {
    pub async fn receive_normal(&mut self) -> Result<Vec<u8>> {
        if self.phase == PHASE_DOWNLOAD_RAW {
            return Err(Error::Protocol(
                "下载裸流开始后不能再按普通响应解码".to_string(),
            ));
        }
        let result = receive_normal(&mut self.receiver, self.inner.request_timeout).await;
        if result.is_err() {
            self.inner.fail_connection().await;
        }
        result
    }

    pub async fn receive_raw_to<W, F>(
        &mut self,
        total: u64,
        writer: &mut W,
        mut on_progress: F,
    ) -> Result<String>
    where
        W: tokio::io::AsyncWrite + Unpin,
        F: FnMut(u64),
    {
        self.phase = PHASE_DOWNLOAD_RAW;
        let inner = Arc::clone(&self.inner);
        let result = async {
            let total = usize::try_from(total)
                .map_err(|_| Error::Protocol("文件长度超出当前平台可处理范围".to_string()))?;
            let mut received = 0_usize;
            let mut digest = Md5::new();
            let mut discriminator = DownloadDiscriminator::default();
            while received < total {
                let chunk = timeout(inner.request_timeout, self.receiver.recv())
                    .await
                    .map_err(|_| Error::Timeout("接收文件数据块".to_string()))?
                    .ok_or_else(|| Error::Transport("下载过程中连接已关闭".to_string()))??;
                let remaining = total - received;
                let Some(chunk) = discriminator.push(&chunk, remaining)? else {
                    continue;
                };
                let next_received = received
                    .checked_add(chunk.len())
                    .ok_or_else(|| Error::Protocol("下载计数溢出".to_string()))?;
                if next_received > total {
                    return Err(Error::Protocol(format!(
                        "下载流超过声明长度：已收 {}，新块 {}，声明 {}",
                        received,
                        chunk.len(),
                        total
                    )));
                }
                writer
                    .write_all(&chunk)
                    .await
                    .map_err(|error| Error::LocalIo(format!("写入下载文件失败：{error}")))?;
                digest.update(&chunk);
                received = next_received;
                on_progress(received as u64);
            }
            writer
                .flush()
                .await
                .map_err(|error| Error::LocalIo(format!("刷新下载文件失败：{error}")))?;
            Ok(format!("{:x}", digest.finalize()))
        }
        .await;
        if result.is_err() {
            inner.fail_connection().await;
        }
        result
    }

    pub async fn send(&self, flag: u8, payload: &[u8]) -> Result<()> {
        self.inner.send(self.sid, flag, payload).await
    }

    pub async fn finish(mut self) {
        self.inner.pending.lock().await.remove(&self.sid);
        self.completed = true;
    }
}

impl Drop for OpenRequest {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if let Ok(mut pending) = self.inner.pending.try_lock() {
            pending.remove(&self.sid);
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let inner = Arc::clone(&self.inner);
            let sid = self.sid;
            runtime.spawn(async move {
                inner.pending.lock().await.remove(&sid);
                if let Err(error) = inner.cancel(sid).await {
                    tracing::debug!(%error, sid = format_args!("{sid:#010x}"), "发送取消请求失败");
                }
            });
        }
    }
}

pub(crate) struct Session {
    inner: Arc<SessionInner>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    heartbeat_task: JoinHandle<()>,
    event_task: JoinHandle<()>,
}

impl Session {
    pub async fn establish(
        mut stream: TcpStream,
        request_timeout: Duration,
        heartbeat_interval: Duration,
        wire_log: Option<Arc<WireLog>>,
        handshake: &dyn HandshakeStrategy,
    ) -> Result<Self> {
        let keys = Arc::new(
            handshake
                .establish(&mut stream, request_timeout, wire_log.as_ref())
                .await?,
        );
        let (reader, writer) = stream.into_split();
        let closed = Arc::new(AtomicBool::new(false));
        let (writer_sender, writer_receiver) = mpsc::channel(64);
        let writer_task = spawn_writer(
            writer,
            writer_receiver,
            Arc::clone(&closed),
            wire_log.clone(),
        );
        let inner = Arc::new(SessionInner {
            writer: writer_sender,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_0001),
            keys,
            request_timeout,
            closed,
            wire_log,
        });
        let (event_sender, event_receiver) = mpsc::channel(64);
        let reader_task = spawn_reader(reader, Arc::clone(&inner), event_sender);
        let event_task = spawn_event_drain(event_receiver);
        let heartbeat_task = spawn_heartbeat(Arc::clone(&inner), heartbeat_interval);
        Ok(Self {
            inner,
            reader_task,
            writer_task,
            heartbeat_task,
            event_task,
        })
    }

    pub async fn request<M: Message>(&self, message: &M) -> Result<Vec<u8>> {
        let mut request = self.open(message).await?;
        let response = request.receive_normal().await;
        request.finish().await;
        response
    }

    pub async fn open<M: Message>(&self, message: &M) -> Result<OpenRequest> {
        let payload = message.encode_to_vec();
        self.inner.open_signed(&payload).await
    }

    pub async fn close(self) -> Result<()> {
        self.heartbeat_task.abort();
        let mut result = Ok(());
        if !self.inner.closed.swap(true, Ordering::SeqCst) {
            let quit = crate::protocol::proto::SspQuitRequest {
                r#type: Some(SspRequestType::QuitRequest as i32),
            };
            let payload = quit.encode_to_vec();
            let signature = self.inner.keys.sign(&payload)?;
            let mut body = Vec::with_capacity(signature.len() + payload.len());
            body.extend_from_slice(&signature);
            body.extend_from_slice(&payload);
            let sid = self.inner.next_sid();
            result = self.inner.send(sid, 1, &body).await;
        }
        self.reader_task.abort();
        self.writer_task.abort();
        self.event_task.abort();
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.heartbeat_task.abort();
        self.reader_task.abort();
        self.writer_task.abort();
        self.event_task.abort();
    }
}

impl SessionInner {
    fn next_sid(&self) -> u32 {
        self.next_sid
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    async fn open_signed(self: &Arc<Self>, payload: &[u8]) -> Result<OpenRequest> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Transport("连接已经关闭".to_string()));
        }
        let sid = self.next_sid();
        let (sender, receiver) = mpsc::unbounded_channel();
        self.pending
            .lock()
            .await
            .insert(sid, PendingRequest { sender });

        let signature = self.keys.sign(payload)?;
        let mut body = Vec::with_capacity(signature.len() + payload.len());
        body.extend_from_slice(&signature);
        body.extend_from_slice(payload);
        let mut request = OpenRequest {
            sid,
            receiver,
            inner: Arc::clone(self),
            phase: PHASE_NORMAL,
            completed: false,
        };
        if let Err(error) = self.send(sid, 1, &body).await {
            self.pending.lock().await.remove(&sid);
            request.completed = true;
            return Err(error);
        }
        Ok(request)
    }

    async fn send(&self, sid: u32, flag: u8, payload: &[u8]) -> Result<()> {
        let (completion, response) = oneshot::channel();
        let command = WriteCommand {
            sid,
            flag,
            payload: payload.to_vec(),
            completion,
        };
        let send = async {
            self.writer
                .send(command)
                .await
                .map_err(|_| Error::Transport("SSP 写任务已经关闭".to_string()))?;
            response
                .await
                .map_err(|_| Error::Transport("SSP 写任务未返回结果".to_string()))?
        };
        match timeout(self.request_timeout, send).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                self.fail_connection().await;
                return Err(error);
            }
            Err(_) => {
                self.fail_connection().await;
                return Err(Error::Timeout(format!(
                    "发送 SSP 帧 sid={sid:#010x} flag={flag}"
                )));
            }
        };
        Ok(())
    }

    async fn cancel(&self, target_sid: u32) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let cancel_sid = self.next_sid();
        self.send(cancel_sid, 2, &target_sid.to_be_bytes()).await
    }

    async fn fail_connection(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.pending.lock().await.clear();
    }
}

fn spawn_writer(
    mut writer: OwnedWriteHalf,
    mut receiver: mpsc::Receiver<WriteCommand>,
    closed: Arc<AtomicBool>,
    wire_log: Option<Arc<WireLog>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = receiver.recv().await {
            let result =
                write_upstream(&mut writer, command.sid, command.flag, &command.payload).await;
            if let Ok(frame) = &result
                && let Some(log) = &wire_log
            {
                log.record(
                    WireDirection::Out,
                    &format!("sid={:#010x} flag={}", command.sid, command.flag),
                    frame,
                );
            }
            if result.is_err() {
                closed.store(true, Ordering::SeqCst);
            }
            let failed = result.is_err();
            let _ = command.completion.send(result);
            if failed {
                break;
            }
        }
        let _ = writer.shutdown().await;
    })
}

fn spawn_reader(
    mut reader: OwnedReadHalf,
    inner: Arc<SessionInner>,
    event_sender: mpsc::Sender<(u32, Vec<u8>)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut unmatched = HashMap::<u32, NormalAccumulator>::new();
        loop {
            let result = read_downstream(&mut reader).await;
            let (sid, chunk, header) = match result {
                Ok(value) => value,
                Err(error) => {
                    if !inner.closed.load(Ordering::SeqCst) {
                        tracing::debug!(%error, "SSP 读取任务结束");
                    }
                    inner.closed.store(true, Ordering::SeqCst);
                    inner.pending.lock().await.clear();
                    break;
                }
            };
            if let Some(log) = &inner.wire_log {
                log.record(
                    WireDirection::In,
                    &format!("sid={sid:#010x} header"),
                    &header,
                );
                log.record(WireDirection::In, &format!("sid={sid:#010x} chunk"), &chunk);
            }
            let pending = inner.pending.lock().await.get(&sid).cloned();
            if let Some(pending) = pending {
                if pending.sender.send(Ok(chunk)).is_err() {
                    inner.pending.lock().await.remove(&sid);
                }
            } else {
                let accumulator = unmatched.entry(sid).or_default();
                match accumulator.push(&chunk) {
                    Ok(Some(body)) => {
                        unmatched.remove(&sid);
                        if event_sender.try_send((sid, body)).is_err() {
                            tracing::debug!("手机主动消息队列已满，关闭 SSP 连接");
                            inner.closed.store(true, Ordering::SeqCst);
                            inner.pending.lock().await.clear();
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::debug!(sid = format_args!("{sid:#010x}"), %error, "手机主动消息无效");
                        unmatched.remove(&sid);
                    }
                }
            }
        }
    })
}

fn spawn_event_drain(mut receiver: mpsc::Receiver<(u32, Vec<u8>)>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some((sid, body)) = receiver.recv().await {
            tracing::debug!(
                sid = format_args!("{sid:#010x}"),
                length = body.len(),
                "收到手机主动消息"
            );
        }
    })
}

fn spawn_heartbeat(inner: Arc<SessionInner>, heartbeat_interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(heartbeat_interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if inner.closed.load(Ordering::SeqCst) {
                break;
            }
            let heartbeat = SspHeartBeatRequest {
                r#type: Some(SspRequestType::HeartBeatRequest as i32),
                host_timestamp: Some(unix_seconds()),
            };
            let payload = heartbeat.encode_to_vec();
            let mut request = match inner.open_signed(&payload).await {
                Ok(request) => request,
                Err(error) => {
                    tracing::debug!(%error, "发送心跳失败");
                    inner.closed.store(true, Ordering::SeqCst);
                    break;
                }
            };
            if let Err(error) = request.receive_normal().await {
                tracing::debug!(%error, "接收心跳失败");
                request.finish().await;
                inner.closed.store(true, Ordering::SeqCst);
                break;
            }
            request.finish().await;
        }
    })
}

#[derive(Default)]
struct NormalAccumulator {
    bytes: Vec<u8>,
    total: Option<usize>,
}

impl NormalAccumulator {
    fn push(&mut self, chunk: &[u8]) -> Result<Option<Vec<u8>>> {
        self.bytes.extend_from_slice(chunk);
        if self.total.is_none() && self.bytes.len() >= 8 {
            let total = u64::from_be_bytes(self.bytes[..8].try_into().expect("eight bytes"));
            self.total = Some(
                usize::try_from(total)
                    .map_err(|_| Error::Protocol("主动消息长度超出平台范围".to_string()))?,
            );
        }
        let Some(total) = self.total else {
            return Ok(None);
        };
        let expected = total
            .checked_add(8)
            .ok_or_else(|| Error::Protocol("主动消息长度溢出".to_string()))?;
        if self.bytes.len() == expected {
            return Ok(Some(self.bytes.split_off(8)));
        }
        if self.bytes.len() > expected {
            return Err(Error::Protocol("主动消息超过声明长度".to_string()));
        }
        Ok(None)
    }
}

const MAX_PUSH_MESSAGE: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct DownloadDiscriminator {
    candidate: Vec<u8>,
    expected: Option<usize>,
}

impl DownloadDiscriminator {
    fn push(&mut self, chunk: &[u8], remaining: usize) -> Result<Option<Vec<u8>>> {
        self.candidate.extend_from_slice(chunk);
        if self.expected.is_none() && self.candidate.len() < 8 && self.candidate.len() >= remaining
        {
            return Ok(Some(self.take_candidate()));
        }
        if self.expected.is_none() && self.candidate.len() >= 8 {
            let declared = u64::from_be_bytes(self.candidate[..8].try_into().expect("eight bytes"));
            let expected = usize::try_from(declared)
                .ok()
                .and_then(|length| length.checked_add(8));
            match expected {
                Some(expected)
                    if expected <= remaining && (8..=MAX_PUSH_MESSAGE + 8).contains(&expected) =>
                {
                    self.expected = Some(expected);
                }
                _ => return Ok(Some(self.take_candidate())),
            }
        }

        let Some(expected) = self.expected else {
            return Ok(None);
        };
        if self.candidate.len() < expected {
            return Ok(None);
        }
        if self.candidate.len() > expected {
            return Ok(Some(self.take_candidate()));
        }

        Err(Error::Protocol(
            "下载裸流中出现普通消息长度包络，线路无法安全判别".to_string(),
        ))
    }

    fn take_candidate(&mut self) -> Vec<u8> {
        self.expected = None;
        std::mem::take(&mut self.candidate)
    }
}

async fn receive_normal(
    receiver: &mut ChunkReceiver,
    request_timeout: Duration,
) -> Result<Vec<u8>> {
    let future = async {
        let mut assembled = Vec::new();
        let mut total = None;
        loop {
            let chunk = receiver
                .recv()
                .await
                .ok_or_else(|| Error::Transport("响应到达前连接已关闭".to_string()))??;
            assembled.extend_from_slice(&chunk);
            if total.is_none() && assembled.len() >= 8 {
                total = Some(u64::from_be_bytes(
                    assembled[..8].try_into().expect("eight bytes"),
                ));
            }
            if let Some(total) = total {
                let total = usize::try_from(total)
                    .map_err(|_| Error::Protocol("响应长度超出平台范围".to_string()))?;
                let expected = total
                    .checked_add(8)
                    .ok_or_else(|| Error::Protocol("响应长度溢出".to_string()))?;
                if assembled.len() == expected {
                    return Ok(assembled.split_off(8));
                }
                if assembled.len() > expected {
                    return Err(Error::Protocol("普通响应超过声明长度".to_string()));
                }
            }
        }
    };
    timeout(request_timeout, future)
        .await
        .map_err(|_| Error::Timeout("等待 SSP 响应".to_string()))?
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::Aes256;
    use base64::Engine;
    use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    use rand::rngs::OsRng;
    use rsa::pkcs1::DecodeRsaPublicKey;
    use rsa::{Pkcs1v15Encrypt, Pkcs1v15Sign, RsaPublicKey};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::protocol::crypto::KEY_TABLE;
    use crate::protocol::frame::MAX_UPSTREAM_PAYLOAD;
    use crate::protocol::handshake::AdbRawKeyExchange;
    use crate::protocol::proto::SspHeartBeatResponse;

    type Aes256CbcDecryptor = cbc::Decryptor<Aes256>;

    async fn read_upstream(stream: &mut TcpStream) -> (u32, u8, Vec<u8>) {
        let mut header = [0_u8; 9];
        stream
            .read_exact(&mut header)
            .await
            .expect("upstream header");
        let sid = u32::from_be_bytes(header[..4].try_into().expect("sid"));
        let flag = header[4];
        let length = u32::from_be_bytes(header[5..].try_into().expect("length")) as usize;
        let mut payload = vec![0_u8; length];
        stream
            .read_exact(&mut payload)
            .await
            .expect("upstream payload");
        (sid, flag, payload)
    }

    async fn write_normal(stream: &mut TcpStream, sid: u32, body: &[u8]) {
        let mut payload = (body.len() as u64).to_be_bytes().to_vec();
        payload.extend_from_slice(body);
        for chunk in payload.chunks(5) {
            stream.write_all(&sid.to_be_bytes()).await.expect("sid");
            stream
                .write_all(&(chunk.len() as u16).to_be_bytes())
                .await
                .expect("chunk length");
            stream.write_all(chunk).await.expect("chunk");
        }
    }

    #[tokio::test]
    async fn normal_response_prefix_can_span_chunks() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let body = b"hello";
        sender
            .send(Ok((body.len() as u64).to_be_bytes()[..3].to_vec()))
            .unwrap();
        let mut second = (body.len() as u64).to_be_bytes()[3..].to_vec();
        second.extend_from_slice(body);
        sender.send(Ok(second)).unwrap();
        assert_eq!(
            receive_normal(&mut receiver, Duration::from_secs(1))
                .await
                .expect("response"),
            body
        );
    }

    #[tokio::test]
    async fn normal_response_rejects_overshoot() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut bytes = 1_u64.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"xx");
        sender.send(Ok(bytes)).unwrap();
        assert!(
            receive_normal(&mut receiver, Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn writer_queue_wait_respects_request_timeout() {
        let (writer, _receiver) = mpsc::channel(1);
        let (completion, _) = oneshot::channel();
        writer
            .send(WriteCommand {
                sid: 1,
                flag: 1,
                payload: Vec::new(),
                completion,
            })
            .await
            .unwrap();
        let inner = Arc::new(SessionInner {
            writer,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_0001),
            keys: Arc::new(SessionKeys::generate().unwrap()),
            request_timeout: Duration::from_millis(20),
            closed: Arc::new(AtomicBool::new(false)),
            wire_log: None,
        });
        let error = inner
            .send(0x8000_0002, 1, b"blocked")
            .await
            .expect_err("timeout");
        assert!(matches!(error, Error::Timeout(_)));
        assert!(inner.closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn canceling_caller_does_not_cancel_enqueued_frame_write() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address);
        let server = listener.accept();
        let (client, server) = tokio::join!(client, server);
        let client = client.unwrap();
        let (mut server, _) = server.unwrap();
        let (_, writer) = client.into_split();
        let closed = Arc::new(AtomicBool::new(false));
        let (writer_sender, writer_receiver) = mpsc::channel(64);
        let writer_task = spawn_writer(writer, writer_receiver, Arc::clone(&closed), None);
        let inner = Arc::new(SessionInner {
            writer: writer_sender,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_0001),
            keys: Arc::new(SessionKeys::generate().unwrap()),
            request_timeout: Duration::from_secs(5),
            closed,
            wire_log: None,
        });
        let payload = vec![0x5a; MAX_UPSTREAM_PAYLOAD];
        let send_inner = Arc::clone(&inner);
        let send_task =
            tokio::spawn(async move { send_inner.send(0x8000_0002, 3, &payload).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !send_task.is_finished(),
            "write should still be in progress"
        );
        send_task.abort();

        let receive = async {
            let mut header = [0_u8; 9];
            server.read_exact(&mut header).await.unwrap();
            assert_eq!(header[4], 3);
            let length = u32::from_be_bytes(header[5..].try_into().unwrap()) as usize;
            assert_eq!(length, MAX_UPSTREAM_PAYLOAD);
            let mut bytes = vec![0_u8; length];
            server.read_exact(&mut bytes).await.unwrap();
            assert!(bytes.iter().all(|byte| *byte == 0x5a));
        };
        timeout(Duration::from_secs(5), receive)
            .await
            .expect("complete frame");
        writer_task.abort();
    }

    #[test]
    fn download_discriminator_detects_split_envelope_without_field_one() {
        let body = [0x10, 0x01];
        let mut message = (body.len() as u64).to_be_bytes().to_vec();
        message.extend_from_slice(&body);
        let mut discriminator = DownloadDiscriminator::default();
        assert_eq!(
            discriminator.push(&message[..5], 100).expect("first part"),
            None
        );
        let error = discriminator
            .push(&message[5..], 100)
            .expect_err("push collision");
        assert!(matches!(error, Error::Protocol(_)));
    }

    #[test]
    fn download_discriminator_preserves_short_raw_file() {
        let mut discriminator = DownloadDiscriminator::default();
        assert_eq!(
            discriminator.push(b"short", 5).expect("raw"),
            Some(b"short".to_vec())
        );
    }

    #[tokio::test]
    async fn raw_handshake_and_signed_request_round_trip() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let (handshake_sid, handshake_flag, payload) = read_upstream(&mut stream).await;
            assert_eq!(handshake_sid, 0x8000_0001);
            assert_eq!(handshake_flag, 0);

            let clear =
                Aes256CbcDecryptor::new((&KEY_TABLE[16..48]).into(), (&KEY_TABLE[..16]).into())
                    .decrypt_padded_vec_mut::<Pkcs7>(&payload[16..])
                    .expect("AES public key");
            let public_der = base64::engine::general_purpose::STANDARD
                .decode(clear)
                .expect("base64 public key");
            assert_eq!(&payload[..16], &Md5::digest(&public_der)[..]);
            let public = RsaPublicKey::from_pkcs1_der(&public_der).expect("RSA public key");
            let encrypted = public
                .encrypt(&mut OsRng, Pkcs1v15Encrypt, b"ok")
                .expect("encrypt handshake result");
            let encoded = base64::engine::general_purpose::STANDARD.encode(encrypted);
            write_normal(&mut stream, handshake_sid, encoded.as_bytes()).await;

            let (request_sid, request_flag, payload) = read_upstream(&mut stream).await;
            assert_eq!(request_sid, 0x8000_0002);
            assert_eq!(request_flag, 1);
            let (signature, protobuf) = payload.split_at(128);
            public
                .verify(
                    Pkcs1v15Sign::new::<Sha256>(),
                    &Sha256::digest(protobuf),
                    signature,
                )
                .expect("request signature");
            let request = SspHeartBeatRequest::decode(protobuf).expect("heartbeat request");
            let response = SspHeartBeatResponse {
                r#type: None,
                host_timestamp: request.host_timestamp,
                client_timestamp: Some(42),
            }
            .encode_to_vec();
            write_normal(&mut stream, request_sid, &response).await;

            let (_, quit_flag, quit_payload) = read_upstream(&mut stream).await;
            assert_eq!(quit_flag, 1);
            assert!(quit_payload.len() >= 128);
        });

        let stream = TcpStream::connect(address).await.expect("connect");
        let session = Session::establish(
            stream,
            Duration::from_secs(2),
            Duration::from_secs(60),
            None,
            &AdbRawKeyExchange,
        )
        .await
        .expect("session");
        let request = SspHeartBeatRequest {
            r#type: Some(SspRequestType::HeartBeatRequest as i32),
            host_timestamp: Some(21),
        };
        let body = session.request(&request).await.expect("response");
        let response = SspHeartBeatResponse::decode(body.as_slice()).expect("decode response");
        assert_eq!(response.r#type, None, "field 1 may be absent");
        assert_eq!(response.host_timestamp, Some(21));
        assert_eq!(response.client_timestamp, Some(42));
        session.close().await.expect("close");
        server.await.expect("server");
    }
}
