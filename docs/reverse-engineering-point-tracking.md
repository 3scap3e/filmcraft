# Point tracking: BootsTAPIR

This is a separate optional analysis tool, independent of SEA-RAFT/global subtraction. FilmCraft's
commands, frame rendering, query selection, subprocess management, validation, UI and exports are
Rust. Actual neural inference runs the official PyTorch model in an external local runtime, as
with SEA-RAFT. No model weights or Python inference code are bundled in the repository.

## Use in the editor

1. Select video content and any valid timeline range of **at least two frames**.
2. Open **Point tracking**. Its fixed viewer works before inference; use previous/next or the
   frame number to select a visible reference frame (avoid the black opening frames).
3. Choose **Manual points** and click several edges on the image, or choose **Automatic grid**.
   The grid seeds 2×2 through 8×8 points on the displayed frame. **Seed grid on displayed frame**
   reseeds after navigation. A grid is automatic sampling, not automatic object detection.
4. Click **Track points**. Frame preparation and inference status remain visible; cancellation
   terminates the child process. The timeline range highlight remains visible and range editing is
   disabled during the job.
5. Navigate frames to inspect predicted point positions and trailing paths (up to 30 frames).
   Paths break across predictions the model marks invisible. Hover a marker for displacement from
   its reference and the model visibility score. Save/delete/export use the same
   analysis store and combined export as other tools.

Manual points may be initialized on different reference frames. Switching selection mode clears
previous seeds. Adding/clearing points or changing the range invalidates the old result; inference
only runs on **Track points**. The cap is 64 query points. Start with a short range and 2–4 edge
points for the first test. Model visibility scores are not calibrated confidence probabilities.

Coordinates in exports are normalized `(x,y)`; each query records its zero-based frame index
within the selected range. The external adapter explicitly converts to the model's `(t,y,x)`
query format. Frames are rendered at up to 320 pixels and resized to the official demo's 256×256
inference size. Every predicted frame retains its exact rendered-timeline capture timestamp.
Exports record query selection, source size, project revision, runtime/model provenance, predicted
positions and model visibility scores/decisions. These are predictions, not object identities or
physical camera measurements. Cuts and ambiguous/occluded features can break tracking.

## Installed runtime on this Mac

Directory: `~/Library/Application Support/FilmCraft/bootstapir`.
Contains `worker.py`, `python` launcher, `metadata.json`, pinned official source under `source/`,
Apache-2.0 license, and `bootstapir_checkpoint_v2.pt` (218,886,140 bytes). The launcher reuses the
existing SEA-RAFT `venv-arm` PyTorch installation; added packages are dm-tree 0.1.10 and einshape
1.0. The model is downloaded only during setup, not automatically during app launch or inference.

Configuration overrides:

- `FILMCRAFT_BOOTSTAPIR_DIR`: external runtime directory.
- `FILMCRAFT_BOOTSTAPIR_PYTHON`: actual Python executable (use the virtual environment's original
  executable path, not a bare symlink that bypasses its environment).
- `FILMCRAFT_BOOTSTAPIR_DEVICE`: `auto`, `cpu` or `gpu` (the worker also accepts `cuda`). The
  app sets this per model subprocess from its execution control. This Mac’s Auto uses CPU. CUDA execution has not
  been benchmarked here.

The Apple MPS attempt failed the model's reference-point sanity check even with its unsupported
3D grid sampler routed to CPU. Auto therefore uses CPU on Apple devices. Explicit GPU selection attempts MPS and reports runtime errors without silently retrying on CPU. CPU execution produced
visible downward tracks on the sun in the short fixture test; exact accuracy still requires
inspection. No alternative tracking algorithm is silently substituted.

Source: https://github.com/google-deepmind/tapnet

Pinned source revision: `730cda1c730877cfedbe01bf87fb1cadb78a565d`.

Checkpoint: https://storage.googleapis.com/dm-tapnet/bootstap/bootstapir_checkpoint_v2.pt

SHA-256: `8493c7a69e02c85b9382fbb3c7b8b539b36bc08ede744b9e99feb739a0129f4b`.

Official code and checkpoints are Apache-2.0 licensed. Attribution and license remain with the
external installation. Model checksum is verified by the worker; source/checkpoint provenance
and output shape/coordinates/visibility are validated by the Rust connector. Input is bounded
RGBA binary data after a JSON request; output is a bounded JSON response. Timeouts and
cancellation kill and reap the process, preserving previous completed results on failures.

## Focused validation without compiling the editor

`cargo test --locked --offline -p filmcraft-vision --lib`

`cargo run --locked --offline -p filmcraft-vision --example bootstapir_ocean -- /tmp/bootstapir-ocean.json`

The example seeds four sun-edge points on 25 consecutive Ocean Sunset frames and reports
predicted displacement against the demo generator's known vertical displacement.

Verified CPU test: on 25 consecutive frames, the top/bottom edge points moved down 2.157/2.148
pixels at 640×360 versus the known 2.162-pixel displacement. The side points moved only
0.661/0.852 pixels; success is point-dependent, not a guarantee of exact tracking. The complete
Rust connector test took approximately 10 seconds. The editor UI has not been compiled or launched
by the agent. Add `grid` to the example command to test automatic 4×4 grid queries.


## Full-range chunking

The UI no longer rejects ranges longer than 120 frames. Frames are rendered on demand per bounded
window; the entire range is not buffered as RGBA images. Each inference window contains at most
120 frames and shares one boundary frame with the next. Points initialized on different frames
are grouped by reference frame and propagated forward/backward to cover the selected range.
Continuation uses the model’s visible, in-bounds endpoint prediction, preserving point identity.
Lost continuations remain explicit `null` entries rather than invented positions; the viewer breaks
paths at those gaps. Re-querying predictions at boundaries can accumulate drift and does not
provide uninterrupted re-detection across an occluded boundary.

Progress represents the entire selected range. The export includes all range timestamps and
per-window boundaries, directions and point IDs. Schema version 2 adds chunk metadata and nullable
observations (older single-window observations remain readable). Resource errors are explicit;
result storage has a 512 MB working-memory budget and metadata allocation is bounded, rather than
imposing a user-facing inference frame-count cap. Selecting fewer points reduces result storage
without shortening the selected range. Other tools retain their existing analysis limits.

Focused tests cover 301-frame coverage from a middle reference, identity carry-over, overlap
deduplication, missing continuations and cancellation. The `bootstapir_chunks` example exercises
the real model across a boundary using small 16-frame test windows.

The real-model boundary smoke test processed all 31 frames in two 16-frame windows, retaining
both point identities. Predicted downward displacement was 2.345/2.160 pixels versus 2.703
expected. This validates execution/stitching, not precision; chunk continuation can accumulate
error. The editor UI remains for manual testing.

### User point groups

Choose a group name before clicking manual points. Group chips select the group for subsequent
points; color is derived consistently from the label. “Assign unlabelled points to this group”
labels existing points without rerunning inference. Labels are user annotations, not model-detected
objects or segmentation. Saved/exported `pointLabels` maps by index to `queries` and
`inference.tracks`; older reports without labels display as Unlabelled. Saving preserves query
points and the working report. New ranges and cleared/reseeded points clear the label assignments.
