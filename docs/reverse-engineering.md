# Reverse Engineering

RE1 adds a native dockable panel shell. It opens by default beside **Metadata** in Editing, Assembly and All Panels. You can also open it with **Window → Reverse Engineering**.

Existing saved layouts gain the new tab once. After that migration, closing the panel and saving your layout remains respected.
It uses the existing docking, close, float and workspace-layout persistence behavior.
Opening or closing it does not edit the media project.

## Capture a frame

Select a video clip in the timeline, move the playhead inside that clip, then click **Extract current frame**. The panel shows the clip name, source and timeline timecodes, and a frozen 640-pixel-wide preview. Moving the playhead does not change the captured image; extract again to replace it. The preview uses the existing background decoder and applies the clip’s source in-point, speed and frame hold. It captures the source image before timeline effects; it does not write an image file or modify the project.

No selection, an audio-only selection, or a playhead outside the selected clip disables extraction and explains what to do. Missing media is reported in the panel. Camera, lighting and scene analysis come later.

The capture is inspectable at `ui.panels.reverseEngineering.capture`. Invoke the same action with `ui.menu.invoke {"id":"reverseEngineering.extractFrame"}`. Automation ids: `reverseEngineering.selection`, `reverseEngineering.extractFrame`, `reverseEngineering.timecode`, `reverseEngineering.frame`.

Automation uses the existing UI commands:

```json
{"method":"ui.panel.show","params":{"panel":"ReverseEngineering"}}
{"method":"ui.panel.close","params":{"panel":"ReverseEngineering"}}
```

The Window menu command is `window.panel.ReverseEngineering`. The introductory label
is exposed as `reverseEngineering.intro` through `ui.elements`.

Focused verification: `cargo test --release -p filmcraft-ui-egui --test panels_ui reverse_engineering`.

## Capture a range

Switch to **Range**. Drag across the selected video clip in the main timeline to select a range; drag either highlighted edge to resize it. The playhead follows the drag and the Program monitor shows the frame at that position. Scrub the normal timeline ruler to inspect the range without changing its bounds. **Replay range** plays from its start through its final frame and stops; **Stop** stops playback. Range mode temporarily uses drag gestures on the selected clip for range selection rather than moving/trimming that clip. Switch to Frame mode for ordinary editing. Start and end are editable exact sequence timecodes (`HH:MM:SS:FF`), synchronized with the visual selection. Press Enter or leave the field to apply a timestamp and preview that boundary in the Program monitor. **Use playhead** sets either boundary to the current position. The default range covers the entire clip, and the default interval samples every 30 timeline frames. Both boundaries are included even when the interval does not divide the range evenly. A one-frame range is valid.

Click **Extract range** to create a frozen, horizontally scrolling filmstrip. Each thumbnail has a timeline timestamp; click it to inspect that source frame in the larger preview. Sampling uses timeline frame boundaries and maps them through the clip's source in-point, playback speed/reverse and frame hold. This is source-frame inspection, before timeline effects; no files are exported and no AI analysis is performed.

Ranges are limited to 64 previews. Invalid/reversed bounds, a zero interval or an oversized range return an error and keep the last successful capture. Increase the interval or shorten the range for longer clips. Changing the selected clip resets the range controls; captures stay frozen until replaced.

UI state: `ui.panels.reverseEngineering` includes `rangeMode`, `rangeClip`, `startFrame`, `endFrame`, `sampleInterval` and `samples`. UI commands: `reverseEngineering.setRange {"startTimecode":"00:00:01:00","endTimecode":"00:00:03:00"}` (or integer `startFrame`/`endFrame` offsets), `reverseEngineering.playRange`, `reverseEngineering.extractRange`, `reverseEngineering.markStart`, `reverseEngineering.markEnd`, `reverseEngineering.selectSample {"index":0}`. Automation ids: `reverseEngineering.range.timeline`, `reverseEngineering.range.handleStart`, `reverseEngineering.range.handleEnd`, `reverseEngineering.playRange`, `reverseEngineering.stop`, `reverseEngineering.mode.frame`, `reverseEngineering.mode.range`, `reverseEngineering.range.start`, `reverseEngineering.range.end`, `reverseEngineering.range.interval`, `reverseEngineering.sample.N`, `reverseEngineering.sampleTime.N`.

This work remains R3: frame/range capture, including visual selection and exact timestamp entry.

## Local motion evidence — R4

In **Range**, select 2–300 timeline frames and click **Analyze motion · SEA-RAFT**. The background Rust worker decodes every timeline frame and passes each consecutive pair to the isolated local SEA-RAFT M runtime. Filmstrip preview spacing does not affect analysis. The custom patch tracker has been removed; there is no fallback. The app does not download models or make network calls during analysis.

The model loads once per job. Cancel interrupts inference through the Rust process guard and terminates the external runtime; an in-progress source decoder must still return before cancellation can take effect. Per-model responses have a 90-second limit; the complete job has a ten-minute limit. Failed/cancelled jobs retain the previous complete report.

The fixed inspection area keeps the source preview and Previous/Next controls together while settings scroll. Overlay controls include visibility, movement cutoff in analysis pixels, arrow scale, and spatial grid stride. Defaults use zero cutoff, true displacement scale (×1), and every second model grid row/column. There are no moving/stationary categories, fixed amplification, or stationary rings. Arrows visualize SEA-RAFT samples; display controls never alter the underlying predictions.

Editing any control creates an override for the selected incoming frame pair (movement from the previous frame to this frame). **Apply to all** sets the shared settings for every analyzed frame and clears all frame overrides. **Use all-frame settings** removes only the selected frame's override. Navigation preserves each frame's settings. New analysis starts with default display settings. Hover a displayed sample for its exact model displacement; click it to inspect displacement, normalized origin, and uncertainty log-scale. A small selection marker identifies only the selected measurement. Uncertainty is not labeled as calibrated confidence.

**Export JSON** exports schema version 4 with analyzer `filmcraft-vision/sea-raft-M`, pinned checkpoint/source revisions and checksum, device, model iterations, analysis dimensions, sampling spacing, all captured frame times, per-pair motion vectors and model uncertainty log-scale, dense pixel count, mean displacement, and appearance change. The JSON does not contain every dense pixel vector. Its separate `display` object stores `allFrames` settings and `frameOverrides`, keyed by incoming pair index. All original vectors remain in the report even if filtered out of the display. Uncertainty log-scale is model evidence, not a calibrated confidence probability. The old patch-error/ambiguity fields are omitted from new results. Old saved reports remain readable and explicitly say legacy; their export is blocked until reanalysis with SEA-RAFT.

Commands remain `reverseEngineering.analyzeMotion`, `reverseEngineering.cancelMotion`, `reverseEngineering.motionPair {"index":0}`, and `reverseEngineering.exportMotion {"path":"/absolute/path/motion.json"}`. Display commands: `reverseEngineering.setMotionDisplay {"scope":"frame","index":0,"display":{"enabled":true,"cutoffPixels":0.1,"arrowScale":4,"sampleStride":2}}`. Scope `all` applies those settings to all analyzed frames and removes overrides; scope `inherit` requires no display object and resets only the given frame. `reverseEngineering.inspectMotionVector {"pairIndex":0,"vectorIndex":0}` selects and returns actual model evidence. All controls have automation ids. The same report is exposed at `ui.panels.reverseEngineering.motion`.

Analysis uses RGB images up to 640 pixels on the longest side. Both dimensions must be at least 64 pixels; extremely thin media needs another model/preprocessing path. Flow estimates can fail on cuts, occlusion, flat regions, or appearance changes. They are not camera pose, subject identity, or semantic descriptions.

## Local model runtime

The product connector is Rust (`crates/vision/src/sea_raft.rs`); UI orchestration stays in `panels/reverse_motion.rs`. The official PyTorch model runs in a separate local process, not a linked dependency. It is currently installed on this Mac at:

```text
~/Library/Application Support/FilmCraft/sea-raft/
  worker.py
  model.safetensors
  SEA-RAFT/             # pinned official source checkout
  venv-arm/             # ARM Python environment with the model dependencies
```

Set `FILMCRAFT_SEA_RAFT_DIR` to use another runtime location. Other desktop platforms default to `~/.cache/filmcraft/sea-raft`. Missing runtimes fail clearly; they never silently use another algorithm. No cross-platform runtime installation has been tested yet. Model/runtime binaries are local and are not committed. This is an optional local runtime integration, not a fully Rust model implementation.

Focused checks: `cargo test --locked -p filmcraft-vision --lib`. The explicit runtime cancellation test uses `-- --ignored`. The UI end-to-end motion test also requires an installed runtime and is ignored in default CI. It has not been rerun after this change, to avoid an editor compilation; the Rust connector has been tested on the full Ocean Sunset range.

Repeat that connector test without building the UI:

```sh
cargo run --locked -p filmcraft-vision --example ocean_motion -- /tmp/ocean-sea-raft 34 141
```


## Analysis-first viewer

Range mode now opens as the default for new panel state. Choose the range on the timeline, then Analyze. The frame rate and inclusive frame count are shown before the job starts. The old Extract range and preview-spacing controls are absent from this workflow; their legacy automation commands remain compatible. No thumbnails or frozen range results appear before analysis.

Completed analysis has one filmstrip and one viewer. Every analyzed capture is browsable, including the first reference frame. Use a thumbnail, First/Previous/Next/Last, or the 1-based frame-number jump to select it. Source/timeline timestamps refer to the selected capture during the same UI pass. The first frame has no incoming flow; later frames show motion from the immediately preceding frame. Thumbnail decoding is requested only for visible filmstrip cells.

Changing the clip/range/source timing hides prior results and display controls; a stale-result message asks for analysis again. Range inputs and timeline range dragging are locked while a job is running, and result controls are hidden. Overlay edits update immediately. Reanalyze entire range recomputes all predictions while preserving display preferences for the same range. Reanalyze selected frame reruns the model on the preceding and selected captures, then atomically replaces only that pair. Other predictions and display preferences remain intact; cancellation/failure leaves the previous complete result untouched. Reanalysis uses the existing model settings: display cutoff, scale, and density are not inference parameters.

New automation commands: `reverseEngineering.motionFrame {"frameIndex":0}` (zero-based captured frame), and `reverseEngineering.reanalyzeFrame` (selected frame's incoming comparison). `motionPair` remains compatible. Focused Rust tests verify atomic selected-pair replacement and rejection of mismatched/invalid updates. UI tests are updated but not rebuilt or run for this change.

Preview loading uses a separate timeout for each exact source timestamp and size. Newly visible result thumbnails therefore load even after a long analysis or delayed scroll. Missing previews offer a local Retry action that requests the image again without rerunning SEA-RAFT. Compact loading/retry placeholders keep the filmstrip readable.

Schema version 5 adds an `audit` object. Successful completion increments the report revision and relevant 1-based destination frame revisions. `completedRuns` records scope (`all` or `frame`), frame number, and UTC Unix completion milliseconds; the latest 64 records are retained. Reanalyzing UI frame 116 records `frameNumber: 116` even if its predictions are identical. Cancelled, failed, or timestamp-mismatched reruns never update the audit. Older reports load with an empty audit; their previous execution history is not inferred.

## R5 tool workflow

The range selector now leads to a tool menu instead of opening optical-flow controls directly. See [R5 tool menu, saving, exports, and global image motion](reverse-engineering-r5.md). SEA-RAFT and global motion are available; the five later layers are disabled.

R6 adds [local residual motion](reverse-engineering-r6.md) with independent display settings and saved/exported results.

[Motion regions](reverse-engineering-motion-regions.md) groups neighboring residual samples with adjustable thresholds and per-frame settings.

[Point tracking with BootsTAPIR](reverse-engineering-point-tracking.md) adds optional manual/grid query selection and trajectories from an independent model.


## Changing the analysis range

Dragging previews a new range without committing it. On release, or when entering new timestamps
or using a playhead boundary, a confirmation asks whether to start again. Cancel/Escape retains
the old range and all progress. Accept removes previous-range saved snapshots from the analysis
store, then clears working results, frame captures and tracking queries for every tool, commits
the new range and returns to the tool menu. Other saved ranges and exported JSON files are retained.
A failed store write keeps the old range and results. Tool borders now reflect the current range.
Confirmation does not start inference; choose the next tool after setting the range.

## Model execution choices

SEA-RAFT and BootsTAPIR have separate **Auto / CPU / GPU** choices. The selected preference applies
to the next model run, not existing results. It is sent through that subprocess’s environment;
the actual device and requested preference are recorded in runtime metadata. Forced GPU reports
an error when its backend is unavailable rather than silently using CPU. Auto uses an available
supported GPU or CPU. SEA-RAFT supports MPS on this Mac. BootsTAPIR Auto uses CPU on Apple devices because earlier MPS predictions failed validation.
Explicit GPU selection allows users to try CUDA or Apple MPS and inspect runtime failures. Rust-only global fitting and grouping do not
need model execution controls.

## Two-column inspection layout

All tools share a full-height left column for analysis controls and save/delete/export actions.
Navigation, timestamps, measurements, the selected frame and its horizontal thumbnail strip
are in the right column. Text
and control groups wrap within their columns; the selected frame scales to leave room for the
strip. The left column scrolls independently only when the panel is too short to contain its
controls. The image is not placed below that scroll area. Thumbnails are virtualized, including
for full-range point tracking, so offscreen frames do not create decoding requests or widgets.

## Object masks

SAM 2.1 is an independent optional model tool. See [setup, usage and mask export](reverse-engineering-sam2.md).

## Components menu and 2D camera

The tool menu lists the scene components to extract (2D camera, 3D camera, shots, look, sound,
script, figure and object movements, text and titles, effects and transitions), enabled as they
are built, with the raw analysis layers under **Evidence**.
The 2D camera is built step by step: [2D camera](reverse-engineering-camera2d.md). The look of
each shot as data, words and a reusable Lumetri grade: [Look](reverse-engineering-look.md). The
mix per shot as loudness, speech / music / effects / ambience, tempo, beats on cuts and dialogue
lines: [Sound](reverse-engineering-sound.md).

## Shots & look, camera moves, AI brief

Pure-Rust shot detection with per-shot look (palette, key, pixel grid, cadence), camera-language
labels from the global fit, and a per-shot Markdown + JSON breakdown for people and language
models: [shots, look and the brief](reverse-engineering-shots-look.md). What a complete breakdown
still needs, ranked: [roadmap](reverse-engineering-roadmap.md).
