# Look: the grade of a shot as data, words and a reusable component

The **Look** tool of the Reverse Engineering panel reads the grade of every shot in the selected
range: tone, colour, palette, vignette, grain and sharpness, measured on several rendered frames,
classified in a colourist's words under explicit rules, exportable as a `filmcraft.look` file,
copyable as a description for an AI, and applicable to clips as a Lumetri Color grade recorded on
the RE lane. Pure Rust; no model, no download.

## What is measured

Up to *Sampled frames* (default 60, at most 240) of the range are rendered at 320 pixels on the
longest side and spread over the shots: the shots of a current **Shots & look** result for the
same range, else the whole range as one shot. Black shots carry no grade. Each sampled frame gives
(`crates/vision/src/look.rs`, `crates/vision/src/grade.rs`):

| measurement | how |
|---|---|
| mean luma, saturation, warm bias, palette | the Shots & look statistics: means over sampled pixels, red-minus-blue warmth, a deterministic k-means palette |
| black point, white point, contrast | the 1 % and 99 % luma percentiles and their difference |
| vignette | mean luma of the four corners (each 1/6 of the frame) over the centre third's |
| grain | RMS of the fine noise (pixel minus its 3×3 mean) in the flattest third of the 8×8 blocks |
| sharpness | mean absolute luma Laplacian |

Per shot and for the whole range the samples are averaged into a **grade** with the spread of
the mean luma across frames (steady or changing) and a merged palette whose colours are named
from hue, lightness and saturation bands (black, white, greys, red, brown, orange, yellow, green,
teal, blue, purple, pink, with dark / light prefixes).

## The words and their rules

Every word is a classification of the number next to it; the thresholds travel in `rules`:

| word | rule (default) |
|---|---|
| low-key / mid-key / high-key | mean luma < 0.25 / otherwise / > 0.65 |
| flat / normal / punchy contrast | contrast < 0.5 / < 0.8 / else |
| desaturated / natural / vivid | saturation < 0.15 / < 0.35 / else |
| cool / neutral / warm | warm bias < −0.05 / within / > 0.05 |
| strong / light / no vignette | corner-over-centre ratio < 0.55 / < 0.8 / else |
| grainy / clean | grain RMS ≥ 0.02 |
| soft / sharp | mean Laplacian < 0.01 |
| steady / changing | luma coefficient of variation < 0.1 |

The summary reads, for instance: "Look: low-key, punchy contrast, desaturated, warm; dominant
colours dark blue 42 %, brown 20 %, black 15 %; light vignette, clean, sharp; steady across 12
sampled frames."

## Shots first

The look is one grade per shot, so Analyse needs the range's shots. When the Shots & look result
is not current for the range (never run, stale options, changed range or project), `runLook`
starts shot detection itself, returns `{"started":true,"detectingShots":true}`, shows the badge
"Detecting shots…" with the detector's progress, and starts the grade as soon as the shots are
in; Cancel stops both. The shot result stays in place for the Shots & look page. `fromShots`
is therefore true unless shot detection failed, in which case nothing is graded and the error
is shown.

## Where it appears

The page shows the status badge, the whole-range verdict with its palette and **Copy
description**, the actions (Analyse, Apply to the selected clips, Export look…, Details, Advanced
with the palette size and the number of sampled frames), the shared result row (Save result,
Delete result, Export this tool, with the saved/unsaved status), one row per shot with its words and
palette (click to select: the playhead and the picture follow), the selected shot's keyframe, and
under Details every number behind the words plus the limitations.

**Apply** puts the look on the selected clips at the pixel level. Every graded shot (and the
whole range) carries its **colour statistics**: the mean and standard deviation of the Oklab axes
(lightness L, green–red a, blue–yellow b) over the shot's sampled frames
(`crates/render/src/look_transfer.rs`). Applying measures the destination clip the same way
(three frames rendered with its Lumetri switched off) and bakes Reinhard's colour transfer
(Reinhard, Ashikhmin, Gooch and Shirley 2001) in Oklab — each axis re-centred on the reference
mean and rescaled by the ratio of the spreads, bounded ×0.5…×2 — into a 17-point `.cube` LUT
over the sRGB-encoded signal. The LUT goes into the project's LUT library (`lut.add`) and onto
the clip's Lumetri **Creative ▸ Look** slot at 100 % (`creative_on`, `look_lut`, `look_intensity`),
and a `look` component records it, so it shows on the timeline's RE lane, can be bypassed, and
is removed with the slot restored to what it held before. The clip's tone, cast and
colourfulness become the reference's; its objects and textures stay. `applyLook` takes an optional
`strength` (0–1) to blend the transfer. A current result made before the statistics were
recorded gets them computed on the spot from its sampled frames; a look file without them, or a
clip that cannot be rendered, is refused with a message rather than approximated with sliders.
(`lumetri_params` still fills the file's informational `lumetri` block.)

**Where it lands.** Selecting is for analysing; a look always lands on **adjustment layer
clips**, never inside the footage clips, so every look is its own object on the timeline. With
video clips selected, one layer per clip covers that clip's span; with none selected (or with the
chooser's "adjustment layer" target, or `applyLook {"target":"range"}` / `{"range":{"start","end"}}`),
one layer covers a time range: the sequence's In → Out when both are set, else the analysed
range. `components.applyLookRange` measures what the sequence shows in that span, bakes the
transfer, creates the adjustment layer item with Lumetri and the LUT, places it on the first free
video track above the pictures (adding a track when needed) and records a `look` component that
**follows the clip**: drag, trim, cut (razor, also on the RE lane), copy or stack the layer like
any clip and the look moves with it — a razor leaves a look on each piece, removing the look
removes its layer. Different parts of a look file can sit over different parts of the edit. One
undo step per layer.

**Apply look file…** opens a saved `filmcraft.look` file and shows a chooser: the whole-range
look (when the file has several graded shots) and every graded shot with its palette and
summary, each with its own Apply, and a target switch (the selected clips, or an adjustment layer
over the range shown). The chosen grade lands the same way, so a look exported from one video
goes onto another, and the component records the file path in its source. From the control
channel: `chooseLookFile {"path"}` opens the chooser, `applyLook {"path","shot"?,"target"?}`
applies directly.

Commands: `reverseEngineering.runLook`, `reverseEngineering.cancelLook`,
`reverseEngineering.setLookOptions {"paletteSize":6,"maxSamples":60}`,
`reverseEngineering.lookShot {"index":0}`, `reverseEngineering.exportLook {"path"?}`,
`reverseEngineering.applyLook {"clips":[id]?,"shot":index?,"path"?|"file":true}`,
`reverseEngineering.copyLookDescription`. The result lives at
`ui.panels.reverseEngineering.lookReport` (analyzer `filmcraft-vision/look-grade-v1`) and saves
with the other tool results. Automation ids: `reverseEngineering.runLook`,
`reverseEngineering.cancelLook`, `reverseEngineering.copyLookDescription`,
`reverseEngineering.look.{apply,applyFile,export,paletteSize,maxSamples,keyframe}`,
`reverseEngineering.look.overall.palette.N`, `reverseEngineering.look.shot.N`,
`reverseEngineering.look.shot.N.palette.M`, `reverseEngineering.look.shot.N.apply`.

## The look file

`filmcraft.look` version 1: `source` (sequence, size, frame rate, range, analyzer), `overall`
(the grade with its rules), `color` (Oklab mean, std and pixel weight of the whole range), `shots[]`
(index, frames, seconds, black, grade, color), `lumetri` (the
parameters of the overall grade) and `summary`. Copy description puts the same JSON on the
clipboard.

## Verification

`cargo test -p filmcraft-vision --lib grade` (a dark warm vignetted gradient, a flat grey card, a
noisy card, colour names, hostile inputs) and
`cargo test -p filmcraft-ui-egui --test panels_ui reverse_engineering_look` (the demo project's
first 60 frames: options validation, the report and its words, no project change from analysing,
shot selection, the description, the exported file, apply as a component with the Lumetri
parameters written, and the RE lane).
