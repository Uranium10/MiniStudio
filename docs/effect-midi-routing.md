# Effect MIDI routing contract

MiniStudio's native effect contract already accepts a sorted `&[NoteEvent]`. Each
`sample_offset` is an exact offset inside the current audio block; it must never
be rounded to the block boundary. Effects that return `false` from
`wants_midi()` receive the shared empty slice.

## Current state

- Instruments receive timeline and live MIDI sample-accurately.
- Effect inserts expose `wants_midi()`, but the graph deliberately supplies an
  empty slice until an explicit effect-MIDI source is stored in the project.
- Colorizer keeps the MIDI choice visible and reports that routing is not yet
  available. With MIDI enabled and no routed notes, its pitch mask is closed.
- Manual pitch classes remain a deterministic source for development and sound
  design. Switching sources updates the spectral mask with frame interpolation;
  it does not reset or cut the existing tail.

## Planned graph edges

1. An instrument track may fan its scheduled/live `NoteEvent` stream out to its
   instrument and to selected post-instrument effects. The same immutable slice
   should be borrowed by both consumers.
2. An effect can select another MIDI-producing track as a source, analogous to
   an audio sidechain. Audio tracks may therefore reference MIDI tracks.
3. Project snapshots must store an effect MIDI source independently from audio
   sidechain routing, including `None`, own-track post-instrument, and another
   track ID.
4. Graph rebuild validation must reject self references and any directed cycle
   across MIDI-source edges. Removal of a source track clears dependent routes.
5. Offline rendering and realtime playback must run the identical scheduler so
   note-on, note-off, controller, pressure, and pitch-bend offsets are preserved.

The DSP core must receive only its fixed-capacity active-pitch snapshot. Source
selection (`Manual`, future `AutoDetect`, or `MidiInput`) stays at the adapter
boundary and must not leak into the spectral algorithm.
