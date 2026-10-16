# 2D camera

The 2D camera component extracts the image-plane camera movement of a range (pan, tilt as
translation, zoom as scale, roll as rotation) as something that can be saved and applied to
another clip. It is built in verifiable steps; each step is a state of the same tool, inspectable
in the panel, exportable as JSON, and tested on synthetic images with a known answer.

Steps: **A** feature points · **B** descriptors and matching between consecutive frames ·
**C** robust similarity per comparison with photometric verification · **D** camera versus
objects, anchors and the jointly solved path · E the path as Motion keyframes and a camera file (all on this page) · formerly: E export as a Motion
preset and apply.

## Steps B and C: matches and one similarity per comparison

For every pair of consecutive frames the tool describes each point, matches the two sets, fits
a 2D similarity to the matches and checks it against the pixels:

- **Descriptor** (`crates/vision/src/matching.rs`, clean-room after Lowe 2004): a 16×16 sample
  grid around the point, spaced by the point's scale and rotated by its orientation, split into
  4×4 cells with an 8-bin gradient-orientation histogram each (128 numbers), Gaussian-weighted,
  normalised, clipped at 0.2 and renormalised. Rotation is absorbed by the orientation, zoom is
  handled per octave (2× steps); in-between zooms rely on the descriptor's tolerance (tested at
  ×1.15 with 25° of rotation). Descriptors are working data; they are never saved or exported.
- **Matching**: the two nearest descriptors by Euclidean distance; Lowe's ratio test (default
  0.75) rejects ambiguous points; an optional cross-check keeps only mutual matches; an optional
  spatial window limits the search to nearby points.
- **Fit** (`crates/vision/src/camera2d.rs`): deterministic RANSAC over two-match hypotheses, then
  least squares on the supporting matches, for `x' = a·x − b·y + tx`, `y' = b·x + a·y + ty`
  (translation, uniform scale, rotation) mapping the previous frame onto the current one.
  Reported per comparison: centre displacement, scale, rotation, RMS residual, inlier indices,
  or an explicit *insufficient* reason (defaults: tolerance 1.5 px, at least 8 inliers and 25 %
  support).
- **Photometric check**: mean absolute luma difference between the current frame and the previous
  frame warped by the fit (bilinear), on a sample grid, next to the same difference without the
  warp. A comparison is **verified** only when the warp lowers the error; when standing still
  explains the two frames at least as well (a static camera with moving content, which is what
  the demo project shows), the fit is reported but flagged as not verified. The Difference view
  shows |current − warp(previous)| amplified ×4.

The panel's left column shows, for the selected frame, its comparison with the previous frame:
match count, the four numbers of the fit with its inliers, the photometric error before and
after the warp, and the verification verdict. The graph in the right column stacks points per
frame over fit inliers per comparison (green verified, amber fitted but not verified, red none).
Views: **Points**, **Matches** (lines from the previous position to the current one; green
supports the fit, red does not) and **Difference**.

Synthetic tests (`cargo test -p filmcraft-vision --lib matching camera2d`): a textured scene
rotated 25° and scaled ×1.15 gives at least 20 matches of which ≥ 90 % land within 2 px of the
true position, two unrelated scenes give at most 5 % spurious matches, a brightness change of
×0.6 + 20 moves descriptors by less than 0.15, the similarity is recovered to 1e-6 with 25 %
outliers, insufficient and random evidence are explicit, and the photometric error of a known
shift drops by more than 5× when the right warp is applied. The UI test checks 70 comparisons on
the demo range, a fitted lit pair with bounded numbers and a consistent verification verdict,
no estimate between black frames, the views, option validation and the export.

## Step D: camera versus objects, anchors and the solved path

A camera moves the whole picture; an object moves a region. Step D uses that to separate the two
and to turn the comparisons into one camera path for the range:

- **Groups first, camera by enclosure, then by spread** (`camera2d::camera_between`): RANSAC
  extracts the coherent motion groups one after another (largest first, up to `maxRegions + 1`).
  An object moves inside the picture while the background surrounds it, so a group whose extent
  encloses every other group (2 % margin) is the camera, however few points it has: the static
  border of a chat window beats the scrolling text inside it. Without an enclosing group, the
  camera is the group that occupies the most cells of a 4×4 grid and spans the most of the frame,
  accepted when it covers at least 6 cells and 50 % of the frame in both directions (adjustable),
  or when it is the largest group and holds at least half of all matches (and twice the minimum
  inliers). Anything else is **no camera evidence**. Known limit: a lone moving group with no
  background evidence at all (a sprite on black) is reported as the camera; the weights below and
  the residual keep it from bending the path, and the figure/object components will claim it.
- **Moving regions**: the groups that are not the camera become up to 3 regions, each with its
  box, mean displacement, scale and rotation. In the Matches view they are cyan boxes; in the
  export they are `regions[]` per comparison. These are the raw material of the future object and
  text components.
- **Pixel refinement** (`camera2d::refine_similarity`): the feature-based similarity is refined
  on the luma planes by Gauss–Newton with a robust weight, excluding the moving regions. This
  removes the sub-pixel "pixel locking" of point detectors that made 1-pixel motions come out
  short (the synthetic zoom test went from 17 % under to within 0.3 %).
- **States** (`camera2d::classify`): *static* (centre displacement ≤ 0.15 px, scale within 0.001,
  rotation within 0.02°), *moving* (the warp lowers the photometric error), *static* again when
  the fitted motion is negligible (≤ 1 px, 0.5 % scale, 0.1°) and standing still explains the
  pair at least as well, *not verified* (a real motion that explains nothing better), *no camera
  evidence*.
- **Anchors**: every `step` frames (default 4) the frame is also compared with the frame `gap`
  earlier (default 8) and with the frame `longGap` earlier (default 24; 0 turns it off), through
  the same matching, fit, refinement and verification, from a ring buffer of the last
  `max(gap, longGap) + 1` analysed frames. The long anchors bridge short stretches without
  evidence (a screen shake, a flash, a few frames of pure motion blur) so the path after them
  returns to the right place instead of keeping a permanent offset.
- **Joint solve** (`crates/vision/src/trajectory.rs`): consecutive comparisons and anchors become
  weighted edges. The weight is the evidence behind the measurement, inliers × (occupied cells /
  16): a static step with 300 inliers over 14 cells weighs 262, a stray 8-point group in 2 cells
  weighs 1, the same as a comparison with no evidence (identity, weight 1, so unsupported stretches
  stay still instead of drifting). The absolute similarity of every frame relative to the first
  is solved in two gauge-invariant stages, each by Jacobi-preconditioned conjugate gradient on
  the normal equations: first the rotation-scales in log form (`log z_to − log z_from = log m`),
  then the translations with the rotation-scales already fixed. The linear form `z_to = m·z_from`
  used before was not scale invariant: a stretch with contradictory evidence (steps saying zoom,
  anchors saying still) lowered its own residual by shrinking everything behind a weak link, which
  on a real video collapsed the path to 40 % zoom through a few no-evidence frames. Each pose
  reports its residual: the largest disagreement between a measurement into that frame and the
  solved path; a large residual on a frame marks evidence the path had to overrule.

The right column gains a **camera path** graph (Δx, Δy and zoom over the range; click to jump)
under the points/state graph, whose lower half is now coloured by state (blue static, green
moving, amber not verified, red no evidence). The left column shows the solved pose at the
selected frame, the comparison's coverage, span and state, and its moving regions.

Verified synthetically (`cargo test -p filmcraft-vision --lib trajectory camera2d`): a chain
with a deliberate 0.2 px bias per step drifts by more than 5 px over 60 frames and comes back
within 0.6 px once anchors are added; a contradictory block behind a single weight-1 link leaves
the frames before the link within 0.3 px and 0.1 % of scale; a sparse enclosing group beats a
dense inner one; a rendered sequence with a smooth pan, zoom and roll plus
a textured object sliding across the frame is recovered within 0.5 px and 0.3 % of scale on every
frame, while the object shows up as a moving region in most comparisons and never as the camera;
pixel refinement recovers a 0.7 px shift with 0.3° roll and 0.2 % zoom from a coarse guess, also
with a masked region; hostile edges and planes are errors.

## Step E: the camera as a reusable component

The solved path becomes "the code" of the camera: Motion keyframes FilmCraft already understands,
and a file that carries them to any project.

- **Maths** (`crates/vision/src/motion.rs`): FilmCraft's Motion maps a source pixel by
  *subtract anchor → scale → rotate (degrees, clockwise on screen) → translate to position*, which
  is exactly the similarity `x' = a·x − b·y + tx, y' = b·x + a·y + ty` with `a = s·cos θ`,
  `b = s·sin θ`. A pose measured on frames analysed at `analysisWidth` is converted to sequence
  pixels (rotation-scale unchanged, translation divided by `analysisWidth / sequenceWidth`), then
  composed with the clip's Motion similarity at that frame, `M_i = C_i ∘ M_i⁰` (**follow**: the
  clip reproduces the camera) or `M_i = C_i⁻¹ ∘ M_i⁰` (**stabilize**: the clip cancels it), and
  decomposed again: scale % = 100·hypot(a, b) / fit (fit is the Scale to Frame Size factor),
  rotation = atan2(b, a) unwrapped so frames never jump by 360°, position = M_i(anchor). The anchor
  is left alone; Uniform Scale is switched on. Composing with the clip's Motion *at each frame*
  (not a single base) means following adds the camera on top of an existing animation and
  stabilising cancels the measured motion whatever produced it.
- **Writing** (`effects.setKeyframes`, `crates/engine/src/commands.rs`): a new engine command that
  writes every keyframe of several parameters in **one undo step**, at timeline ticks converted to
  the clip's media time, replacing the parameter's existing keyframes inside the written span,
  with static values (`set`) in the same step; it refuses times outside the clip, non-animatable
  parameters, wrong types and non-finite values, and changes nothing when it refuses. Any later
  component (text, objects) writes its keyframes the same way.
- **Applying** (`reverseEngineering.applyCamera`): `{"clips":[id]? (the selection),
  "mode":"follow"|"stabilize", "path":file? or "file":true (open dialog), "start":ticks? (where
  frame 0 lands; default the clip start), "atTimeline":true? (the analysed times, for
  stabilising the analysed clip)}`. One keyframe per path frame on position, scale and rotation,
  linear interpolation, exact ticks; frames outside the clip are skipped; one undo step per clip.
  The clip and sequence must share the pixel aspect ratio and the clip's Motion must be a
  similarity (uniform scale). Applying to a clip of the analysed sequence makes the result
  stale, as any edit does; applying to another sequence keeps it.
- **Exporting** (`reverseEngineering.exportCamera {"path"?}`): a `*.camera2d.json` file, format
  `filmcraft.camera2d` version 1: `source` (sequence name, size, frame rate, range, analysis
  width, analyzer, timestamp), `path[]` (per frame: exact `tick` from the first frame, seconds,
  transform in sequence pixels, centre displacement, scale, rotation, residual, state of the
  comparison into the frame) and `motion[]` (the equivalent Motion keyframes for a clip centred
  at 100 % in the source sequence, for humans and other tools). The effect-presets system is not
  used because it rescales Position by the clip size, which is wrong for a camera defined in the
  sequence.
- **Panel**: after an analysis, the row *Step E · camera component* offers **Apply to N selected
  clips**, **Stabilize selected**, **Export camera…** and **Apply camera file…** (automation ids
  `reverseEngineering.camera2d.{apply,stabilize,exportCamera,applyFile}`).

Verified: `cargo test -p filmcraft-vision --lib motion` (round trips of the decomposition with and
without Scale to Frame Size, follow reproduces the camera, stabilise returns an animated clip to
its base, rotation unwrapping, hostile input); `cargo test -p filmcraft-engine --lib
set_keyframes` (nine keyframes in one undo step, the rendered Motion matrix equals the
similarity at every keyframe and interpolates linearly between them, span replacement, hold
interpolation, refusals change nothing); and `cargo test -p filmcraft-ui-egui --test panels_ui
reverse_engineering_camera2d_exports`: a sequence made from a textured still is analysed, exported
(71 frames, identity at frame 0, exact ticks), a synthetic pan and zoom is written back from a
camera file, the rendered Motion equals camera ∘ base at every checked frame, **re-analysing
recovers the applied camera within 0.1 analysis pixel and 0.001 of scale on all 71 frames**,
stabilising with the measured path brings the composite back to standing still, and two undos
remove both applications. The demo's own scenes cannot serve this test: the city shot is a
three-layer parallax and the aurora twinkles and drifts, so no 2D camera is defined on them.

Known limits: the three weak-evidence artefacts of step D pass into the keyframes as they are
(the residual marks them); a clip with non-uniform scale or a different pixel aspect ratio is
refused; keyframes are written per frame (no thinning yet).

### Applied components and the RE lane

An applied camera is more than keyframes: the project records it as an **applied component**
(`crates/project/src/component.rs`, `Sequence::components`): kind (2D camera), name, mode,
clip, start and duration, the effect and parameters written, where it was measured (sequence,
size, frame rate, range, file) and one sample per frame (offset tick, centre displacement in
sequence pixels, scale, rotation, residual, evidence state). The record is written in the
**same undo step** as the keyframes by `components.add` (`crates/engine/src/components.rs`,
which the panel uses instead of `effects.setKeyframes`); `components.remove {"id"}` takes the
record and exactly the keyframes it wrote away in one step; `components.list` reports them with
their change marks; deleting a clip prunes its components. Projects save and reload them.

The Timeline shows them on a fixed **RE lane** above the caption tracks and the video tracks
(`crates/ui-egui/src/panels/timeline_components.rs`), visible while the sequence has components
and switchable from the wrench menu (*Show RE Components Lane*, control channel
`ui.set {"timeline":{"componentsLane":bool}}`). Each component is a bar aligned with its clip and
time: name and mode, a mini graph of the path (Δx yellow, Δy blue, zoom pink, each normalised to
the bar), and vertical marks where the motion changes (green: motion starts, blue: motion stops,
red: no evidence begins, amber: a frame with a residual above 2 px). Hovering shows the facts
(clip, duration and frames, source, samples, largest displacement, zoom range, file, the first
changes); clicking selects the clip, moves the playhead to the component's start and opens the
2D camera tool on it (the bar and the panel's list highlight it); the context menu removes the
component, exports it as a camera file (`reverseEngineering.exportComponent {"id","path"?}`,
rebuilt from the samples) or selects the clip. The 2D camera panel lists the same components
with Go / Remove / Export… buttons. The lane is informative: nothing on it is dragged or trimmed.
Automation ids: `timeline.components.header`, `timeline.components.{id}`,
`timeline.components.{id}.menu.{remove,export,select}`, `timeline.settings.showComponentsLane`,
`reverseEngineering.camera2d.component.{id}[.remove|.export]`.

Verified: `cargo test -p filmcraft-project --lib component` (change marks at motion edges, gaps
and high residuals; hostile samples refused), `cargo test -p filmcraft-engine --lib components`
(add writes keyframes and record in one undo step, list reports frames and marks, the project
round-trips through JSON, remove takes record and keyframes away and keeps the resting value,
refused calls change nothing, deleting the clip prunes the component), and the UI round trip above
also checks the component record, the lane and its toggle, the panel's list, the rebuilt camera
file matching the applied transforms, and that removing or undoing clears the lane.

## Step A: feature points

Open **2D camera** in the components menu with a range selected and press **Analyse**. The tool renders **every frame
of the range** through the full-resolution media path at the analysis width (default 640 pixels,
160–4096), detects feature points on each image in a background job with progress and cancel,
and keeps one record per frame: index, exact timeline tick, analysed size, mean luma and the
points. The viewer opens on the middle frame; First/Previous/Next/Last, the frame number, the
strip or the points-per-frame graph pick another frame without rerunning anything. The frame is
shown as the exact image that was analysed (rendered again for display, the last eight kept in
memory), with the points drawn over it. Changing any option hides the result; **Detect** runs the
whole range again. A result holds at most 2 000 000 points in total; lower max points or shorten
the range otherwise.

### Algorithm (`crates/vision/src/features.rs`, pure Rust, clean-room from Harris & Stephens 1988 and Lowe 2004)

1. Luma of the rendered frame.
2. An octave pyramid (1–6 levels, default 4): each level halves the previous by 2×2 averaging,
   so points survive zooms.
3. Per level, Harris response `det(M) − 0.04·tr(M)²` from gradients of the image smoothed with
   σ = 0.8 and gradient products integrated with σ = 1.0.
4. Local maxima (3×3) with a positive response, three pixels away from the borders, refined to
   sub-pixel by a parabola through the three neighbours in x and in y.
5. Coarse-level maxima are relocated to the strongest fine-level maximum within 2^octave pixels:
   a coarse Harris peak sits several pixels inside its corner, and the fine response says where
   the corner actually is. Their scale and octave are kept.
6. A dominant orientation per point from a 36-bin histogram of gradient directions weighted by
   magnitude and a Gaussian window (radius 4 at the point's level), parabola-interpolated.
7. Candidates below `threshold × strongest response` are dropped (default 0.001; the response
   grows with the fourth power of local contrast, so 0.001 keeps points down to about one fifth
   of the strongest contrast). The rest are taken in decreasing response, skipping any closer
   than `min distance` (default 8 px) to an accepted point, up to `max points` (default 500).

Each point reports x, y (analysis pixels, sub-pixel), scale (1.5·2^octave), orientation
(radians), response (relative to the strongest point of the frame) and octave. The hover text
also converts the position to source pixels.

### Verified

`cargo test --locked -p filmcraft-vision --lib features`:

- rectangles on a flat background: every true corner has a detected point within 1.5 px, every
  detected point is within 3 px of a true corner, and the flat background yields none;
- the same picture rotated 30° and scaled ×1.5: at least 85 % of the points are found again
  within 1.5 px of their transformed position (measured: all of them within 0.3 px);
- max points and min distance are respected; a flat frame gives no points; hostile dimensions,
  short buffers and invalid options are errors.

`cargo test -p filmcraft-ui-egui --test panels_ui reverse_engineering_camera2d` renders the
first 71 demo frames: every frame gets a record, the dip-to-black opening frame has no points,
frame 61 arrives lit (mean luma above 0.03, so the analysed image is the real frame) with
points inside the image and bounded values, the viewer opens on the middle frame, navigation
moves the playhead without rerunning, invalid options are rejected, changed options hide the
result until detection reruns, the per-frame cap holds, the JSON export (tagged `camera2d`)
carries all frames, and the project is unchanged.

### Commands

`reverseEngineering.chooseTool {"tool":"camera2d"}` (shows the tool and restores a saved current
result; it no longer starts an analysis, since the tool also applies camera files),
`reverseEngineering.runFeatures`, `reverseEngineering.cancelFeatures`,
`reverseEngineering.featureFrame {"frameIndex":0}` (display only),
`reverseEngineering.setFeatureOptions {"maxPoints":500,"minDistance":8,"threshold":0.001,"octaves":4,"analysisWidth":640,"ratio":0.75,"crossCheck":true,"maxDistancePx":0,"tolerancePx":1.5,"minInliers":8,"minSupport":0.25,"minCoverageCells":6,"minSpan":0.5,"maxRegions":3,"staticPx":0.15,"anchorGap":8,"anchorStep":4,"anchorLongGap":24}`
(applies to the next analysis), `reverseEngineering.setFeatureView {"view":"points"|"matches"|"difference"}`.
State: `ui.panels.reverseEngineering.camera2dReport`, schema version 4 with `frames[]`,
`comparisons[]` (matches, fit, coverage, span, regions, state, photometric before/after,
verified), `anchors[]` (the same records for anchor pairs) and `trajectory[]` (one absolute
pose per frame: transform, centre displacement, scale, rotation, residual). Save, delete and export use the common result store (tool id
`camera2d`). Automation ids: `reverseEngineering.runFeatures`, `reverseEngineering.cancelFeatures`,
`reverseEngineering.camera2d.{maxPoints,minDistance,threshold,octaves,analysisWidth,summary,graph,jump,First,Previous,Next,Last}`,
`reverseEngineering.camera2d.resultFrame.N`, `reverseEngineering.camera2d.resultThumbnail.N`,
`reverseEngineering.frame`.

### Limits

Points need two-directional contrast; flat areas, straight edges and low-contrast texture give
none. Positions are in the analysed image; the source conversion is a scale factor. Each
comparison is an independent 2D similarity under a static-scene assumption: a large moving
subject can dominate it, and nothing is chained across the range yet.
