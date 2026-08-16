# Tempo map and audio-source layer

This change establishes the persistent/runtime boundaries needed for future variable-tempo and ARA work. It intentionally does not add the tempo-track editor UI or any ARA SDK integration.

## Musical time

- Project format v3 stores a normalized `tempoMap` with sorted tempo and time-signature points. `transport.bpm` and `transport.timeSignature` remain compatibility readouts for the first points.
- Rust prepares an immutable `TempoMap` while building the graph. Its cumulative sample and time-signature caches are never rebuilt on the audio thread.
- Jump segments use the fixed-tempo formula. Linear BPM ramps use the logarithmic closed-form integral and its exponential inverse; conversion performs a binary search and allocates nothing.
- MIDI events are mapped from clip-local ticks through the global map, and metronome beat/downbeat placement reads the same graph-owned map. VST3 process context receives the current block tempo, and abrupt tempo/signature boundaries split the render block before the next transport snapshot; built-in effects currently expose no tempo-synced parameter, and CLAP transport enrichment remains a separate host-protocol task.
- Arrangement and piano-roll rulers, adaptive grids, snapping/quantize, smart duplication, MIDI recording, MIDI clip trim/split, automation time axes, and clip-start warp ratios all convert through the same prepared TypeScript map.
- The transport readout resolves the current playhead BPM and bar/beat through the prepared TypeScript map. Tempo-point and time-signature editing UI is deliberately deferred.

## Audio sources

The persisted ownership chain is now:

`Clip.audioSourceRefId -> AudioSourceRef.assetId -> AudioAssetInfo`

- Importing creates a fresh source ref. A repeated path reuses project asset metadata while retaining a distinct source identity.
- Duplicate, copy/paste, split and track duplication preserve the source-ref identity.
- **독립된 사본으로 만들기** creates a new ref pointing to the same decoded asset.
- Clip/track deletion removes unused refs and then assets from the project model. Undo snapshots retain their own metadata; native buffer lifetime remains controlled independently so undo cannot resurrect a clip with a prematurely freed buffer.
- The source ref is resolved to a native asset ID only in `RustEngine.toNativeSnapshot`, before graph construction. The realtime graph remains unaware of the extra layer.

## Compatibility

- v1 and v2 files migrate sequentially to v3. Legacy clips sharing an asset are grouped onto one source ref.
- Missing fields receive defaults and unknown fields are ignored.
- Files with a newer format version fail with an explicit compatibility error.
- Version fixtures live under `src/io/tests/fixtures/`.
