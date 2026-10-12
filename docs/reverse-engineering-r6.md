# R6: Local residual motion

Select a range in the main timeline and click **Local motion**. Current SEA-RAFT and global-fit
results are reused. Missing input is prepared inside the tool, then residuals are calculated.
This does not generate videos or download another model.

The shared frame viewer includes the same thumbnails, direct frame jump and timestamp navigation.
Frame 1 is the reference; each later frame shows motion from its preceding frame. Arrows are
anchored at the measured destination in the displayed frame. Hover near an arrow for its original
SEA-RAFT sample index, observed displacement, global displacement and residual displacement.
All numbers use the analysis image's pixel dimensions, not full-resolution pixels.

For source position `(x,y)` and global similarity `[a,b,tx,ty]`, predicted destination is
`(a*x-b*y+tx, b*x+a*y+ty)`. Residual is observed destination minus predicted destination.
This removes translation, rotation and uniform scaling at each sample position; it is not just
subtracting the center displacement. Failed global fits produce an explicitly unavailable
comparison, never an assumed stationary camera.

The cutoff, arrow scale and display grid stride are presentation controls. Editing applies to
motion into the selected frame. **Apply to all** replaces shared settings and clears overrides;
**Use all-frame settings** removes only the selected frame's override. Neither changes measurements.
Local display settings are separate from the raw-flow display settings.

**Save result**, **Delete result** and **Export this tool** use the existing analysis store.
Combined export includes saved local results, rejecting conflicts with saved source flow/global
results. Each local export records source revisions, source completion timestamps, fitted matrices,
original sample indices, source/destination positions, observed/global/residual displacements,
model uncertainty and per-frame display settings. New upstream results invalidate residuals.

Residuals can include subject motion, parallax, occlusion, cuts and estimation errors. They do not
identify the sun, track objects, recover physical camera angles or establish confidence.

## First test

Use Ocean Sunset and the same range used in R5. Click Local motion, select frame 2 or later and
inspect several frames. Try display grid stride 1 and an enlarged arrow scale for small movements.
Experiment with cutoff rather than assuming a universal noise threshold. Export the result so we
can check whether the sun's residuals differ from the surrounding background.

Core tests cover cancellation of global translation, preservation of local displacement,
position-dependent rotation/scale compensation, missing fits, and malformed numeric inputs.

Verification against the supplied R5 Ocean Sunset combined export produced 118 available residual
comparisons out of 118. Maximum sample residual was 3.4460 analysis pixels. This checks arithmetic
and compatibility with real exported evidence; it does not establish which samples belong to the sun.
The editor UI is left for manual testing.

## Compare before adding another analyzer

Inside Local motion, select a non-reference frame and switch between **Raw SEA-RAFT** and
**Residual motion**. Both views anchor the same samples at their observed current-frame positions
and share the same cutoff, scale, spatial stride and frame overrides. Only displacement changes.
The caption compares mean observed and residual magnitudes in analysis pixels. Cutoff is applied
to the currently displayed displacement, so sample visibility can differ between views.
Switching views performs no inference and changes no saved measurements.
