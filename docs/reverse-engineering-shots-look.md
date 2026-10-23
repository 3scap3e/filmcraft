# Shots & look, camera moves and the AI brief

Three pure-Rust layers of the Reverse Engineering panel. None of them downloads or runs a model;
they measure rendered timeline frames or reuse the fitted global image motion. All three register
commands, automation ids and save/export like the other tools, and none edits the project.

## Shots & look

Select any timeline range (two frames up to 36 000 frames) and open **Shots & look**. The tool
renders every frame of the range at 160 pixels on the longest side, keeps a small signature per
frame (16 × 9 area-averaged luma, a 16-bin RGB histogram, mean luma, contrast, saturation and
warm bias), then detects shots:

- **Cut**: the change score (the larger of histogram distance and downscaled-luma difference)
  is at least the cut threshold *and* at least the adaptive ratio times the median score of the
  surrounding ±8 comparisons. The default is 0.3 and 3×.
- **ToBlack / FromBlack**: a run of black frames (mean luma and contrast at or below the black
  luma, default 0.04) starts and ends a shot. Black shots are reported as such and get no look.
- **Held frames**: a frame whose mean cell difference, maximum cell difference and histogram
  distance against the previous frame are within the held-frame tolerance counts as a duplicate.
  Each shot reports unique frames, held frames and the hold fraction; the panel converts them to
  "about N fps of new drawings", which is how animation on 2s or 3s shows up.
- **Dissolve**: a gradual transition. Over a window of `dissolveMinFrames`–`dissolveMaxFrames`
  frames (default 3–72) the whole-frame statistics travel from one place to another: the sum of
  the changes in mean luma, warm bias and saturation between the frame before the window and its
  last frame is at least `dissolveShift` (default 0.25), and the frame-to-frame statistics do not
  wander more than twice that distance on the way (a handheld or panning shot wanders; a blend
  travels). Windows that touch a cut, a black frame or the rising/falling frames of a fade from or
  to black are not dissolves. Overlapping windows keep the one with the largest shift, trimmed to
  the frames where the statistics actually move; the boundary sits at the middle of the dissolve
  and the shot records `transitionFrames`, its length. Cross dissolves, fades through a colour and
  wipes all read this way; a hard cut followed by a slow pan does not.
- Shots shorter than the minimum shot length merge into the previous shot.

After detection the middle frame of every visible shot is rendered again at 320 pixels and
measured: mean luma, contrast, near-black and near-white fractions, saturation, warm bias, a
low/mid/high-key label, the number of distinct 4-bit colours, a deterministic k-means palette of
1–16 colours with shares, an estimated **pixel grid pitch** (runs of near-identical pixels whose
lengths are multiples of one size in both directions, 2–32 analysis pixels) and black-bar
fractions on all four sides. The pitch is reported in analysis pixels together with the analysed
size; the brief converts it to source pixels.

The right column shows a proportional shot bar (click a block), the selected shot's keyframe and a
strip of keyframes. The left column shows thresholds, the summary, the selected shot's cadence,
look numbers and palette swatches. Changing any threshold or the palette size re-detects on the
stored signatures without re-reading the frames; only keyframes without a cached look are
rendered again.

Commands: `reverseEngineering.runShots`, `reverseEngineering.cancelShots`,
`reverseEngineering.setShotOptions {"cutThreshold":0.3,"adaptiveRatio":3,"minShotFrames":3,"blackLuma":0.04,"duplicateTolerance":0.004,"dissolveShift":0.25,"dissolveMinFrames":3,"dissolveMaxFrames":72,"paletteSize":8}`
and `reverseEngineering.shotSelect {"index":0}`. The result lives at
`ui.panels.reverseEngineering.shots`; schema version 1, analyzer `filmcraft-vision/shots-and-look-v1`,
with every frame's change record, every shot and one optional look per shot. Automation ids:
`reverseEngineering.runShots`, `reverseEngineering.shots.{cutThreshold,adaptiveRatio,minShotFrames,blackLuma,duplicateTolerance,dissolveShift,dissolveMinFrames,dissolveMaxFrames,paletteSize,summary,bar,jump,First,Previous,Next,Last}`,
`reverseEngineering.shots.shot.N`, `reverseEngineering.shots.thumbnail.N`, `reverseEngineering.shots.palette.N`.

Limitations: dissolves, flashes, strobing and whip pans can be missed or over-split; a black
shot says nothing about content; look statistics describe one keyframe at reduced resolution;
pitch and letterbox are heuristics. Nothing here identifies objects, text, audio or camera pose.

## Camera moves

Open **Camera moves** on a range of 2–300 frames. Missing SEA-RAFT flow and the global similarity
fit are prepared inside the tool. Every comparison is labelled from the fitted centre
displacement, scale and rotation under a static-scene assumption:

| Evidence | Label |
|---|---|
| nothing above the thresholds | static |
| content moves right / left | pan left / pan right |
| content moves down / up | tilt up / tilt down |
| scale above / below 1 | zoom in / zoom out |
| content rotates clockwise / counterclockwise | roll counterclockwise / roll clockwise |
| two or more kinds at once | mixed movement |
| insufficient global evidence | unavailable |

Defaults: movement ≥ 0.25 analysis pixels, zoom ≥ 0.002 scale change, roll ≥ 0.1°. Consecutive
comparisons with the same label become segments with mean displacement, scale and rotation. The
right column shows a colour bar per comparison above the usual frame viewer; the left column lists
segments (click to inspect the first frame) and the share of every label. Changing a threshold
reclassifies the same fitted numbers at once.

Commands: `reverseEngineering.runCamera`, `reverseEngineering.setCameraOptions {"movePixels":0.25,"zoomFraction":0.002,"rollDegrees":0.1}`.
Result at `ui.panels.reverseEngineering.cameraMoves`; analyzer `filmcraft-vision/camera-moves-v1`.
The saved result records the flow revision, flow and global completion stamps and the fit
tolerance, and the combined export drops it when its inputs were replaced.

Limitations: a large moving subject, parallax, cuts, flat regions and flow errors can produce
wrong labels; no physical pose, focal length or dolly/pan separation is recovered.

## AI brief

On the tool menu, **Export AI brief (Markdown + JSON)** writes a per-shot breakdown built only
from measured results and user labels: the shot table, and for every shot its timing, boundary,
cadence, tone, colour, palette, pixel grid, black bars, camera segments overlapping the shot,
named SAM objects present in the shot (with rule-of-thirds position words and mean area), its
sound (kind shares, dominant kind, mean loudness, beats and dialogue lines), and a one-line
`prompt` scaffold. A **Sound** section before the limitations gives the whole range's loudness,
mix, tempo and cuts on beats. The JSON file next to it holds the same `BriefInput` for programs.

It needs a Shots & look result for the selected range (current or saved). Camera moves, object
masks and the [Sound](reverse-engineering-sound.md) result are added when a current or saved
result for the same range exists; the export's reply says which (`"camera"`, `"sound"`). Command:
`reverseEngineering.exportBrief {"path":"/absolute/breakdown.md"}`; the JSON goes to the same
path with a `.json` extension.

The brief is a reconstruction, not the creator's intent, script or prompt, and it says so in its
limitations section. Text on screen, music stems and depth are not measured yet; see
[the roadmap](reverse-engineering-roadmap.md).

## Verification

`cargo test --locked --offline -p filmcraft-vision --lib` covers cut, black-run and held-frame
detection on synthetic frames, palette and pixel-pitch recovery, letterbox measurement, every
camera label and segment merge, brief rendering, and hostile inputs for each function.

`cargo test --release -p filmcraft-ui-egui --test panels_ui reverse_engineering_shots` runs the
real renderer on the first 60 frames of the demo project: it checks that shots partition the
range, that black shots have no look and visible shots have a bounded palette, shot selection and
playhead movement, invalid options, palette re-detection without re-reading frames, the brief
export (Markdown and JSON) before and after a sound analysis, saving, and that the project is
unchanged.
