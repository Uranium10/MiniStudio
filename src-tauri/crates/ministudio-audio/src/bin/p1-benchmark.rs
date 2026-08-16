//! Reproducible local P1 scheduler benchmark.
//!
//! Run with:
//! `cargo run --release -p ministudio-audio --bin p1-benchmark`

use std::{collections::HashMap, time::Instant};

use ministudio_audio::audio::{
    graph::AudioGraph,
    instrument::{NoteEvent, NoteEventKind},
    types::{
        BusSpec, EffectSpec, GraphSnapshot, InstrumentSpec, Level, LoopSpec, MasterSpec,
        MidiClipSpec, SendSpec, SidechainSpec, TrackSpec, TransportSpec,
    },
    MAX_TRACKS,
};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 256;
const WARMUP_BLOCKS: usize = 256;
const MEASURE_BLOCKS: usize = 4_096;

fn effect(id: String, kind: &str) -> EffectSpec {
    EffectSpec {
        id,
        kind: kind.into(),
        bypassed: false,
        plugin: None,
        sidechain: None,
        params: HashMap::new(),
    }
}

fn track(id: &str) -> TrackSpec {
    TrackSpec {
        id: id.into(),
        kind: "instrument".into(),
        name: id.into(),
        clips: Vec::new(),
        midi_clips: Vec::<MidiClipSpec>::new(),
        instrument: None,
        volume_db: 0.0,
        pan: 0.0,
        muted: false,
        solo: false,
        effects: Vec::new(),
        sends: Vec::new(),
        output_bus_id: None,
    }
}

fn snapshot() -> GraphSnapshot {
    GraphSnapshot {
        tracks: Vec::new(),
        buses: Vec::new(),
        master: MasterSpec {
            volume_db: 0.0,
            effects: Vec::new(),
        },
        transport: TransportSpec {
            bpm: 120.0,
            tempo_points: Vec::new(),
            time_signatures: Vec::new(),
            playhead_sec: 0.0,
            is_playing: true,
            loop_: LoopSpec {
                enabled: false,
                start_sec: 0.0,
                end_sec: 0.0,
            },
        },
    }
}

fn silent_fx(count: usize) -> GraphSnapshot {
    let mut spec = snapshot();
    let mut silent = track("silent");
    silent.effects = (0..count)
        .map(|index| effect(format!("utility-{index}"), "builtin:utility"))
        .collect();
    spec.tracks.push(silent);
    spec
}

fn active_fx(count: usize) -> GraphSnapshot {
    let mut spec = silent_fx(count);
    spec.tracks[0].instrument = Some(InstrumentSpec {
        id: "benchmark-synth".into(),
        kind: "builtin:testtone".into(),
        plugin: None,
        params: HashMap::new(),
        bypassed: false,
    });
    spec
}

fn tail_heavy() -> GraphSnapshot {
    let mut spec = active_fx(8);
    spec.tracks[0].effects.extend((0..16).flat_map(|index| {
        [
            effect(format!("delay-{index}"), "builtin:delay"),
            effect(format!("reverb-{index}"), "builtin:reverb"),
        ]
    }));
    spec
}

fn sidechain() -> GraphSnapshot {
    let mut spec = active_fx(16);
    spec.tracks[0].sends.push(SendSpec {
        id: "benchmark-send".into(),
        target_bus_id: "return".into(),
        gain_db: -6.0,
        pre_fader: false,
    });
    let mut compressor = effect("sidechain-compressor".into(), "builtin:compressor");
    compressor.sidechain = Some(SidechainSpec {
        enabled: true,
        source_track_id: Some("silent-source".into()),
    });
    let mut source = track("silent-source");
    source.instrument = Some(InstrumentSpec {
        id: "sidechain-synth".into(),
        kind: "builtin:testtone".into(),
        plugin: None,
        params: HashMap::new(),
        bypassed: false,
    });
    spec.tracks.push(source);
    spec.buses.push(BusSpec {
        id: "return".into(),
        name: "return".into(),
        effects: vec![compressor],
        volume_db: 0.0,
    });
    spec
}

fn benchmark(name: &str, spec: &GraphSnapshot, scheduler: bool, live: bool) {
    let (mut graph, _) = AudioGraph::build(spec, &HashMap::new(), SAMPLE_RATE).unwrap();
    graph.set_scheduler_enabled(scheduler);
    if live && !spec.tracks.is_empty() {
        graph.push_live_event(
            0,
            NoteEvent {
                sample_offset: 0,
                kind: NoteEventKind::NoteOn {
                    note_id: 1,
                    pitch: 60,
                    velocity: 0.8,
                    tuning_cents: 0.0,
                },
            },
        );
    }
    let mut output = [0.0_f32; BLOCK * 2];
    let mut levels = [Level::default(); MAX_TRACKS];
    let mut master = Level::default();
    let mut position = 0_u64;
    for _ in 0..WARMUP_BLOCKS {
        graph.process(
            position,
            BLOCK,
            &mut output,
            &mut levels,
            &mut master,
            false,
        );
        position += BLOCK as u64;
    }
    let started = Instant::now();
    for _ in 0..MEASURE_BLOCKS {
        graph.process(
            position,
            BLOCK,
            &mut output,
            &mut levels,
            &mut master,
            false,
        );
        position += BLOCK as u64;
    }
    let elapsed = started.elapsed();
    let ns_per_block = elapsed.as_nanos() as f64 / MEASURE_BLOCKS as f64;
    let deadline_ns = BLOCK as f64 / SAMPLE_RATE as f64 * 1_000_000_000.0;
    let metrics = graph.scheduler_snapshot();
    println!(
        "| {name} | {} | {:.0} | {:.3} | {}/{} | {} |",
        if scheduler { "ON" } else { "OFF" },
        ns_per_block,
        ns_per_block / deadline_ns * 100.0,
        metrics.sleeping_nodes,
        metrics.total_nodes,
        metrics.skipped_process_calls,
    );
}

fn main() {
    println!("MiniStudio P1 benchmark: release, {SAMPLE_RATE} Hz, {BLOCK} samples");
    println!("| Scenario | Scheduler | ns/block | Deadline % | Sleeping/Total | Skips |");
    println!("|---|---:|---:|---:|---:|---:|");
    let scenarios = [
        ("Empty", snapshot(), false),
        ("Silent FX 100", silent_fx(100), false),
        ("Silent FX 500", silent_fx(500), false),
        ("Active FX 100", active_fx(100), true),
        ("Tail Heavy", tail_heavy(), true),
        ("Sidechain", sidechain(), true),
        ("Live", active_fx(32), true),
    ];
    for (name, spec, live) in scenarios {
        benchmark(name, &spec, false, live);
        benchmark(name, &spec, true, live);
    }
}
