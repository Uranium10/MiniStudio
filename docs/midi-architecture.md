# MiniDAW MIDI architecture

## Time and project model

MIDI note positions are stored as integer ticks at a fixed 960 PPQ. `MidiClip.startSec`
places a clip on the arrangement; note ticks remain relative to the clip. The native
`TempoMap` is the only tick/sample conversion boundary. It currently represents one
fixed BPM, so tempo automation can replace its internals without changing callers.

Projects use format version 2. Loading an older project supplies `kind: audio`, empty
`midiClips`, and a null instrument. Audio and instrument tracks share inserts, sends,
mute/solo, pan, volume, and master routing.

## Scheduling and realtime path

Graph construction expands clip loops and converts each note into a sorted NoteOn and
NoteOff pair. Transpose, velocity scale, and clip-end clipping happen here, outside the
audio callback. During playback each track advances a cursor and copies only events in
the current block into a preallocated event buffer. `sample_offset` is retained, so an
event is never rounded to the start of the block.

Live input uses a bounded lock-free queue. `midir` callbacks push directly into it;
the audio callback drains it and merges live events with scheduled events. Live notes
are processed even when transport playback is stopped. Overflow drops the newest
event instead of allocating or blocking the audio thread.

## Voices and stuck-note prevention

`VoiceAllocator<32>` owns a fixed array. Allocation priority is idle, oldest release,
then oldest held voice. Stealing uses a 3 ms fade before reassignment. NoteOff matches
`note_id`, not pitch, and CC64 defers release until the pedal is lifted.

Stop, seek, graph reset, instrument replacement, and window blur clear active input.
The audio callback still renders release tails while the timeline is stopped. Scheduled
notes that cross a MIDI clip boundary receive NoteOff at the boundary; playback begun
inside an already-started note does not retrigger it.

## UI and performance

The piano roll shares the arrangement `getEffectiveTool` state. Background, notes,
interaction preview, and velocity are separate Canvas 2D layers. Notes stay sorted by
start tick, and visible drawing begins with binary search. Dragging paints a transient
overlay and commits to Zustand on pointer-up, avoiding graph rebuilds per mouse event.
Project history uses structural sharing, preserving large waveform buffers and
untouched tracks.

Offline rendering uses the same graph and MIDI schedule, resets instruments before
rendering, includes instrument release tails, and excludes the live input queue.

