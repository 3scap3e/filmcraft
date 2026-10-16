# filmcraft-vision

Rust motion evidence types and a bounded connector to the isolated local SEA-RAFT M inference runtime. The custom patch matcher has been removed.

The connector validates frame dimensions and result values, caps response size and vector count, verifies model/checkpoint provenance, supports cancellation and timeouts, and kills/reaps its child process on exit. One model process is reused across all frame pairs in a job. No native ML library is linked into the Rust product; the optional external model uses the official PyTorch implementation.

Inference is dense, at up to 640 pixels on the longest side. JSON and overlays sample it every 16 pixels. Every timeline frame is analyzed. Motion estimates and model log-scale uncertainty do not establish physical camera geometry or calibrated confidence.

Runtime installation and UI behavior: `docs/reverse-engineering.md`. Existing patch reports remain deserializable for saved projects, but are never produced by this connector.

Run bounded-protocol/provenance tests with `cargo test --locked -p filmcraft-vision --lib`. Verify the installed runtime on the editor's own Ocean Sunset generator with `cargo run --locked -p filmcraft-vision --example ocean_motion -- /tmp/ocean-sea-raft 34 141`. This runs all 108 frames and 107 consecutive comparisons without compiling the editor.

`display` provides independently tested, serializable presentation controls, shared settings, and per-frame overrides. They filter visualization only; model predictions remain intact.

Pure-Rust analyzers that need no runtime: `shots` (frame signatures, cut / black-run / held-frame detection, shot statistics), `look` (luma, saturation, warmth, k-means palette, pixel-grid pitch, letterbox for one frame), `camera_moves` (pan / tilt / zoom / roll labels and segments from `global_motion` fits) and `brief` (the per-shot Markdown breakdown built from those results plus object presence). Each validates hostile input and is covered by `cargo test --locked -p filmcraft-vision --lib`. Behaviour: `docs/reverse-engineering-shots-look.md`.
