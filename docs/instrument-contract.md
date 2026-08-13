# Instrument contract

An instrument is a native audio-thread object implementing `audio::instrument::Instrument`.
To add a synth, create a separate implementation and register its type string in
`create_instrument`; the scheduler, voice event model, piano roll, and project format do
not need to change.

## Lifecycle

- `prepare(sample_rate, max_block)` preallocates all working storage. It is called away
  from realtime processing.
- `process(events, out, frames)` adds stereo samples to `out`. Events are sorted by
  `sample_offset`, and every offset is inside `0..frames`.
- `set_param(id, value)` receives normalized or explicitly documented domain values and
  must avoid blocking.
- `reset()` clears scheduled/live state for stop, seek, or graph replacement.
- `tail_samples()` reports the longest release tail needed by offline export.
- `active_voice_count()` is a cheap, allocation-free diagnostic for the device UI.

## Realtime rules

`process` must not allocate, lock, access the filesystem, log, or perform IPC. Use
`for_each_segment` to render up to each event boundary, apply events at that exact
sample, then render the next segment. Keep voices and scratch buffers fixed-size.
Never match NoteOff by pitch: overlapping equal-pitch notes are distinguished by
`note_id`.

Supported event shapes mirror the future VST3 adapter: NoteOn, NoteOff, PolyPressure,
Controller, PitchBend, and AllNotesOff. Velocities and controller values are 0..1;
pitch bend is -1..1; tuning is cents. Unknown controllers may be ignored.

## UI/project registration checklist

1. Choose a stable type such as `builtin:my-synth` or `vst3:<uid>`.
2. Add defaults and clipped parameter ranges to the TypeScript instrument catalog/UI.
3. Register construction in `create_instrument` and map the same parameter IDs.
4. Ensure replacement releases the old instrument's voices before graph swap.
5. Add sample-offset, block-size invariance, tail, voice-stealing, and reset tests.

`builtin:testtone` is intentionally only a verification instrument: one oscillator,
PolyBLEP saw/square, ADSR, gain, velocity curve, polyphony, and ±2-semitone bend.
