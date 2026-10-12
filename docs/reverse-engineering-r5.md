# R5: tool menu and global image-motion analysis

Select a video and frame/range on the tool menu. All current motion tools need a range of at least two frames. Single-image capture remains available; these motion tools are disabled for a single image.

The menu contains the SEA-RAFT input tool and six analytical layers: global image motion, local motion, motion regions, motion over time, trajectories, and discontinuities. Only SEA-RAFT and global motion are implemented. The other five are explicitly disabled; no placeholder results are generated. Menu status distinguishes not used, computed/not saved, saved current result, unsaved edits, and saved results for another range of the video.

## Use

1. Select the range in the menu or main timeline.
2. Open **SEA-RAFT optical flow**, analyze the range, then **Save result**.
3. Return to **Tool menu**, open **Global image motion**, and fit every comparison.
4. Inspect selected frame estimates; optionally display the samples supporting the mathematical fit. Change fit tolerance and rerun to experiment; changes hide old estimates until the new fit completes.
5. Save/delete the result or export this tool's JSON. Return to the menu for combined export of saved results for the selected range.

The shared frame viewer and filmstrip appear only for completed current results. Range controls live on the menu. Tool pages show the selected range and their own controls. Individual tools export their current result without requiring Save. Combined export includes only saved snapshots and excludes a derived global result if its saved flow input has been replaced by a newer revision.

Save persists results atomically. Named projects use `<project path>.analysis.json`; unsaved projects use an analysis-results directory alongside application preferences, keyed by deterministic project content. Saved snapshots are restored when returning to their project/range. Up to 16 saved tool/range snapshots and a 100 MB store are supported. Invalid/unrelated existing files are preserved. Project/media files are not edited. Export is the explicit portable JSON file. Delete updates the store before discarding the session result. Global motion depends on the matching optical-flow input; deleting that input requires recomputing/restoring flow before using the derived viewer again.

## Algorithm and output

Pure Rust, deterministic robust 2D similarity fitting of SEA-RAFT samples. The transform is `x'=a*x-b*y+tx`, `y'=b*x+a*y+ty` in analysis-image pixels. The fitter evaluates 256 two-sample hypotheses and refines the best support set with least squares. Default supporting-sample tolerance is 0.5 pixels, adjustable between 0.01 and 10 pixels.

Estimates require eight supporting samples, at least 50% support, coverage of three image quadrants, and a span of 30% of both image dimensions. Otherwise the result explicitly records insufficient evidence. Output includes center displacement, image-plane rotation, uniform scale, RMS residual, support fraction, supporting indices and actual source/target coordinates, analysis dimensions, source flow revision/completion timestamp, and exact captured source/timeline ticks. This is not physical camera pose, lens calibration, or camera/subject separation. A dominant moving object or bad flow can bias the fit.

The optional white support markers identify actual supporting samples at their target-frame coordinates. The default view has no overlay. No new optical-flow model or model download is used.

## Concrete verification

Controlled tests recover translation, rotation and uniform scale to within 0.001 in transform coefficients, reject 25% local-motion outliers, and reject sparse/concentrated/invalid evidence without inventing movement.

On the user's Ocean Sunset export: all 118 comparisons yielded estimates. Maximum center displacement was 0.11785 analysis pixels, maximum absolute image rotation 0.00882 degrees, and maximum scale deviation 0.0002874 (0.02874%). The estimates remain near identity on this fixed-camera example, but these values are not a calibrated guarantee of camera accuracy. The procedural scene has no ground-truth per-pixel flow.

Repeat without building the editor:

```sh
cargo test --locked -p filmcraft-vision --lib
cargo run --locked -p filmcraft-vision --example global_motion -- /absolute/sea-raft-export.json /tmp/global-motion.json
```

The Rust geometry tests and real-export benchmark passed. UI/save-store tests have been added/updated, but the desktop editor has not been rebuilt or launched after R5. Check the menu, saving, restoring, and exports manually before committing.

## Commands

All UI actions are registered commands: `chooseTool`, `runGlobal`, `cancelGlobal`, `setGlobalOptions`, `saveToolResult`, `deleteToolResult`, `exportTool`, and `exportCombined`, prefixed by `reverseEngineering.`. `chooseTool` accepts `menu`, `opticalFlow`, or `globalMotion`; unavailable tool ids return an explicit error. `setGlobalOptions` accepts `tolerancePixels` and `showSupport`. Export/save commands accept an explicit `path` for automation. Controls and tool entries register automation ids. Results and tool selection remain in serializable reverse-engineering panel state.


### Timeline ranges and direct tool opening

Tool buttons keep their labels on one line and wrap as complete buttons. Select video content
in the timeline before opening a tool. Clicking SEA-RAFT flow opens it and immediately starts a fresh analysis of every frame
in the selected timeline range. Global image motion reuses current flow when available. Global image motion prepares missing SEA-RAFT input inside its own
page and continues fitting after preparation succeeds; it does not require opening another tool.

Ranges use absolute timeline frame numbers and may cross clips, layers and empty gaps. Drag
across the video timeline area or enter exact timeline timestamps. Capture items identify the
sequence; media and timeline timestamps both identify the rendered sequence frame. Analysis and
previews use the existing sequence renderer, including timeline effects and compositing. The
2–300-frame analysis limit remains. Results are invalidated by project edits during the session.
Flow at a cut is an image correspondence estimate, not evidence of continuous camera motion.
