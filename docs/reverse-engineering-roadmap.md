# Reverse engineering: what a full breakdown needs

The goal of the Reverse Engineering panel is to take any video and produce evidence rich enough
that a person or a language model can recreate it: every shot, every component in the shot, the
camera, the look, the sound and the text, each with numbers, timecodes and provenance rather
than guesses. This page ranks what exists against what a complete breakdown needs, using a
32-second pixel-art product spot (16 shots, speech bubbles, UI text, music, hard cuts and holds)
as the reference case.

## What exists (all on a selected timeline range, saved per range, exported as JSON)

| Layer | Source | Answers |
|---|---|---|
| Shots & look | pure Rust | where the cuts and black runs are, how long each shot is, held-frame cadence, key, palette, saturation, warmth, pixel grid, letterbox |
| SEA-RAFT flow | local PyTorch runtime | dense image motion between consecutive frames |
| Global image motion | pure Rust | the 2D similarity shared by the frame (translation, rotation, scale) |
| Camera moves | pure Rust | pan / tilt / zoom / roll / static labels and segments from the global fit |
| Local motion, motion regions | pure Rust | what moves differently from the frame, grouped |
| Motion over time, discontinuities | pure Rust | motion curves and abrupt changes |
| Point tracking | local BootsTAPIR runtime | trajectories of chosen points |
| Object masks | local SAM 2.1 runtime | pixel masks of user-named objects through the range |
| AI brief | pure Rust | one Markdown + JSON document per range combining the above per shot |

## What is missing, ranked by how much of a recreation it unlocks

1. **Whole-video pass and shot-aware ranges.** Shots & look already accepts 36 000 frames, but
   the model tools still work on one 300-frame range. Next: run flow, global motion, camera
   moves and masks per detected shot automatically, and let the brief cover the whole timeline.
2. **On-screen text (OCR).** Speech bubbles, UI labels, prices, titles and credits are the
   literal script of many videos. Needs a local runtime like the SEA-RAFT pattern (a pinned,
   permissively licensed OCR model in its own process; pixel fonts need a model tolerant of
   low resolution) plus Rust-side box tracking, per-shot deduplication and the `notes` field the
   brief already has.
3. **Audio breakdown.** FilmCraft already has a Whisper transcriber (`crates/speech`) and audio
   analysis; the brief needs per-shot dialogue, loudness curve, music/speech/effects segmentation,
   beat positions and whether cuts land on beats. This is the cheapest high-value step because
   the crates exist.
4. **Depth and camera pose.** For live-action material the 2D similarity is not enough. A local
   runtime for a monocular geometry model (Depth Anything 3, VGGT, MegaSaM or ViPE; check each
   licence and whether weights are permissive) would give per-frame depth, intrinsics and camera
   path, which turns "pan left" into a trajectory ArtCraft's 3D stage can replay.
5. **Open-vocabulary objects.** SAM 2.1 needs clicks and a name per object. A text-promptable
   segmenter (SAM 3 class; its licence is custom and must be checked) or a detector would list
   subjects without clicks and let the brief say "two characters, a terminal, a window" by itself.
6. **Semantic captions per shot.** A local vision-language model producing a short caption per
   keyframe, constrained by the measured facts (palette, camera, objects), so the prompt scaffold
   becomes a sentence instead of a list. Must be clearly separated from measurements in the
   output, as the brief does today with its limitations section.
7. **Look transfer, not just description.** Fit a 3D LUT between a neutral reference and the
   graded keyframe, export `.cube`, and estimate grain, vignette, bloom and halation strength so
   the look can be reapplied rather than re-prompted.
8. **Typography and layout.** Font class (pixel, serif, sans, mono), size in frame fractions,
   position, animation (typewriter, fade) for every text block from the OCR step.
9. **Transitions.** Dissolves, wipes, fades and speed ramps are not detected (only hard cuts and
   black). Flow plus the change score over several frames can classify them.
10. **Multi-video comparison.** Compare two breakdowns (reference versus recreation) shot by shot
    with the same metrics, which is how a recreation gets validated.

## Principles that keep this honest

- Every number carries provenance: analyzer id, revision, exact ticks, model checksum when a
  model is involved. The brief only states what a tool measured or a user labelled.
- Rust owns orchestration, validation, rendering, geometry, storage and exports. Models run in
  isolated local processes behind bounded JSON protocols, are never linked, and are optional.
- Heuristics are named as heuristics (pixel pitch, key, camera labels) and keep their thresholds
  visible and adjustable.
- No feature may add a panic path; hostile inputs are tested per function.

## Reference case: the 32-second pixel-art spot

Measured by hand with ffprobe/ffmpeg as the oracle while building Shots & look: 640 × 360 at
30 fps, H.264 plus stereo AAC, 22 scene-change candidates at a 0.15 threshold, including a
cluster of four candidates within 0.13 s where a speech bubble pops in word by word. That cluster
is exactly the case the adaptive ratio and minimum shot length exist for, and the kind of
"change that is not a cut" a transitions layer (item 9) should label instead of swallowing.
