# Motion over time

Open Motion over time after selecting a timeline range. Current SEA-RAFT results are reused;
missing results trigger optical-flow inference with the selected execution device.

The right column graph shows three sampled-flow means per consecutive frame comparison:
X displacement (positive right), Y displacement (positive down), and vector magnitude.
Units are analysis pixels per comparison, not full-resolution pixels or pixels per second.
Directional means can cancel opposing movement; mean magnitude preserves its strength.
The vertical scale automatically covers all three series. Missing samples break lines.
Click a graph position to inspect its destination frame; navigation and thumbnails remain below.
No extra overlay is drawn on the image.

Save, delete, per-tool export and combined export use the common analytical result store.
Reports record source flow revision and completion time, frame capture provenance, exact ticks,
and sample counts. Display cutoff and arrow scale do not change this measurement.
This measures sampled image motion, not object identity, camera angle, or semantic activity.
