# Motion regions: first implementation

Select the range in the main timeline and click **Motion regions**. Current flow, global fit and
local residuals are reused; missing prerequisites are prepared inside the tool. Inspect frame 2
or later with the existing thumbnail strip, frame jump and previous/next controls.

The analyzer selects original SEA-RAFT grid samples whose residual magnitude is above the
cutoff, then builds four-neighbor connected groups. An edge connects two neighboring source
samples only when the difference between their residual vectors is at or below the neighbor
threshold. Groups smaller than the minimum sample count are discarded. Defaults are 0.3 analysis
pixels cutoff, 0.5 pixels neighbor difference and 3 samples minimum.

The overlay draws translucent cells at actual observed sample positions and a bounding rectangle
for each group. Colors and labels distinguish groups in the selected comparison. Hover for the
sample count, mean residual vector, mean magnitude and maximum magnitude. These cells and bounds
show sampled coverage, **not object contours or semantic segmentation**. Group IDs restart per
comparison; this is not tracking. Connected groups can include gradual changes because the
similarity threshold is tested on neighboring samples rather than against one global group mean.

Controls update grouping for the selected incoming frame comparison immediately. **Apply to all**
regroups the complete range and clears overrides. **Use all-frame settings** removes only that
comparison's override. These controls change grouping results, not merely arrow presentation,
and do not rerun the model. Local motion's arrow display cutoff does not affect this tool.

Save/delete/export work like the other tools. Export includes source revision/timestamps, range,
shared and per-frame grouping settings, original sample indices, observed positions and bounds,
mean vectors/magnitudes, maxima, filtered-sample counts and discarded-group counts. A comparison
with unavailable residual evidence remains unavailable with its original reason. Zero groups is
a valid result and is different from unavailable evidence.

For the first UI test, choose a visible Ocean Sunset frame. Lower or raise the cutoff to see how
sample coverage changes. Lower neighbor difference to split groups and increase minimum samples
to remove small groups. Export for inspection before concluding that a group corresponds to the
sun or another object.

Focused tests cover adjoining groups with opposite movement, threshold/minimum effects,
provenance indices, missing fits and malformed inputs. The vision-only `motion_regions` example
can process a Local motion JSON export without inference or an editor build.
