# Generate: AI video clips onto the timeline

The Generate panel (Window ▸ Generate) describes a shot in words, picks its length and an optional
still, chooses where it goes, and puts an AI-generated clip on the timeline. The first provider is
Reactor's FastH3 model (MiniMax H3 accelerated by FastVideo, served live by [reactor.inc](https://reactor.inc)).

## Flow

1. **Key.** Settings ▸ Generate ▸ API key (or the `REACTOR_API_KEY` environment variable). FilmCraft
   sends the key only to Reactor's token endpoint and works with a short-lived JWT scoped to the
   model from then on.
2. **Session.** `POST /sessions` for `reactor/fast-h3`; the session is polled until a GPU is ready
   (up to about two and a half minutes).
3. **WebRTC.** One peer connection with two data channels, `control` and `data`, plus `recvonly`
   transceivers for the model's `main_video` / `main_audio` tracks. The media is negotiated but never
   decoded: FilmCraft only speaks the data channels (`reactor_wire.v1` protobuf) and fetches the
   clip afterwards as a file.
4. **Commands.** `set_autoplay {enabled:true}`, `set_canvas {aspect:"4:3"}` (1024×768), the still is
   uploaded (`POST /sessions/{id}/uploads` → presigned `PUT`) and referenced as `starting_frame` /
   `ending_frame`, then `enqueue {prompt, seconds, seed?}`. `clip_started` → `clip_finished` are
   followed for that clip id (`clip_failed` ends the run). A `Ping` keeps the session alive every 10 s.
5. **Capture.** `requestClip(seconds)` on the control channel returns an HLS playlist (fragmented
   MP4: an init segment plus `.m4s` parts). The peer connection is closed and the session is deleted
   first, since it is billed per second, then the parts are downloaded and concatenated into one MP4.
6. **Placement.** The engine imports the MP4 and places it at the playhead (or the position given)
   on the chosen video track, with its audio on the patched audio track, as one undo step, and records
   the prompt, seed and provider on the item (`Project::generations`, schema v16).

## Commands

| command | does |
|---|---|
| `generate.providers` | the generators this build knows: limits, availability, whether a key is set |
| `generate.video {prompt, seconds?, image?, imageRole?, seed?, track?, atTime|atFrame|atSeconds?, insert?, dir?, wait?}` | start a generation job; `wait: true` runs it synchronously (tests) |
| `generate.status` | running generations: phase, detail, billed seconds, estimate |
| `generate.cancel {job?}` | stop a generation; the provider session is ended |
| `generate.inspect {clip?|item?}` | the generation record of a clip |

Limits (FastH3): 5.167–14.375 s (snapped by the model), prompt ≤ 800 characters, 1024×768 at
24 fps, image roles first / last / both. The session is billed for roughly twice the clip's length
(build + playback) at about $0.0125 per second; the panel shows the estimate before you generate
and the billed seconds while it runs. Every exit path, including cancel and failures, ends the
session.

## Build

`crates/generate` compiles everywhere without network code; the Reactor client is behind the
feature `reactor` (on by default in the desktop app, off in the web build and the lower crates).
It brings webrtc-rs (`webrtc` 0.21 over tokio), `prost`, `ureq` with rustls + RustCrypto and the
OS certificate verifier. webrtc-rs's DTLS uses `ring`, which has C and assembly parts: that is the
one non-pure-Rust dependency, and it is isolated to this feature.

```bash
cargo run -p filmcraft                      # Generate panel available (feature on by default)
cargo run -p filmcraft --no-default-features  # without the Reactor client
```

Tests never call Reactor: the engine tests install a fake `VideoGenerator`, and the protocol
pieces (protobuf, HLS parsing and assembly, SDP track mapping, HTTP status mapping) have unit tests
on synthetic data.
