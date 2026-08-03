use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest as _, Md5};
use prost::Message;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout};

use crate::error::{Error, Result};
use crate::event_decode::{decode_cancel, decode_event};
use crate::events::{ClientEvent, EventFilter, EventSubscription, event_channel};
use crate::i18n;
use crate::protocol::crypto::SessionKeys;
use crate::protocol::frame::{WireDirection, WireLog, read_downstream, write_upstream};
use crate::protocol::handshake::HandshakeStrategy;
use crate::protocol::proto::{SspHeartBeatRequest, SspRequestType};
use crate::protocol::wifi_handshake::WifiHandshakeInfo;

type ChunkReceiver = mpsc::UnboundedReceiver<Result<Incoming>>;

const PHASE_NORMAL: u8 = 0;
const PHASE_DOWNLOAD_RAW: u8 = 1;

#[derive(Clone)]
struct PendingRequest {
    sender: mpsc::UnboundedSender<Result<Incoming>>,
}

enum Incoming {
    Data(Vec<u8>),
    RemoteCancelled(crate::cancellation::CancellationInfo),
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
    serial: String,
    events: StdMutex<Option<broadcast::Sender<ClientEvent>>>,
}

pub(crate) struct OpenRequest {
    pub sid: u32,
    receiver: ChunkReceiver,
    inner: Arc<SessionInner>,
    phase: u8,
    completed: bool,
    cancellation: Option<crate::cancellation::CancellationToken>,
}

impl OpenRequest {
    pub async fn receive_normal(&mut self) -> Result<Vec<u8>> {
        if self.phase == PHASE_DOWNLOAD_RAW {
            return Err(Error::Protocol(
                i18n::text("session.raw_after_download").to_string(),
            ));
        }
        let result = receive_normal(
            &mut self.receiver,
            self.inner.request_timeout,
            self.cancellation.as_ref(),
            self.sid,
            &self.inner,
        )
        .await;
        if matches!(&result, Err(Error::Cancelled(_))) {
            self.completed = true;
        }
        if result.is_err()
            && !matches!(&result, Err(Error::Cancelled(info)) if !info.connection_closed)
        {
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
                .map_err(|_| Error::Protocol(i18n::text("session.file_too_large").to_string()))?;
            let mut received = 0_usize;
            let mut digest = Md5::new();
            let mut discriminator = DownloadDiscriminator::default();
            while received < total {
                let incoming = receive_incoming(
                    &mut self.receiver,
                    inner.request_timeout,
                    self.cancellation.as_ref(),
                    self.sid,
                    &inner,
                    true,
                )
                .await
                .map_err(|error| match error {
                    Error::Timeout(_) => {
                        Error::Timeout(i18n::text("session.receive_file_chunk").to_string())
                    }
                    other => other,
                })?;
                let chunk = match incoming {
                    Incoming::Data(chunk) => chunk,
                    Incoming::RemoteCancelled(info) => return Err(Error::Cancelled(info)),
                };
                let remaining = total - received;
                let Some(chunk) = discriminator.push(&chunk, remaining)? else {
                    continue;
                };
                let next_received = received.checked_add(chunk.len()).ok_or_else(|| {
                    Error::Protocol(i18n::text("session.download_count_overflow").to_string())
                })?;
                if next_received > total {
                    return Err(Error::Protocol(i18n::format(
                        "session.download_too_long",
                        &[
                            &received.to_string(),
                            &chunk.len().to_string(),
                            &total.to_string(),
                        ],
                    )));
                }
                writer.write_all(&chunk).await.map_err(|error| {
                    Error::LocalIo(i18n::format(
                        "session.write_download_failed",
                        &[&error.to_string()],
                    ))
                })?;
                digest.update(&chunk);
                received = next_received;
                on_progress(received as u64);
            }
            writer.flush().await.map_err(|error| {
                Error::LocalIo(i18n::format(
                    "session.flush_download_failed",
                    &[&error.to_string()],
                ))
            })?;
            Ok(format!("{:x}", digest.finalize()))
        }
        .await;
        if matches!(&result, Err(Error::Cancelled(_))) {
            self.completed = true;
        }
        if result.is_err()
            && !matches!(&result, Err(Error::Cancelled(info)) if !info.connection_closed)
        {
            inner.fail_connection().await;
        }
        result
    }

    pub async fn send(&mut self, flag: u8, payload: &[u8]) -> Result<()> {
        let result = match self.ensure_not_cancelled(false).await {
            Ok(()) => self.inner.send(self.sid, flag, payload).await,
            Err(error) => Err(error),
        };
        if matches!(&result, Err(Error::Cancelled(_))) {
            self.completed = true;
        }
        result
    }

    async fn ensure_not_cancelled(&self, close_connection: bool) -> Result<()> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(crate::cancellation::CancellationToken::is_cancelled)
        {
            self.cancel_local(close_connection).await
        } else {
            Ok(())
        }
    }

    async fn cancel_local(&self, close_connection: bool) -> Result<()> {
        self.inner.pending.lock().await.remove(&self.sid);
        if close_connection {
            self.inner.fail_connection().await;
        }
        let flag_sent = if close_connection {
            false
        } else {
            let result = self.inner.cancel(self.sid).await;
            if result.is_err() {
                tracing::debug!(
                    sid = self.sid,
                    message = i18n::text("session.cancel_failed")
                );
            }
            result.is_ok()
        };
        Err(Error::Cancelled(crate::cancellation::CancellationInfo {
            sid: self.sid,
            origin: crate::cancellation::CancellationOrigin::Local { flag_sent },
            connection_closed: close_connection,
        }))
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
                    tracing::debug!(%error, sid = format_args!("{sid:#010x}"), message = i18n::text("session.cancel_failed"));
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
    /// WiFi two-round handshake metadata, when the connection used it.
    pub(crate) handshake_info: Option<WifiHandshakeInfo>,
}

impl Session {
    pub async fn establish<S>(
        mut stream: S,
        request_timeout: Duration,
        heartbeat_interval: Duration,
        wire_log: Option<Arc<WireLog>>,
        handshake: &dyn HandshakeStrategy<S>,
        serial: String,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let outcome = handshake
            .establish(&mut stream, request_timeout, wire_log.as_ref())
            .await?;
        let keys = Arc::new(outcome.keys);
        let handshake_info = outcome.wifi;
        let (reader, writer) = tokio::io::split(stream);
        let closed = Arc::new(AtomicBool::new(false));
        let (writer_sender, writer_receiver) = mpsc::channel(64);
        let writer_task = spawn_writer(
            writer,
            writer_receiver,
            Arc::clone(&closed),
            wire_log.clone(),
        );
        let events = event_channel();
        // Business request sids start at 0x8000_1000, deliberately above the
        // phone's push-sid generator (0x8000_0001 +, docs/14 §7.5). The two
        // generators previously collided (both starting at 0x8000_0001),
        // which routed phone pushes into pending requests — first observed
        // on-device 2026-08-03 during M6 photo-sync acceptance.
        let inner = Arc::new(SessionInner {
            writer: writer_sender,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_1000),
            keys,
            request_timeout,
            closed,
            wire_log,
            serial,
            events: StdMutex::new(Some(events)),
        });
        let reader_task = spawn_reader(reader, Arc::clone(&inner));
        let heartbeat_task = spawn_heartbeat(Arc::clone(&inner), heartbeat_interval);
        Ok(Self {
            inner,
            reader_task,
            writer_task,
            heartbeat_task,
            handshake_info,
        })
    }

    pub async fn request<M: Message>(&self, message: &M) -> Result<Vec<u8>> {
        let mut request = self.open(message).await?;
        let response = request.receive_normal().await;
        request.finish().await;
        response
    }

    pub async fn request_with_options<M: Message>(
        &self,
        message: &M,
        options: crate::cancellation::RequestOptions,
    ) -> Result<Vec<u8>> {
        let mut request = self.open_with_options(message, options).await?;
        let response = request.receive_normal().await;
        request.finish().await;
        response
    }

    pub async fn open<M: Message>(&self, message: &M) -> Result<OpenRequest> {
        self.open_with_options(message, crate::cancellation::RequestOptions::default())
            .await
    }

    pub async fn open_with_options<M: Message>(
        &self,
        message: &M,
        options: crate::cancellation::RequestOptions,
    ) -> Result<OpenRequest> {
        let payload = message.encode_to_vec();
        self.inner.open_signed(&payload, options.cancellation).await
    }

    pub fn subscribe_events(&self, filter: EventFilter) -> EventSubscription {
        self.inner.subscribe_events(filter)
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
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.heartbeat_task.abort();
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

impl SessionInner {
    fn subscribe_events(&self, filter: EventFilter) -> EventSubscription {
        let receiver = self
            .events
            .lock()
            .ok()
            .and_then(|events| events.as_ref().map(broadcast::Sender::subscribe));
        match receiver {
            Some(receiver) => EventSubscription::new(receiver, filter),
            None => EventSubscription::closed(filter),
        }
    }

    fn publish_event(&self, event: ClientEvent) {
        if let Ok(events) = self.events.lock()
            && let Some(sender) = events.as_ref()
        {
            let _ = sender.send(event);
        }
    }

    fn next_sid(&self) -> u32 {
        self.next_sid
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    async fn allocate_sid(&self) -> Result<u32> {
        for _ in 0..1024 {
            let sid = self.next_sid();
            if !self.pending.lock().await.contains_key(&sid) {
                return Ok(sid);
            }
        }
        Err(Error::Protocol(
            i18n::text("session.sid_exhausted").to_string(),
        ))
    }

    async fn open_signed(
        self: &Arc<Self>,
        payload: &[u8],
        cancellation: Option<crate::cancellation::CancellationToken>,
    ) -> Result<OpenRequest> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Transport(
                i18n::text("client.connection_closed").to_string(),
            ));
        }
        let sid = self.allocate_sid().await?;
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
            cancellation,
        };
        if request
            .cancellation
            .as_ref()
            .is_some_and(crate::cancellation::CancellationToken::is_cancelled)
        {
            self.pending.lock().await.remove(&sid);
            request.completed = true;
            return Err(Error::Cancelled(crate::cancellation::CancellationInfo {
                sid,
                origin: crate::cancellation::CancellationOrigin::Local { flag_sent: false },
                connection_closed: false,
            }));
        }
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
                .map_err(|_| Error::Transport(i18n::text("session.writer_closed").to_string()))?;
            response
                .await
                .map_err(|_| Error::Transport(i18n::text("session.writer_no_result").to_string()))?
        };
        match timeout(self.request_timeout, send).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                self.fail_connection().await;
                return Err(error);
            }
            Err(_) => {
                self.fail_connection().await;
                return Err(Error::Timeout(i18n::format(
                    "session.send_frame",
                    &[&format!("{sid:#010x}"), &flag.to_string()],
                )));
            }
        };
        Ok(())
    }

    async fn cancel(&self, target_sid: u32) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let cancel_sid = self.allocate_sid().await?;
        self.send(cancel_sid, 2, &target_sid.to_be_bytes()).await
    }

    async fn fail_connection(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.pending.lock().await.clear();
        if let Ok(mut events) = self.events.lock() {
            events.take();
        }
    }
}

fn spawn_writer<S>(
    mut writer: WriteHalf<S>,
    mut receiver: mpsc::Receiver<WriteCommand>,
    closed: Arc<AtomicBool>,
    wire_log: Option<Arc<WireLog>>,
) -> JoinHandle<()>
where
    S: AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(command) = receiver.recv().await {
            let result =
                write_upstream(&mut writer, command.sid, command.flag, &command.payload).await;
            if let Ok(frame) = &result
                && let Some(log) = &wire_log
            {
                log.record(
                    WireDirection::Out,
                    &i18n::format(
                        "wire.outgoing_frame",
                        &[&format!("{:#010x}", command.sid), &command.flag.to_string()],
                    ),
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

fn spawn_reader<S>(mut reader: ReadHalf<S>, inner: Arc<SessionInner>) -> JoinHandle<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut unmatched = HashMap::<u32, NormalAccumulator>::new();
        loop {
            let result = read_downstream(&mut reader).await;
            let (sid, chunk, header) = match result {
                Ok(value) => value,
                Err(error) => {
                    if !inner.closed.load(Ordering::SeqCst) {
                        tracing::debug!(%error, message = i18n::text("session.reader_ended"));
                    }
                    inner.fail_connection().await;
                    break;
                }
            };
            if let Some(log) = &inner.wire_log {
                log.record(
                    WireDirection::In,
                    &i18n::format("wire.incoming_header", &[&format!("{sid:#010x}")]),
                    &header,
                );
                log.record(
                    WireDirection::In,
                    &i18n::format("wire.incoming_chunk", &[&format!("{sid:#010x}")]),
                    &chunk,
                );
            }
            let pending = inner.pending.lock().await.get(&sid).cloned();
            if let Some(pending) = pending {
                if pending.sender.send(Ok(Incoming::Data(chunk))).is_err() {
                    inner.pending.lock().await.remove(&sid);
                }
            } else {
                let accumulator = unmatched.entry(sid).or_default();
                match accumulator.push(&chunk) {
                    Ok(Some(body)) => {
                        unmatched.remove(&sid);
                        if let Some(cancel) = decode_cancel(&body) {
                            let target_sid = cancel
                                .session_id
                                .and_then(|value| u32::try_from(value).ok())
                                .unwrap_or(sid);
                            let info = crate::cancellation::CancellationInfo {
                                sid: target_sid,
                                origin: crate::cancellation::CancellationOrigin::Remote {
                                    error_code: cancel.error_code,
                                },
                                connection_closed: false,
                            };
                            let target = inner.pending.lock().await.remove(&target_sid);
                            if let Some(target) = target {
                                let _ = target.sender.send(Ok(Incoming::RemoteCancelled(info)));
                            } else {
                                inner.publish_event(ClientEvent::RequestCancelled(info));
                            }
                        } else {
                            let event = decode_event(sid, &body, &inner.serial);
                            inner.publish_event(event);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::debug!(sid = format_args!("{sid:#010x}"), %error, message = i18n::text("session.event_invalid"));
                        unmatched.remove(&sid);
                    }
                }
            }
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
            let mut request = match inner.open_signed(&payload, None).await {
                Ok(request) => request,
                Err(error) => {
                    tracing::debug!(%error, message = i18n::text("session.heartbeat_send_failed"));
                    inner.closed.store(true, Ordering::SeqCst);
                    break;
                }
            };
            if let Err(error) = request.receive_normal().await {
                tracing::debug!(%error, message = i18n::text("session.heartbeat_receive_failed"));
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
        if self.bytes.len() > MAX_PUSH_MESSAGE + 8 {
            return Err(Error::Protocol(
                i18n::text("session.event_length_too_large").to_string(),
            ));
        }
        if self.total.is_none() && self.bytes.len() >= 8 {
            let total = u64::from_be_bytes(self.bytes[..8].try_into().expect("eight bytes"));
            if total > MAX_PUSH_MESSAGE as u64 {
                return Err(Error::Protocol(
                    i18n::text("session.event_length_too_large").to_string(),
                ));
            }
            self.total = Some(usize::try_from(total).map_err(|_| {
                Error::Protocol(i18n::text("session.event_length_too_large").to_string())
            })?);
        }
        let Some(total) = self.total else {
            return Ok(None);
        };
        let expected = total.checked_add(8).ok_or_else(|| {
            Error::Protocol(i18n::text("session.event_length_overflow").to_string())
        })?;
        if self.bytes.len() == expected {
            return Ok(Some(self.bytes.split_off(8)));
        }
        if self.bytes.len() > expected {
            return Err(Error::Protocol(
                i18n::text("session.event_too_long").to_string(),
            ));
        }
        Ok(None)
    }
}

const MAX_PUSH_MESSAGE: usize = 4 * 1024 * 1024;

/// Upper bound for a reassembled normal response body. Media libraries and
/// other bulk replies are capped here before buffering, so a hostile device
/// declaring an oversized `total` cannot force unbounded memory use.
const NORMAL_RESPONSE_LIMIT: usize = 64 * 1024 * 1024;

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
            i18n::text("session.download_ambiguous").to_string(),
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
    cancellation: Option<&crate::cancellation::CancellationToken>,
    sid: u32,
    inner: &Arc<SessionInner>,
) -> Result<Vec<u8>> {
    let future = async {
        let mut assembled = Vec::new();
        let mut total = None;
        loop {
            let incoming =
                receive_incoming(receiver, request_timeout, cancellation, sid, inner, false)
                    .await?;
            let chunk = match incoming {
                Incoming::Data(chunk) => chunk,
                Incoming::RemoteCancelled(info) => return Err(Error::Cancelled(info)),
            };
            assembled.extend_from_slice(&chunk);
            if total.is_none() && assembled.len() >= 8 {
                total = Some(u64::from_be_bytes(
                    assembled[..8].try_into().expect("eight bytes"),
                ));
            }
            if let Some(total) = total {
                let total = usize::try_from(total).map_err(|_| {
                    Error::Protocol(i18n::text("session.response_length_too_large").to_string())
                })?;
                if total > NORMAL_RESPONSE_LIMIT {
                    return Err(Error::Protocol(i18n::format(
                        "session.response_cap_exceeded",
                        &[&(total / 1024 / 1024).to_string()],
                    )));
                }
                let expected = total.checked_add(8).ok_or_else(|| {
                    Error::Protocol(i18n::text("session.response_length_overflow").to_string())
                })?;
                if assembled.len() == expected {
                    let body = assembled.split_off(8);
                    if let Some(cancel) = decode_cancel(&body) {
                        return Err(Error::Cancelled(crate::cancellation::CancellationInfo {
                            sid: cancel
                                .session_id
                                .and_then(|value| u32::try_from(value).ok())
                                .unwrap_or(sid),
                            origin: crate::cancellation::CancellationOrigin::Remote {
                                error_code: cancel.error_code,
                            },
                            connection_closed: false,
                        }));
                    }
                    return Ok(body);
                }
                if assembled.len() > expected {
                    return Err(Error::Protocol(
                        i18n::text("session.response_too_long").to_string(),
                    ));
                }
            }
        }
    };
    timeout(request_timeout, future)
        .await
        .map_err(|_| Error::Timeout(i18n::text("session.wait_response").to_string()))?
}

async fn receive_incoming(
    receiver: &mut ChunkReceiver,
    request_timeout: Duration,
    cancellation: Option<&crate::cancellation::CancellationToken>,
    sid: u32,
    inner: &Arc<SessionInner>,
    close_connection_on_cancel: bool,
) -> Result<Incoming> {
    let receive = async {
        let item = if let Some(token) = cancellation {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    inner.pending.lock().await.remove(&sid);
                    if close_connection_on_cancel {
                        inner.fail_connection().await;
                    }
                    let flag_sent = if close_connection_on_cancel {
                        false
                    } else {
                        inner.cancel(sid).await.is_ok()
                    };
                    return Err(Error::Cancelled(crate::cancellation::CancellationInfo {
                        sid,
                        origin: crate::cancellation::CancellationOrigin::Local { flag_sent },
                        connection_closed: close_connection_on_cancel,
                    }));
                }
                item = receiver.recv() => item,
            }
        } else {
            receiver.recv().await
        };
        match item {
            Some(Ok(incoming)) => Ok(incoming),
            Some(Err(error)) => Err(error),
            None => Err(Error::Transport(
                i18n::text("session.response_closed").to_string(),
            )),
        }
    };
    timeout(request_timeout, receive)
        .await
        .map_err(|_| Error::Timeout(i18n::text("session.wait_response").to_string()))?
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
    use tokio::net::{TcpListener, TcpStream};

    use crate::protocol::crypto::KEY_TABLE;
    use crate::protocol::frame::MAX_UPSTREAM_PAYLOAD;
    use crate::protocol::handshake::AdbRawKeyExchange;
    use crate::protocol::proto::SspHeartBeatResponse;

    type Aes256CbcDecryptor = cbc::Decryptor<Aes256>;

    async fn read_upstream<S>(stream: &mut S) -> (u32, u8, Vec<u8>)
    where
        S: AsyncRead + Unpin,
    {
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

    async fn write_normal<S>(stream: &mut S, sid: u32, body: &[u8])
    where
        S: AsyncWrite + Unpin,
    {
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

    fn test_inner(request_timeout: Duration) -> Arc<SessionInner> {
        let (writer, _receiver) = mpsc::channel(1);
        Arc::new(SessionInner {
            writer,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_1000),
            keys: Arc::new(SessionKeys::generate().unwrap()),
            request_timeout,
            closed: Arc::new(AtomicBool::new(false)),
            wire_log: None,
            serial: "test".to_string(),
            events: StdMutex::new(Some(event_channel())),
        })
    }

    #[tokio::test]
    async fn normal_response_prefix_can_span_chunks() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let body = b"hello";
        sender
            .send(Ok(Incoming::Data(
                (body.len() as u64).to_be_bytes()[..3].to_vec(),
            )))
            .unwrap();
        let mut second = (body.len() as u64).to_be_bytes()[3..].to_vec();
        second.extend_from_slice(body);
        sender.send(Ok(Incoming::Data(second))).unwrap();
        let inner = test_inner(Duration::from_secs(1));
        assert_eq!(
            receive_normal(&mut receiver, Duration::from_secs(1), None, 1, &inner)
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
        sender.send(Ok(Incoming::Data(bytes))).unwrap();
        let inner = test_inner(Duration::from_secs(1));
        assert!(
            receive_normal(&mut receiver, Duration::from_secs(1), None, 1, &inner)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn normal_response_rejects_oversized_declared_total() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        // A hostile device declaring a total beyond NORMAL_RESPONSE_LIMIT must
        // be rejected as soon as the 8-byte prefix assembles.
        let total = (NORMAL_RESPONSE_LIMIT as u64) + 1;
        sender
            .send(Ok(Incoming::Data(total.to_be_bytes().to_vec())))
            .unwrap();
        let inner = test_inner(Duration::from_secs(1));
        let error = receive_normal(&mut receiver, Duration::from_secs(1), None, 1, &inner)
            .await
            .expect_err("oversized declared total");
        assert!(matches!(error, Error::Protocol(_)));
    }

    #[tokio::test]
    async fn local_request_cancellation_sends_target_sid_flag() {
        let (writer, mut commands) = mpsc::channel(1);
        let inner = Arc::new(SessionInner {
            writer,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_1000),
            keys: Arc::new(SessionKeys::generate().unwrap()),
            request_timeout: Duration::from_secs(1),
            closed: Arc::new(AtomicBool::new(false)),
            wire_log: None,
            serial: "test".to_string(),
            events: StdMutex::new(Some(event_channel())),
        });
        let (sender, mut receiver) = mpsc::unbounded_channel::<Result<Incoming>>();
        let token = crate::cancellation::CancellationToken::new();
        let receive_inner = Arc::clone(&inner);
        let receive_token = token.clone();
        let task = tokio::spawn(async move {
            receive_normal(
                &mut receiver,
                Duration::from_secs(1),
                Some(&receive_token),
                0x8000_0002,
                &receive_inner,
            )
            .await
        });
        drop(sender);
        token.cancel();

        let command = commands.recv().await.expect("cancel command");
        assert_eq!(command.flag, 2);
        assert_eq!(command.payload, 0x8000_0002_u32.to_be_bytes());
        command.completion.send(Ok(Vec::new())).unwrap();

        let error = task.await.unwrap().expect_err("cancelled");
        assert!(matches!(
            error,
            Error::Cancelled(crate::cancellation::CancellationInfo {
                sid: 0x8000_0002,
                origin: crate::cancellation::CancellationOrigin::Local { flag_sent: true },
                connection_closed: false,
            })
        ));
    }

    #[tokio::test]
    async fn remote_cancel_response_is_not_decoded_as_success() {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Result<Incoming>>();
        let body = crate::protocol::proto::SspCancelRequest {
            r#type: Some(SspRequestType::CancelRequest as i32),
            session_id: Some(0x8000_0002),
            error_code: Some(1),
        }
        .encode_to_vec();
        let mut framed = (body.len() as u64).to_be_bytes().to_vec();
        framed.extend_from_slice(&body);
        sender.send(Ok(Incoming::Data(framed))).unwrap();
        let inner = test_inner(Duration::from_secs(1));
        let error = receive_normal(
            &mut receiver,
            Duration::from_secs(1),
            None,
            0x8000_0002,
            &inner,
        )
        .await
        .expect_err("remote cancellation");
        assert!(matches!(
            error,
            Error::Cancelled(crate::cancellation::CancellationInfo {
                sid: 0x8000_0002,
                origin: crate::cancellation::CancellationOrigin::Remote {
                    error_code: Some(1)
                },
                connection_closed: false,
            })
        ));
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
            next_sid: AtomicU32::new(0x8000_1000),
            keys: Arc::new(SessionKeys::generate().unwrap()),
            request_timeout: Duration::from_millis(20),
            closed: Arc::new(AtomicBool::new(false)),
            wire_log: None,
            serial: "test".to_string(),
            events: StdMutex::new(Some(event_channel())),
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
        let (_, writer) = tokio::io::split(client);
        let closed = Arc::new(AtomicBool::new(false));
        let (writer_sender, writer_receiver) = mpsc::channel(64);
        let writer_task = spawn_writer(writer, writer_receiver, Arc::clone(&closed), None);
        let inner = Arc::new(SessionInner {
            writer: writer_sender,
            pending: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(0x8000_1000),
            keys: Arc::new(SessionKeys::generate().unwrap()),
            request_timeout: Duration::from_secs(5),
            closed,
            wire_log: None,
            serial: "test".to_string(),
            events: StdMutex::new(Some(event_channel())),
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
            assert_eq!(request_sid, 0x8000_1001);
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
            "test".to_string(),
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

    /// The session layer must be transport-agnostic: run the exact same
    /// handshake + signed request round trip over an in-memory duplex stream
    /// instead of TCP, mirroring the USB bulk channel (UsbStream implements
    /// AsyncRead + AsyncWrite over libusb).
    #[tokio::test]
    async fn handshake_and_request_round_trip_over_memory_stream() {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let mut stream = server_side;
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
            assert_eq!(request_sid, 0x8000_1001);
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
                client_timestamp: Some(7),
            }
            .encode_to_vec();
            write_normal(&mut stream, request_sid, &response).await;

            let (_, quit_flag, quit_payload) = read_upstream(&mut stream).await;
            assert_eq!(quit_flag, 1);
            assert!(quit_payload.len() >= 128);
        });

        let session = Session::establish(
            client_side,
            Duration::from_secs(2),
            Duration::from_secs(60),
            None,
            &AdbRawKeyExchange,
            "memory".to_string(),
        )
        .await
        .expect("session over duplex stream");
        let request = SspHeartBeatRequest {
            r#type: Some(SspRequestType::HeartBeatRequest as i32),
            host_timestamp: Some(21),
        };
        let body = session.request(&request).await.expect("response");
        let response = SspHeartBeatResponse::decode(body.as_slice()).expect("decode response");
        assert_eq!(response.r#type, None, "field 1 may be absent");
        assert_eq!(response.host_timestamp, Some(21));
        assert_eq!(response.client_timestamp, Some(7));
        session.close().await.expect("close");
        server.await.expect("server");
    }
}
