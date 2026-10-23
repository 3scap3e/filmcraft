//! The WebRTC side of a Reactor session on webrtc-rs: one peer connection with the `control`
//! and `data` channels (created in that order, as Reactor's own clients do) and a `recvonly`
//! transceiver per model track. Media is negotiated but never decoded here: the clip is fetched
//! afterwards as a file. Everything the connection reports arrives on one [`Inbound`] stream.

use std::sync::Arc;

use bytes::BytesMut;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
    RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription, Registry, register_default_interceptors,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
use webrtc::runtime::TokioRuntime;

use super::http::{IceCandidate, IceServer, TrackCapability, TrackMappingEntry};
use crate::GenerateError;

/// What the connection reports, in order.
#[derive(Debug)]
pub enum Inbound {
    Ice(IceCandidate),
    GatheringComplete,
    Connected,
    /// The connection failed, disconnected or closed (the state's name).
    Failed(String),
    ControlOpen,
    DataOpen,
    /// A channel closed (its label).
    Closed(String),
    Control(Vec<u8>),
    Data(Vec<u8>),
}

struct Handler {
    tx: UnboundedSender<Inbound>,
    first_mid: std::sync::Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(init) => {
                let mid = self.first_mid.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
                let _ = self.tx.send(Inbound::Ice(IceCandidate { candidate: init.candidate, sdp_mid: mid.or(init.sdp_mid), sdp_mline_index: Some(0) }));
            }
            Err(e) => log::warn!("reactor: unserializable ICE candidate: {e}"),
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.tx.send(Inbound::GatheringComplete);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        match state {
            RTCPeerConnectionState::Connected => {
                let _ = self.tx.send(Inbound::Connected);
            }
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Closed => {
                let _ = self.tx.send(Inbound::Failed(format!("{state}").to_ascii_lowercase()));
            }
            _ => {}
        }
    }
}

pub struct Peer {
    pc: Arc<dyn PeerConnection>,
    control: Arc<dyn DataChannel>,
    data: Arc<dyn DataChannel>,
    rx: Mutex<UnboundedReceiver<Inbound>>,
}

/// Forward a channel's events onto the shared stream.
fn pump(dc: Arc<dyn DataChannel>, label: &'static str, tx: UnboundedSender<Inbound>) {
    tokio::spawn(async move {
        while let Some(ev) = dc.poll().await {
            let out = match ev {
                DataChannelEvent::OnOpen => Some(if label == "control" { Inbound::ControlOpen } else { Inbound::DataOpen }),
                DataChannelEvent::OnMessage(m) => Some(if label == "control" { Inbound::Control(m.data.to_vec()) } else { Inbound::Data(m.data.to_vec()) }),
                DataChannelEvent::OnClose => Some(Inbound::Closed(label.to_string())),
                _ => None,
            };
            if let Some(out) = out {
                let closed = matches!(out, Inbound::Closed(_));
                if tx.send(out).is_err() || closed {
                    break;
                }
            }
        }
    });
}

fn ice_servers(servers: &[IceServer]) -> Vec<RTCIceServer> {
    servers
        .iter()
        .filter(|s| !s.uris.is_empty())
        .map(|s| RTCIceServer {
            urls: s.uris.clone(),
            username: s.credentials.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
            credential: s.credentials.as_ref().map(|c| c.password.clone()).unwrap_or_default(),
        })
        .collect()
}

/// The `a=mid:` of every `m=` section of `sdp`, in order, with the section's media type.
pub fn media_sections(sdp: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in sdp.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("m=") {
            let kind = rest.split(' ').next().unwrap_or("").to_string();
            out.push((kind, String::new()));
        } else if let Some(mid) = line.strip_prefix("a=mid:")
            && let Some(last) = out.last_mut()
            && last.1.is_empty()
        {
            last.1 = mid.trim().to_string();
        }
    }
    out
}

/// Match the model's tracks to the offer's media sections by kind, in order.
pub fn track_mapping(sdp: &str, tracks: &[TrackCapability]) -> Vec<TrackMappingEntry> {
    let sections = media_sections(sdp);
    let mut used = vec![false; sections.len()];
    let mut out = Vec::new();
    for t in tracks {
        let found = sections.iter().enumerate().find(|(i, (kind, mid))| !used[*i] && *kind == t.kind && !mid.is_empty());
        if let Some((i, (_, mid))) = found {
            used[i] = true;
            out.push(TrackMappingEntry { name: t.name.clone(), kind: t.kind.clone(), direction: t.direction.clone(), mid: mid.clone() });
        }
    }
    out
}

fn rtc_err(what: &str, e: impl std::fmt::Display) -> GenerateError {
    GenerateError::Network(format!("webrtc: {what}: {e}"))
}

impl Peer {
    /// Build the connection, its channels and transceivers, and create the local offer.
    /// Returns the peer, the offer SDP and the track mapping to send with it.
    pub async fn connect(servers: &[IceServer], tracks: &[TrackCapability]) -> Result<(Peer, String, Vec<TrackMappingEntry>), GenerateError> {
        let (tx, rx) = unbounded_channel();
        let handler = Arc::new(Handler { tx: tx.clone(), first_mid: std::sync::Mutex::new(None) });
        let mut media = MediaEngine::default();
        media.register_default_codecs().map_err(|e| rtc_err("codecs", e))?;
        let registry = register_default_interceptors(Registry::new(), &mut media).map_err(|e| rtc_err("interceptors", e))?;
        let config = RTCConfigurationBuilder::new().with_ice_servers(ice_servers(servers)).build();
        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .with_handler(handler.clone())
            .with_runtime(Arc::new(TokioRuntime))
            .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
            .build()
            .await
            .map_err(|e| rtc_err("connection", e))?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);
        let control = pc.create_data_channel("control", None).await.map_err(|e| rtc_err("control channel", e))?;
        let data = pc.create_data_channel("data", None).await.map_err(|e| rtc_err("data channel", e))?;
        pump(control.clone(), "control", tx.clone());
        pump(data.clone(), "data", tx);
        for t in tracks {
            if t.direction != "recvonly" {
                continue;
            }
            let kind = match t.kind.as_str() {
                "video" => rtc::rtp_transceiver::rtp_sender::RtpCodecKind::Video,
                "audio" => rtc::rtp_transceiver::rtp_sender::RtpCodecKind::Audio,
                _ => continue,
            };
            let init = RTCRtpTransceiverInit { direction: RTCRtpTransceiverDirection::Recvonly, ..Default::default() };
            pc.add_transceiver_from_kind(kind, Some(init)).await.map_err(|e| rtc_err(&format!("transceiver for {}", t.name), e))?;
        }
        let offer = pc.create_offer(None).await.map_err(|e| rtc_err("offer", e))?;
        let sdp = offer.sdp.clone();
        pc.set_local_description(offer).await.map_err(|e| rtc_err("local description", e))?;
        let local = pc.local_description().await.map(|d| d.sdp).unwrap_or(sdp);
        *handler.first_mid.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            media_sections(&local).first().map(|(_, mid)| mid.clone()).filter(|m| !m.is_empty());
        let mapping = track_mapping(&local, tracks);
        Ok((Peer { pc, control, data, rx: Mutex::new(rx) }, local, mapping))
    }

    pub async fn set_answer(&self, sdp: &str) -> Result<(), GenerateError> {
        let answer = RTCSessionDescription::answer(sdp.to_string()).map_err(|e| GenerateError::Protocol(format!("bad SDP answer: {e}")))?;
        self.pc.set_remote_description(answer).await.map_err(|e| rtc_err("remote description", e))
    }

    pub async fn recv(&self) -> Option<Inbound> {
        self.rx.lock().await.recv().await
    }

    pub async fn send_control(&self, bytes: Vec<u8>) -> Result<(), GenerateError> {
        self.control.send(BytesMut::from(&bytes[..])).await.map_err(|e| rtc_err("control send", e))
    }

    pub async fn send_data(&self, bytes: Vec<u8>) -> Result<(), GenerateError> {
        self.data.send(BytesMut::from(&bytes[..])).await.map_err(|e| rtc_err("data send", e))
    }

    pub fn control_channel(&self) -> Arc<dyn DataChannel> {
        self.control.clone()
    }

    pub async fn close(&self) -> Result<(), GenerateError> {
        self.pc.close().await.map_err(|e| rtc_err("close", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1 2\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nc=IN IP4 0.0.0.0\r\na=mid:1\r\na=recvonly\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=mid:2\r\na=recvonly\r\n";

    #[test]
    fn tracks_map_onto_the_offers_media_sections_by_kind() {
        let tracks = vec![
            TrackCapability { name: "main_video".into(), kind: "video".into(), direction: "recvonly".into() },
            TrackCapability { name: "main_audio".into(), kind: "audio".into(), direction: "recvonly".into() },
            TrackCapability { name: "cam".into(), kind: "video".into(), direction: "sendonly".into() },
        ];
        let m = track_mapping(SDP, &tracks);
        assert_eq!(m.len(), 2, "a track without a section is left out");
        assert_eq!((m[0].name.as_str(), m[0].mid.as_str()), ("main_video", "1"));
        assert_eq!((m[1].name.as_str(), m[1].mid.as_str()), ("main_audio", "2"));
        assert_eq!(media_sections(SDP)[0], ("application".to_string(), "0".to_string()));
        assert!(media_sections("").is_empty());
    }

    #[test]
    fn ice_servers_carry_credentials_only_when_given() {
        let s = vec![
            IceServer { uris: vec!["stun:a:3478".into()], credentials: None },
            IceServer {
                uris: vec!["turn:b:3478".into()],
                credentials: Some(super::super::http::IceCredentials { username: "u".into(), password: "p".into() }),
            },
            IceServer { uris: vec![], credentials: None },
        ];
        let out = ice_servers(&s);
        assert_eq!(out.len(), 2);
        assert!(out[0].username.is_empty() && out[1].username == "u" && out[1].credential == "p");
    }
}
