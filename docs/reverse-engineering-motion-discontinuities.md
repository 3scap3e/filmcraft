# Motion discontinuities

Select a range and open Motion discontinuities. Current SEA-RAFT flow is reused; otherwise
the tool starts flow inference using the existing execution preference. No new model is needed.

Each adjacent pair of motion summaries produces:
- Direction change: Euclidean difference of the mean X/Y displacement.
- Strength change: absolute difference of mean vector magnitude.
- Score: the larger of those two changes, in analysis pixels per comparison.

Default threshold is 0.1 analysis pixels, adjustable from 0.001 to 100. Threshold changes
reclassify the stored measurements immediately without another model run. Yellow bars are
candidates at or above the threshold; blue bars are smaller changes. Click a bar or candidate
button to inspect its destination frame. Navigation, image and filmstrip remain in the right
column. No synthetic arrows or object overlays are added.

Three selected frames provide the first change score: the first two frames establish the first
flow comparison. Missing measurements or non-contiguous comparisons export null scores and
are never treated as zero. Steady high-speed motion need not trigger a change event.

Candidates are not confirmed scene cuts. Whole-image means can hide local changes, and
transitions, illumination changes, rapid movement or bad flow can produce peaks. This tool
will not fix the unresolved flow predictions on the black opening of Ocean Sunset.

Save/delete, per-tool JSON and combined JSON use the existing result store. Reports include
the threshold, exact timeline ticks, range-relative zero-based frame indices, capture provenance,
flow revision/completion stamp, component changes and all candidate decisions. The UI uses
one-based frame numbers. Conflicting saved flow provenance is excluded from combined exports.

Focused tests cover steady motion, directional reversal with unchanged strength, strength jumps,
threshold changes, missing measurements, timestamp gaps and invalid thresholds.
