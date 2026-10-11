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
