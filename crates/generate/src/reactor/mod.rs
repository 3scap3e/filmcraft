//! Reactor's FastH3 model as a [`VideoGenerator`]: the same flow as Reactor's reference FastH3
//! app, in Rust. A token is minted from the API key; a session is created and polled until a GPU
//! is ready; a WebRTC connection carries two data channels (`control`, `data`); the model is set
//! to autoplay on a 4:3 canvas, the optional still is uploaded, the clip is enqueued and followed
//! through `clip_started` / `clip_finished`; `requestClip` then names an HLS playlist of the
//! playback, which is downloaded and assembled into one MP4. The session is ended on every exit,
//! because it is billed by the second.

mod http;
mod peer;
mod wire;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use prost::Message;
use serde_json::{Value, json};

use crate::{GenerateError, GenerateRequest, GeneratedClip, ImageRole, Limits, Phase, Progress, VideoGenerator};
use http::{ClientInfo, Http, TrackCapability};
use peer::{Inbound, Peer};
use wire::{control_client_payload, control_server_payload, data_client_payload, data_server_payload};

pub const MODEL: &str = "reactor/fast-h3";
pub const GENERATOR_ID: &str = "reactor-fast-h3";
pub const GENERATOR_NAME: &str = "Reactor FastH3";
pub const MIN_SECONDS: f64 = 5.167;
pub const MAX_SECONDS: f64 = 14.375;
pub const DEFAULT_SECONDS: f64 = 8.0;
pub const MAX_PROMPT_CHARS: usize = 800;
pub const PRICE_PER_SECOND_USD: f64 = 0.0125;
const CANVAS_ASPECT: &str = "4:3";
const CANVAS: (u32, u32) = (1024, 768);
const FPS: f64 = 24.0;
const TOKEN_LIFETIME_SECONDS: u64 = 3600;
const SESSION_POLL_ATTEMPTS: u32 = 20;
const SDP_POLL_ATTEMPTS: u32 = 6;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const CLIP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(10);
/// Longest wait for the clip to build and play, to protect the user's credits.
const SAFETY_TIMEOUT: Duration = Duration::from_secs(6 * 60);
/// How long past the runtime's own estimate the playlist may still say "not ready".
const PLAYLIST_SLACK: Duration = Duration::from_secs(60);
const MAX_IMAGE_BYTES: u64 = 32 << 20;

/// Where the API key comes from (the engine reads the user's preferences each time).
pub type KeySource = Box<dyn Fn() -> Option<String> + Send + Sync>;

pub struct FastH3 {
    key: KeySource,
    api_url: String,
}

impl FastH3 {
    pub fn new(key: KeySource) -> Self {
        FastH3 { key, api_url: http::API_URL.to_string() }
    }

    pub fn with_api_url(mut self, url: &str) -> Self {
        self.api_url = url.trim_end_matches('/').to_string();
        self
    }

    fn api_key(&self) -> Result<String, GenerateError> {
        (self.key)().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).ok_or(GenerateError::NoApiKey)
    }
}

pub fn limits() -> Limits {
    crate::request::fast_h3_limits()
}

impl VideoGenerator for FastH3 {
    fn id(&self) -> &str {
        GENERATOR_ID
    }
    fn name(&self) -> &str {
        GENERATOR_NAME
    }
    fn limits(&self) -> Limits {
        limits()
    }
    fn ready(&self) -> Result<(), GenerateError> {
        self.api_key().map(|_| ())
    }

    fn generate(&self, request: &GenerateRequest, progress: &Progress) -> Result<GeneratedClip, GenerateError> {
        crate::request::validate(request, &limits())?;
        let key = self.api_key()?;
        let image = match &request.image {
            Some(p) => Some(read_image(p)?),
            None => None,
        };
        let http = Http::new();
        progress.set_phase(Phase::Token);
        progress.set_detail("Signing in to Reactor");
        let jwt = http.token(&self.api_url, &key, MODEL, TOKEN_LIFETIME_SECONDS)?;
        drop(key);
        let client = ClientInfo::filmcraft();

        progress.set_phase(Phase::Connecting);
        progress.set_detail("Creating a session");
        let session = http.create_session(&self.api_url, &jwt, MODEL, &client)?;
        let session_id = session.session_id.clone();
        let opened = Instant::now();
        let ticker = SessionClock { opened, progress };
        let result = (|| {
            let ready = self.wait_session_ready(&http, &jwt, &session_id, session, progress, &ticker)?;
            let tracks = ready.capabilities.map(|c| c.tracks).filter(|t| !t.is_empty()).unwrap_or_else(preset_tracks);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("filmcraft-reactor")
                .build()
                .map_err(|e| GenerateError::Io(format!("async runtime: {e}")))?;
            let outcome = runtime.block_on(run_session(&http, &self.api_url, &jwt, &session_id, &client, tracks, request, image, progress, &ticker));
            runtime.shutdown_timeout(Duration::from_secs(5));
            outcome
        })();
        progress.set_detail("Ending the session");
        if let Err(e) = http.terminate_session(&self.api_url, &jwt, &session_id) {
            log::warn!("reactor: ending session {session_id}: {e}");
        }
        let session_seconds = opened.elapsed().as_secs_f64();
        ticker.tick();
        let (ready, clip) = result?;

        progress.set_phase(Phase::Saving);
        progress.set_detail("Downloading the clip");
        let playlist_url = absolute(&self.api_url, &ready.playlist_url);
        let body = self.fetch_playlist(&http, &jwt, &playlist_url, ready.predicted_ready_at_ms, progress)?;
        let list = crate::hls::parse(&body, &playlist_url)?;
        let init = http.segment(&list.init_url)?;
        let mut segments = Vec::with_capacity(list.segment_urls.len());
        let mut bytes = init.len() as u64;
        progress.bytes_saved.store(bytes, std::sync::atomic::Ordering::Relaxed);
        for (i, url) in list.segment_urls.iter().enumerate() {
            if progress.cancelled() {
                return Err(GenerateError::Cancelled);
            }
            let seg = http.segment(url)?;
            bytes = bytes.saturating_add(seg.len() as u64);
            progress.bytes_saved.store(bytes, std::sync::atomic::Ordering::Relaxed);
            progress.set_detail(format!("Downloading the clip ({} of {} parts)", i + 1, list.segment_urls.len()));
            segments.push(seg);
        }
        let mp4 = crate::hls::assemble(&init, &segments)?;
        if let Some(dir) = request.output.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&request.output, &mp4)?;
        progress.set_phase(Phase::Done);
        progress.set_detail(format!("Saved {}", request.output.display()));
        Ok(GeneratedClip {
            path: request.output.clone(),
            seconds: clip.seconds,
            width: clip.width,
            height: clip.height,
            fps: FPS,
            provider_clip_id: clip.clip_id,
            seed: clip.seed,
            session_seconds,
        })
    }
}

/// Keeps `Progress::session_ms` current.
struct SessionClock<'a> {
    opened: Instant,
    progress: &'a Progress,
}

impl SessionClock<'_> {
    fn tick(&self) {
        self.progress.session_ms.store(self.opened.elapsed().as_millis().min(u64::MAX as u128) as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

/// FastH3's declared tracks, used when the session reports none.
fn preset_tracks() -> Vec<TrackCapability> {
    vec![
        TrackCapability { name: "main_video".into(), kind: "video".into(), direction: "recvonly".into() },
        TrackCapability { name: "main_audio".into(), kind: "audio".into(), direction: "recvonly".into() },
    ]
}

struct Image {
    name: String,
    mime: String,
    bytes: Vec<u8>,
}

fn read_image(path: &Path) -> Result<Image, GenerateError> {
    let meta = std::fs::metadata(path).map_err(|e| GenerateError::BadRequest(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(GenerateError::BadRequest(format!("{}: the image is larger than {} MB", path.display(), MAX_IMAGE_BYTES >> 20)));
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => return Err(GenerateError::BadRequest(format!("{}: not a PNG, JPEG, WebP, GIF, BMP or TIFF image", path.display()))),
    };
    let bytes = std::fs::read(path).map_err(|e| GenerateError::BadRequest(format!("{}: {e}", path.display())))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("frame").to_string();
    Ok(Image { name, mime: mime.to_string(), bytes })
}

/// A delay doubling from `initial` up to `max`.
fn backoff(attempt: u32, initial: Duration, max: Duration) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    initial.checked_mul(factor).unwrap_or(max).min(max)
}

/// Sleep in short steps so a cancel is seen quickly.
fn sleep_cancellable(d: Duration, progress: &Progress) -> Result<(), GenerateError> {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if progress.cancelled() {
            return Err(GenerateError::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(100).min(end.saturating_duration_since(Instant::now())));
    }
    Ok(())
}

impl FastH3 {
    fn wait_session_ready(
        &self,
        http: &Http,
        jwt: &str,
        session_id: &str,
        first: http::SessionResponse,
        progress: &Progress,
        clock: &SessionClock,
    ) -> Result<http::SessionResponse, GenerateError> {
        let mut current = first;
        for attempt in 0..SESSION_POLL_ATTEMPTS {
            clock.tick();
            if current.is_terminal() {
                return Err(GenerateError::Refused(format!("the session ended before it was ready (state {})", current.state)));
            }
            if current.is_ready() {
                return Ok(current);
            }
            progress.set_detail(format!("Waiting for a GPU (state {})", current.state.to_ascii_lowercase()));
            sleep_cancellable(backoff(attempt, Duration::from_millis(200), Duration::from_secs(10)), progress)?;
            current = http.get_session(&self.api_url, jwt, session_id)?;
        }
        Err(GenerateError::Timeout("no GPU became ready for the session".into()))
    }

    fn fetch_playlist(&self, http: &Http, jwt: &str, url: &str, predicted_ready_at_ms: i64, progress: &Progress) -> Result<String, GenerateError> {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let wait_ms = predicted_ready_at_ms.saturating_sub(now_ms).clamp(0, 60_000) as u64;
        if wait_ms > 0 {
            progress.set_detail("Waiting for the clip to finalize");
            sleep_cancellable(Duration::from_millis(wait_ms), progress)?;
        }
        let deadline = Instant::now() + PLAYLIST_SLACK;
        loop {
            match http.playlist(url, jwt)? {
                Some((body, _)) => return Ok(body),
                None => {
                    if Instant::now() > deadline {
                        return Err(GenerateError::Timeout("the clip's playlist never became ready".into()));
                    }
                    sleep_cancellable(Duration::from_millis(1000), progress)?;
                }
            }
        }
    }
}

/// `url` as an absolute URL (the runtime may return a path relative to the coordinator).
fn absolute(api_url: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    format!("{}/{}", api_url.trim_end_matches('/'), url.trim_start_matches('/'))
}

/// What the model told us about the clip we asked for.
#[derive(Clone, Debug, Default)]
struct ClipInfo {
    clip_id: String,
    seconds: f64,
    seed: Option<u64>,
    width: u32,
    height: u32,
}

/// One decoded event from either channel.
enum Event {
    ControlOpen,
    DataOpen,
    Closed(String),
    Model { request_id: String, message: wire::ModelMessage },
    DataError { request_id: String, error: wire::Error },
    Control { request_id: String, payload: control_server_payload::Payload },
}

struct Flow<'a> {
    http: &'a Http,
    api_url: &'a str,
    jwt: &'a str,
    session_id: &'a str,
    client: &'a ClientInfo,
    peer: Peer,
    progress: &'a Progress,
    clock: &'a SessionClock<'a>,
    connection_id: Option<u32>,
    ice_buffer: Vec<http::IceCandidate>,
    gathering_complete: bool,
    ice_final_sent: bool,
    pending: std::collections::VecDeque<Event>,
    next_request: u64,
    control_open: bool,
    data_open: bool,
    connected: bool,
}

impl Flow<'_> {
    fn request_id(&mut self, prefix: &str) -> String {
        self.next_request = self.next_request.saturating_add(1);
        format!("{prefix}_{}", self.next_request)
    }

    fn flush_ice(&mut self) -> Result<(), GenerateError> {
        let Some(cid) = self.connection_id else { return Ok(()) };
        let is_final = self.gathering_complete && !self.ice_final_sent;
        if self.ice_buffer.is_empty() && !is_final {
            return Ok(());
        }
        let candidates = std::mem::take(&mut self.ice_buffer);
        self.http.send_ice(self.api_url, self.jwt, self.session_id, cid, &candidates, is_final, self.client)?;
        if is_final {
            self.ice_final_sent = true;
        }
        Ok(())
    }

    /// The next event, handling transport housekeeping (ICE trickle, connection state) on the way.
    /// `None` when `timeout` passes.
    async fn next(&mut self, timeout: Duration) -> Result<Option<Event>, GenerateError> {
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.progress.cancelled() {
                return Err(GenerateError::Cancelled);
            }
            self.clock.tick();
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let slice = (deadline - now).min(Duration::from_millis(250));
            let Ok(inbound) = tokio::time::timeout(slice, self.peer.recv()).await else { continue };
            let Some(inbound) = inbound else {
                return Err(GenerateError::Network("the connection closed".into()));
            };
            match inbound {
                Inbound::Ice(c) => {
                    self.ice_buffer.push(c);
                    self.flush_ice()?;
                }
                Inbound::GatheringComplete => {
                    self.gathering_complete = true;
                    self.flush_ice()?;
                }
                Inbound::Connected => self.connected = true,
                Inbound::Failed(state) => return Err(GenerateError::Network(format!("the WebRTC connection {state}"))),
                Inbound::ControlOpen => {
                    self.control_open = true;
                    return Ok(Some(Event::ControlOpen));
                }
                Inbound::DataOpen => {
                    self.data_open = true;
                    return Ok(Some(Event::DataOpen));
                }
                Inbound::Closed(label) => return Ok(Some(Event::Closed(label))),
                Inbound::Data(bytes) => match wire::DataServerMessage::decode(&bytes[..]) {
                    Ok(m) => match m.payload {
                        Some(data_server_payload::Payload::Message(message)) => return Ok(Some(Event::Model { request_id: m.request_id, message })),
                        Some(data_server_payload::Payload::Error(error)) => return Ok(Some(Event::DataError { request_id: m.request_id, error })),
                        None => {}
                    },
                    Err(e) => log::warn!("reactor: undecodable data message: {e}"),
                },
                Inbound::Control(bytes) => match wire::ControlServerMessage::decode(&bytes[..]) {
                    Ok(m) => {
                        if let Some(payload) = m.payload {
                            return Ok(Some(Event::Control { request_id: m.request_id, payload }));
                        }
                    }
                    Err(e) => log::warn!("reactor: undecodable control message: {e}"),
                },
            }
        }
    }

    async fn wait_ready(&mut self) -> Result<(), GenerateError> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        while !(self.connected && self.control_open && self.data_open) {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(GenerateError::Timeout("the WebRTC connection did not come up in 30 s".into()));
            }
            match self.next(left).await? {
                None => return Err(GenerateError::Timeout("the WebRTC connection did not come up in 30 s".into())),
                Some(Event::Closed(l)) => return Err(GenerateError::Network(format!("the {l} channel closed"))),
                Some(Event::ControlOpen | Event::DataOpen) => {}
                Some(other) => self.pending.push_back(other),
            }
        }
        Ok(())
    }

    async fn send_control(&mut self, kind: wire::MessageKind, request_id: String, payload: control_client_payload::Payload) -> Result<(), GenerateError> {
        let m = wire::ControlClientMessage { request_id, kind: kind as i32, payload: Some(payload) };
        self.peer.send_control(wire::encode(&m)).await
    }

    /// Send a model command and wait for its correlated reply (a message or an error).
    async fn command(&mut self, name: &str, data: Value, uploads: BTreeMap<String, wire::UploadReference>) -> Result<wire::ModelMessage, GenerateError> {
        let id = self.request_id("data");
        let data = wire::struct_of(&data).ok_or_else(|| GenerateError::Protocol(format!("{name}: the command data is not an object")))?;
        let m = wire::DataClientMessage {
            request_id: id.clone(),
            kind: wire::MessageKind::Request as i32,
            payload: Some(data_client_payload::Payload::Command(wire::Command { r#type: name.to_string(), data: Some(data), uploads })),
        };
        self.peer.send_data(wire::encode(&m)).await?;
        let deadline = tokio::time::Instant::now() + COMMAND_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next(left).await? {
                None => return Err(GenerateError::Timeout(format!("no reply to `{name}` in 10 s"))),
                Some(Event::Model { request_id, message }) if request_id == id => {
                    if message.r#type == "command_error" {
                        let reason = wire::json_of(message.data.as_ref())["reason"].as_str().unwrap_or("rejected").to_string();
                        return Err(GenerateError::Refused(format!("`{name}`: {reason}")));
                    }
                    return Ok(message);
                }
                Some(Event::DataError { request_id, error }) if request_id == id => {
                    return Err(GenerateError::Refused(format!("`{name}`: {} ({})", error.message, error.code)));
                }
                Some(Event::Closed(l)) => return Err(GenerateError::Network(format!("the {l} channel closed"))),
                Some(other) => self.pending.push_back(other),
            }
        }
    }

    /// `requestClip(seconds)` on the control channel.
    async fn request_clip(&mut self, seconds: f64) -> Result<wire::ClipReady, GenerateError> {
        let id = self.request_id("ctrl");
        self.send_control(
            wire::MessageKind::Request,
            id.clone(),
            control_client_payload::Payload::RequestClip(wire::RequestClip { duration_seconds: seconds }),
        )
        .await?;
        let deadline = tokio::time::Instant::now() + CLIP_REQUEST_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next(left).await? {
                None => return Err(GenerateError::Timeout("no answer to the clip request in 10 s".into())),
                Some(Event::Control { request_id, payload }) if request_id == id => {
                    return match payload {
                        control_server_payload::Payload::ClipReady(r) => Ok(r),
                        control_server_payload::Payload::ClipFailed(f) => Err(GenerateError::Failed(format!("the recording failed: {}", f.reason))),
                        control_server_payload::Payload::Error(e) => Err(GenerateError::Refused(format!("clip request: {} ({})", e.message, e.code))),
                        _ => Err(GenerateError::Protocol("unexpected answer to the clip request".into())),
                    };
                }
                Some(Event::Closed(l)) => return Err(GenerateError::Network(format!("the {l} channel closed"))),
                Some(other) => self.pending.push_back(other),
            }
        }
    }
}

/// A model event about one clip.
fn clip_of(message: &wire::ModelMessage) -> Option<(String, Value)> {
    let data = wire::json_of(message.data.as_ref());
    let clip = data.get("clip")?.clone();
    let id = clip.get("clip_id")?.as_str()?.to_string();
    Some((id, clip))
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    http: &Http,
    api_url: &str,
    jwt: &str,
    session_id: &str,
    client: &ClientInfo,
    tracks: Vec<TrackCapability>,
    request: &GenerateRequest,
    image: Option<Image>,
    progress: &Progress,
    clock: &SessionClock<'_>,
) -> Result<(wire::ClipReady, ClipInfo), GenerateError> {
    progress.set_detail("Connecting");
    let ice = http.ice_servers(api_url, jwt, session_id)?;
    let (peer, offer, mapping) = Peer::connect(&ice, &tracks).await?;
    let mut flow = Flow {
        http,
        api_url,
        jwt,
        session_id,
        client,
        peer,
        progress,
        clock,
        connection_id: None,
        ice_buffer: Vec::new(),
        gathering_complete: false,
        ice_final_sent: false,
        pending: Default::default(),
        next_request: 0,
        control_open: false,
        data_open: false,
        connected: false,
    };
    let result = drive(&mut flow, offer, mapping, request, image).await;
    if let Err(e) = flow.peer.close().await {
        log::warn!("reactor: closing the peer connection: {e}");
    }
    result
}

async fn drive(
    flow: &mut Flow<'_>,
    offer: String,
    mapping: Vec<http::TrackMappingEntry>,
    request: &GenerateRequest,
    image: Option<Image>,
) -> Result<(wire::ClipReady, ClipInfo), GenerateError> {
    let cid = flow.http.register_connection(flow.api_url, flow.jwt, flow.session_id)?;
    flow.connection_id = Some(cid);
    flow.http.send_offer(flow.api_url, flow.jwt, flow.session_id, cid, &offer, &mapping, flow.client)?;
    flow.flush_ice()?;
    let mut answer = None;
    for attempt in 0..SDP_POLL_ATTEMPTS {
        if let Some(a) = flow.http.poll_answer(flow.api_url, flow.jwt, flow.session_id, cid)? {
            answer = Some(a);
            break;
        }
        let wait = backoff(attempt, Duration::from_millis(200), Duration::from_secs(15));
        let deadline = tokio::time::Instant::now() + wait;
        while tokio::time::Instant::now() < deadline {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match flow.next(left).await? {
                None => break,
                Some(Event::Closed(l)) => return Err(GenerateError::Network(format!("the {l} channel closed"))),
                Some(other) => flow.pending.push_back(other),
            }
        }
    }
    let answer = answer.ok_or_else(|| GenerateError::Timeout("no SDP answer from the runtime".into()))?;
    if let Some(new_cid) = answer.connection_id {
        flow.connection_id = Some(new_cid);
    }
    flow.peer.set_answer(&answer.sdp_answer).await?;
    flow.flush_ice()?;
    flow.wait_ready().await?;
    flow.progress.set_detail("Connected");

    let heartbeat = flow.peer.control_channel();
    let beat = tokio::spawn(async move {
        loop {
            let m = wire::ControlClientMessage {
                request_id: String::new(),
                kind: wire::MessageKind::Notification as i32,
                payload: Some(control_client_payload::Payload::Ping(wire::Ping {})),
            };
            if heartbeat.send(bytes::BytesMut::from(&wire::encode(&m)[..])).await.is_err() {
                break;
            }
            tokio::time::sleep(HEARTBEAT).await;
        }
    });
    let outcome = converse(flow, request, image).await;
    beat.abort();
    outcome
}

async fn converse(flow: &mut Flow<'_>, request: &GenerateRequest, image: Option<Image>) -> Result<(wire::ClipReady, ClipInfo), GenerateError> {
    let mut info = ClipInfo { width: CANVAS.0, height: CANVAS.1, ..Default::default() };
    flow.command("set_autoplay", json!({"enabled": true}), Default::default()).await?;
    let canvas = flow.command("set_canvas", json!({"aspect": CANVAS_ASPECT}), Default::default()).await?;
    let c = wire::json_of(canvas.data.as_ref());
    if let (Some(w), Some(h)) = (c["width"].as_u64(), c["height"].as_u64()) {
        info.width = u32::try_from(w).unwrap_or(CANVAS.0);
        info.height = u32::try_from(h).unwrap_or(CANVAS.1);
    }

    let mut uploads = BTreeMap::new();
    if let Some(img) = image {
        flow.progress.set_phase(Phase::Uploading);
        flow.progress.set_detail(format!("Uploading {}", img.name));
        let created = flow.http.create_upload(flow.api_url, flow.jwt, flow.session_id, &img.name, img.bytes.len() as u64, &img.mime)?;
        flow.http.put_upload(&created.presigned_url, &img.mime, &img.bytes)?;
        let size = i64::try_from(img.bytes.len()).unwrap_or(i64::MAX);
        let reference = wire::UploadReference { upload_id: created.presigned_id.clone(), name: img.name.clone(), mime_type: img.mime.clone(), size };
        flow.send_control(
            wire::MessageKind::Notification,
            String::new(),
            control_client_payload::Payload::FileUploaded(wire::FileUploaded { upload_id: created.presigned_id, name: img.name, mime_type: img.mime, size }),
        )
        .await?;
        if request.image_role != ImageRole::Last {
            uploads.insert("starting_frame".to_string(), reference.clone());
        }
        if request.image_role != ImageRole::First {
            uploads.insert("ending_frame".to_string(), reference);
        }
    }

    flow.progress.set_phase(Phase::Queued);
    flow.progress.set_detail("Queueing the clip");
    let mut data = json!({"prompt": request.prompt.trim(), "seconds": request.seconds});
    if let Some(seed) = request.seed {
        data["seed"] = json!(seed);
    }
    let queued = flow.command("enqueue", data, uploads).await?;
    let (clip_id, clip) = clip_of(&queued).ok_or_else(|| GenerateError::Protocol("`enqueue` replied without a clip".into()))?;
    info.clip_id = clip_id.clone();
    info.seconds = clip["seconds"].as_f64().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(request.seconds);
    info.seed = clip["seed"].as_u64();

    flow.progress.set_phase(Phase::Generating);
    flow.progress.set_detail(format!("Generating {:.1} s of video", info.seconds));
    let deadline = tokio::time::Instant::now() + SAFETY_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(GenerateError::Timeout("stopped after 6 minutes without a finished clip, to protect your credits".into()));
        }
        match flow.next(left).await? {
            None => continue,
            Some(Event::Closed(l)) => return Err(GenerateError::Network(format!("the {l} channel closed"))),
            Some(Event::Control { payload: control_server_payload::Payload::SessionEnded(e), .. }) => {
                return Err(GenerateError::Network(format!("the session ended: {}", e.reason)));
            }
            Some(Event::Control { payload: control_server_payload::Payload::Moderation(m), .. }) => {
                if m.action.eq_ignore_ascii_case("blocked") || m.action.eq_ignore_ascii_case("rejected") {
                    return Err(GenerateError::Refused(format!("moderation: {} ({})", m.message, m.categories.join(", "))));
                }
                log::info!("reactor moderation {}: {}", m.action, m.message);
            }
            Some(Event::Model { message, .. }) => {
                let Some((id, clip)) = clip_of(&message) else { continue };
                if id != clip_id {
                    continue;
                }
                match message.r#type.as_str() {
                    "clip_generated" => {
                        flow.progress.set_detail("Built, waiting for playback");
                        if let Some(s) = clip["seconds"].as_f64().filter(|s| s.is_finite() && *s > 0.0) {
                            info.seconds = s;
                        }
                    }
                    "clip_started" => {
                        flow.progress.set_phase(Phase::Playing);
                        flow.progress.set_detail(format!("Receiving {:.1} s of video", info.seconds));
                    }
                    "clip_finished" | "clip_stopped" => break,
                    "clip_failed" => {
                        let reason = wire::json_of(message.data.as_ref())["reason"].as_str().unwrap_or("unknown reason").to_string();
                        return Err(GenerateError::Failed(reason));
                    }
                    "clip_popped" => return Err(GenerateError::Failed("the clip was removed from the queue".into())),
                    _ => {}
                }
            }
            Some(_) => {}
        }
    }

    flow.progress.set_phase(Phase::Saving);
    flow.progress.set_detail("Capturing the clip");
    let ready = flow.request_clip(info.seconds.ceil().max(1.0)).await?;
    Ok((ready, info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_match_the_reference_app() {
        let l = limits();
        assert_eq!((l.min_seconds, l.max_seconds, l.default_seconds), (5.167, 14.375, 8.0));
        assert_eq!((l.width, l.height), (1024, 768));
        assert_eq!(l.max_prompt_chars, 800);
        assert!((l.estimate_usd(8.0) - 0.2).abs() < 1e-9);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(0, Duration::from_millis(200), Duration::from_secs(10)), Duration::from_millis(200));
        assert_eq!(backoff(3, Duration::from_millis(200), Duration::from_secs(10)), Duration::from_millis(1600));
        assert_eq!(backoff(9, Duration::from_millis(200), Duration::from_secs(10)), Duration::from_secs(10));
        assert_eq!(backoff(40, Duration::from_millis(200), Duration::from_secs(10)), Duration::from_secs(10));
        assert_eq!(absolute("https://api.reactor.inc", "/clips/x.m3u8"), "https://api.reactor.inc/clips/x.m3u8");
        assert_eq!(absolute("https://api.reactor.inc", "https://s3/x.m3u8?s=1"), "https://s3/x.m3u8?s=1");
    }

    #[test]
    fn a_generator_without_a_key_is_not_ready_and_refuses_to_bill() {
        let g = FastH3::new(Box::new(|| None));
        assert!(matches!(g.ready(), Err(GenerateError::NoApiKey)));
        let g = FastH3::new(Box::new(|| Some("  ".into())));
        assert!(matches!(g.ready(), Err(GenerateError::NoApiKey)));
        let req = GenerateRequest { prompt: "x".into(), seconds: 8.0, image: None, image_role: ImageRole::First, seed: None, output: "out.mp4".into() };
        assert!(matches!(g.generate(&req, &Progress::default()), Err(GenerateError::NoApiKey)));
        let bad = GenerateRequest { seconds: 1.0, ..req };
        let g = FastH3::new(Box::new(|| Some("rk_test".into())));
        assert!(matches!(g.generate(&bad, &Progress::default()), Err(GenerateError::BadRequest(_))), "validation runs before any network call");
    }

    #[test]
    fn images_are_typed_by_extension_and_bounded() {
        let dir = std::env::temp_dir().join(format!("filmcraft-generate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let img = read_image(&png).unwrap();
        assert_eq!((img.mime.as_str(), img.name.as_str(), img.bytes.len()), ("image/png", "a.png", 4));
        let txt = dir.join("a.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(matches!(read_image(&txt), Err(GenerateError::BadRequest(_))));
        assert!(matches!(read_image(&dir.join("missing.png")), Err(GenerateError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
