# Object masks — SAM 2.1

Select a timeline range, open **Object masks · SAM 2.1**, and navigate to a visible frame in
its right column. Name the object and click inside it with **Include (+)** selected. Add more
include clicks or **Exclude (−)** clicks on the same reference frame to refine its selection.
Choose Auto, CPU or GPU and click **Analyze masks**. Each object may have one reference frame
and up to 64 clicks; at most eight objects are supported. Clicking an object's chip returns to
its reference frame. Clear that object to choose a different reference frame.

Inspect the colored pixel mask through the range in the large preview and thumbnails.
The fill has a high-contrast boundary derived from the actual four-neighbour mask pixels.
The main Program monitor remains the original video. Include/exclude markers represent your inputs,
not model measurements. Corrections rerun segmentation through the selected range. This first
version does not perform interactive single-frame preview inference before propagation.

Save, delete and per-tool/combined JSON exports use the common analytical result store. Masks
are independent from optical flow and BootsTAPIR. Device changes apply to the next SAM run;
explicit GPU failure is reported rather than silently retrying CPU. Auto chooses CUDA, then
Apple MPS, then CPU when available. Supported operators must be tested on the actual machine.

## Local runtime

Optional inference lives outside the repository, by default:
- macOS: `~/Library/Application Support/FilmCraft/sam2`
- Linux: `~/.cache/filmcraft/sam2`

Overrides: `FILMCRAFT_SAM2_DIR` and `FILMCRAFT_SAM2_PYTHON`. Use the original executable within
your PyTorch virtual environment, not a symlink bypassing that environment. The app sets
`FILMCRAFT_SAM2_DEVICE` per subprocess. No model is downloaded on editor launch.

Install with an existing environment containing PyTorch >=2.5.1 and matching TorchVision:

```sh
python3 tools/sam2/setup.py \
  --python "/absolute/path/to/your/venv/bin/python" \
  --root "$HOME/Library/Application Support/FilmCraft/sam2"
```

On Linux use `--root "$HOME/.cache/filmcraft/sam2"`. On Windows configure
`FILMCRAFT_SAM2_PYTHON` to the environment's original python.exe; the generated shell launcher
is intended for macOS/Linux. GPU/CUDA installation is specific to the operating system and hardware.
The installer checks the source Git blob hashes and model SHA256, and installs hydra-core 1.3.2,
iopath 0.1.10 and tqdm 4.67.1. It reuses your existing PyTorch/TorchVision rather than replacing them.

Model: SAM 2.1 Hiera Tiny, official Apache-2.0 source and weights.
Source: https://github.com/facebookresearch/sam2
Pinned source: `2b90b9f5ceec907a1c18123530e92e794ad901a4`
Checkpoint: https://huggingface.co/facebook/sam2.1-hiera-tiny
SHA256: `7402e0d864fa82708a20fbd15bc84245c2f26dff0eb43a4b5b93452deb34be69`

The optional worker invokes the official predictor; Rust handles commands, rendering, validation,
mask geometry, UI and exports. CUDA post-processing extensions and torch.compile are disabled.

## Processing and output

Frames are rendered from the composited timeline at a maximum 320-pixel longest side. Rust spools
one RGBA frame at a time; the worker writes JPEG quality 100, subsampling 0 for SAM's official loader.
Mask processing uses 32-frame windows with one shared boundary frame. Each named object propagates
forward and backward from its reference. Later windows are seeded with the previous endpoint mask.
Errors can accumulate; an empty endpoint stops that direction and subsequent masks remain null.
The runtime metadata lists each window and its seed type. There is no arbitrary short range cap:
resource budgets limit disk, result size and object/frame count, with explicit errors.

Limits: 1 GiB rendered temporary data, 64 MB worker JSON, 100,000 object-frame slots, 1-hour inference
timeout and 100 MB shared result store. Temporary frames are deleted on success/error/cancellation.
The external worker nests temporary JPEG folders inside the Rust-owned spool so cancellation
also removes those files.

`inference.tracks[i].object` records name, reference frame and normalized include/exclude clicks.
`inference.tracks[i].masks[frame]` contains null or `{ "runs": [[start, length], ...] }`.
Runs cover foreground pixels in row-major order at `inference.maskSize`; all unlisted pixels are
background. Empty runs mean an empty prediction; null means no prediction. Mask logit threshold is
zero. `geometry[i][frame]` records area in mask pixels and normalized centroid/bounds (or null).
`captures[frame]` preserves exact timeline ticks; frame indices in JSON are zero-based.

These are predicted masks, not ground truth or automatic semantic recognition. Small features,
black frames, occlusion and scene changes can defeat propagation. A user label such as “Sun”
expresses the user's selection; the model does not validate that semantic name.

## Focused validation

`cargo test --locked --offline -p filmcraft-vision --lib` verifies hostile prompts and mask runs,
area/centroid/bounds, and the existing analytical tools. Real model smoke test, without editor build:

```sh
cargo run --locked --offline -p filmcraft-vision --example sam2_ocean -- /tmp/sam2-ocean.json auto 3
```

Use `cpu` or `gpu` to test that backend explicitly; use 33 frames to exercise window continuation.
Ocean Sunset is generated by FilmCraft's existing licensed demo generator. Test results are local
artifacts, not media committed to the repository.

Local real-model validation: Apple MPS / Auto, three frames with a middle-frame reference
produced sun masks with 0.938–1.000 overlap (IoU) against the demo generator’s known disk.
This validates the fixture only; it does not establish accuracy on arbitrary footage.

The 33-frame explicit GPU/MPS test crossed a 32-frame window boundary and returned all 33
masks; ideal-disk IoU averaged 0.996 (minimum 0.913). A separate three-frame CPU test returned
valid sun masks. Device results need not be bit-identical. The desktop editor UI was not
compiled or launched during these focused model tests.

## Mask display controls

The Mask color picker changes the selected object’s overlay color in the large preview and
filmstrip thumbnails. Colors are independent for each named object. Color overlay preserves
the video underneath. Mask only isolates the selected object: included pixels are opaque white
and every other pixel is black. Click markers and point editing are hidden in Mask only; switch
to Color overlay to edit prompts. These controls invalidate the display texture cache immediately
and do not change prompts, measurements or model results, so no reanalysis is required.
