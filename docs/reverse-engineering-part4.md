# R4 — learned optical-flow evaluation

Status: SEA-RAFT is now connected in the motion UI and export code. The Rust connector has passed the full Ocean Sunset range test; the editor has not been rebuilt or visually verified after integration.

## Existing inspection UI

The motion inspection preview, pair navigation, and export controls are outside the settings scroll area. The settings occupy the upper 40% of the panel; the inspection occupies the lower area. Motion arrows are cyan and enlarged for visibility; stationary matches use yellow markers. This UI change is implemented but has not been visually verified after the change. No editor build was run for R4.

## Evaluated reference

- SEA-RAFT: https://github.com/princeton-vl/SEA-RAFT
- Source revision: `e0882ac0cc336057eedecd4f793b9ca5b7be9dc9`
- Checkpoint: `MemorySlices/Tartan-C-T-TSKH-spring540x960-M`
- Checkpoint revision: `eb97ef34ba5d856c3fa2cdcd073150c057ac8b69`
- Code and checkpoint model-card license: BSD-3-Clause.
- Environment: Apple M1 Max, 64 GB, native ARM Python 3.12, PyTorch 2.7.1, torchvision 0.22.1.
- Four refinement iterations, 384 × 256 synthetic images. Accuracy measured inside a 24-pixel border.
- Checkpoint loaded strictly after restoring safetensors-deduplicated BatchNorm aliases, verified to refer to the same module. No missing parameters silently accepted.

## Initial results

Synthetic image: deterministic colored rectangles on a flat background. Second image either identical or translated by (+3, +2) pixels. This is a basic sanity check, not a real-footage benchmark.

| Test | Expected flow | Metal median flow | Median endpoint error | Metal time |
| --- | --- | --- | --- | --- |
| Static | (0, 0) px | (0.0066, -0.0203) px | 0.0244 px | 3.624 s, including first-run warm-up |
| Translation | (3, 2) px | (2.1203, 1.7203) px | 1.0476 px | 0.076 s |

CPU reproduced the same errors (static 0.0244 px; translation 1.0476 px), with approximately 0.61 s per pair. The translation error therefore is not specific to Metal. Flat regions make correspondence ambiguous; these aggregate numbers do not establish performance on textured natural video. Single-pair timings are not sustained throughput measurements.

## Reproduce locally

The isolated evaluation directory has moved to `~/Library/Application Support/FilmCraft/sea-raft` (persistent local runtime; not committed). It contains the pinned reference checkout, model weights, ARM virtual environment, dependency lock, `evaluate.py`, and `results/` JSON, dense flow arrays, and enlarged-arrow PNG overlays.

```sh
cd "$HOME/Library/Application Support/FilmCraft/sea-raft"
venv-arm/bin/python evaluate.py --device mps
venv-arm/bin/python evaluate.py --device cpu
venv-arm/bin/python evaluate.py --first /absolute/frame1.png --second /absolute/frame2.png --device mps
```

Custom pairs must have matching dimensions; their longest side is limited to 640 pixels. Overlay arrows are enlarged four times; numeric flow is in analysis-image pixels. Each run currently replaces results from the previous run.

## Next decision

Evaluate the user's actual selected footage, including subtle motion, moving subjects, and camera motion. Compare models on the same frame pairs before selecting an inference engine or Rust integration. WAFT remains untested: its official reference setup uses CUDA and xformers, which require portability work on this Mac.

The application remains Rust. The Python reference is an external evaluation experiment, not a product dependency. Optical flow measures image displacement; camera geometry, subject identities, and lighting need additional analyzers.


## Standard fixture: Ocean Sunset

Ocean Sunset is an in-memory procedural generator, not a physical MP4 file. Use FilmCraft's original generator for all repeatable motion comparisons. The Rust-only `filmcraft-vision` example exports frames 43, 44, 49, and 67 at 640 × 360 and records the existing sparse analyzer's results. It does not build or launch the editor.

```sh
cargo run --locked -p filmcraft-vision --example ocean_motion -- /tmp/filmcraft-part4/ocean
cd "$HOME/Library/Application Support/FilmCraft/sea-raft"
venv-arm/bin/python evaluate.py --ocean-dir /tmp/filmcraft-part4/ocean --device mps
```

Frame pairs: identical frame; one frame apart (~0.0417 s); six frames apart (~0.2503 s); 24 frames apart (~1.001 s). Source frame numbers are zero-based. Source ticks and rate are in the exported JSON.

| Frame pair | SEA-RAFT mean water displacement (640 × 360 pixels) | SEA-RAFT inference time | Sparse accepted / tested points |
| --- | --- | --- | --- |
| 43 → 43 | 0.030 px | 1.178 s, first-run warm-up | 5 / 190 |
| 43 → 44 | 0.374 px | 0.111 s | 4 / 190 |
| 43 → 49 | 1.658 px | 0.110 s | 3 / 190 |
| 43 → 67 | 5.037 px | 0.155 s | 0 / 190 |

The learned model provides much broader evidence than the current sparse baseline on this low-texture scene. Its overlay shows displacement concentrated around the changing water reflections and sun. There is no ground-truth correspondence for the generator's changing wave brightness, so these are estimated motion and coverage results, not proof of exact accuracy. Dense and sparse magnitudes are not directly comparable: the sparse analyzer uses 384 × 216 images and only accepted points.

Historical numeric results are recorded in `docs/reverse-engineering-part4-ocean.json`. Historical benchmark overlays and dense arrays are in the persistent runtime’s `ocean-results/` directory. No model weights or generated images are added to the repository. The current Analyze button now calls SEA-RAFT through the Rust connector; there is no custom fallback.


## R4 integration verification

The same Rust connector now used by the UI was run on source frames 34–141 inclusive: 108 frames, 107 consecutive pairs, 880 normalized dense-flow samples per pair. All pairs succeeded on Metal (`mps`). The model/checkpoint/source identifiers and SHA-256 are verified by the connector. Complete local numeric output: `/tmp/filmcraft-sea-raft-ui-check/sea-raft.json.gz`. The UI exports the same evidence with exact timeline/source ticks and schema version 4.

The current Rust fixture example now calls SEA-RAFT instead of the deleted custom tracker. Earlier tables in this document remain historical comparison results; the old sparse analyzer is no longer runnable.


## User-controlled display

Schema version 4 saves presentation settings separately from model evidence. Each incoming frame pair can override visibility, movement cutoff (analysis pixels), arrow scale, and spatial grid stride. Apply to all replaces shared settings and clears overrides; inheritance resets only one frame. All original measurements remain unchanged, including those hidden by the display filter. Fixed moving/stationary categories and colored rings have been removed. Hover/click selects actual model samples and exposes numeric displacement and uncertainty.

Focused settings tests pass for per-frame inheritance, apply-all behavior, serialization, invalid inputs, and cutoff/spatial filtering. The desktop app has not been launched or rebuilt for this UI change. The temporary benchmark PNGs were removed to recover disk space; its numeric report was retained as verified gzip.
