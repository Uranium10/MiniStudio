# Track automation

MiniStudio stores automation lanes with each track. The arrangement header's small disclosure button opens the lane area and creates a Volume lane the first time it is used.

- Double-click an automation curve to create a point.
- Drag a point to change time and value. Timeline snap is shared with clips and notes.
- Right-click a point to remove it.
- `+` opens a categorized picker for mixer, instrument, built-in DSP, VST3, and CLAP parameters.
- Playback uses Read automation. Values are linearly interpolated at the UI control rate and passed through the native engine's parameter smoothers.

External plug-in parameters come from VST3 parameter metadata and the CLAP params extension. VST3 values use normalized 0–1 ranges. CLAP preserves each plug-in's declared plain-value range and parameter/module names.

The current automation path is control-rate rather than sample-accurate. Recording Write/Touch/Latch modes and sample-offset parameter queues are future extensions of the same stored lane model.
