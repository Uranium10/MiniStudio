# Effect MIDI routing contract

MiniStudio's native effect contract already accepts a sorted `&[NoteEvent]`. Each
`sample_offset` is an exact offset inside the current audio block; it must never
be rounded to the block boundary. Effects that return `false` from
`wants_midi()` receive the shared empty slice.

## Current state

- Instruments receive timeline and live MIDI sample-accurately.
- MIDI-aware native inserts receive their own instrument track's already sorted
  timeline/live event slice with the original sample offsets and no callback
  copy or allocation.
- Colorizer derives its active pitch-class mask from held notes when MIDI is
  enabled and clears the mask on All Notes Off.
- Manual pitch classes remain the deterministic fallback. The DSP core is a
  fixed-capacity harmonic-family spectral mapper with reported FFT latency.

## Planned graph edges

1. An effect can select another MIDI-producing track as a source, analogous to
   an audio sidechain. Audio tracks may therefore reference MIDI tracks.
2. Project snapshots must store an effect MIDI source independently from audio
   sidechain routing, including `None`, own-track post-instrument, and another
   track ID.
3. Graph rebuild validation must reject self references and any directed cycle
   across MIDI-source edges. Removal of a source track clears dependent routes.
4. Offline rendering and realtime playback must run the identical scheduler so
   note-on, note-off, controller, pressure, and pitch-bend offsets are preserved.

The DSP core must receive only its fixed-capacity active-pitch snapshot. Source
selection (`Manual` or `MidiInput`) stays at the adapter boundary and must not
leak into harmonic-family analysis or phase resynthesis.
