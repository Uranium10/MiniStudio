// Sample-accurate instrument contract and the intentionally small test-tone synth.
use super::{
    dsp::AudioBuffer,
    types::{db_to_gain, InstrumentSpec},
    MAX_BLOCK_SIZE,
};
use std::{f32::consts::PI, ops::Range};

pub use super::tempo::{TempoMap, MIDI_PPQ};
pub const MAX_VOICES: usize = 32;

pub use ministudio_dsp::{Instrument, NoteEvent, NoteEventKind};

#[derive(Clone, Copy, Debug)]
pub struct LiveMidiMessage {
    pub track: usize,
    pub event: NoteEvent,
}

#[allow(dead_code)]
pub fn for_each_segment(
    events: &[NoteEvent],
    block_len: usize,
    mut callback: impl FnMut(&[NoteEvent], Range<usize>),
) {
    debug_assert!(events
        .windows(2)
        .all(|pair| pair[0].sample_offset <= pair[1].sample_offset));
    debug_assert!(events
        .iter()
        .all(|event| event.sample_offset < block_len as u32));
    let mut cursor = 0;
    let mut start = 0;
    while cursor < events.len() {
        let offset = events[cursor].sample_offset as usize;
        if offset > start {
            callback(&[], start..offset);
        }
        let first = cursor;
        while cursor < events.len() && events[cursor].sample_offset as usize == offset {
            cursor += 1;
        }
        callback(&events[first..cursor], offset..offset);
        start = offset;
    }
    if start < block_len {
        callback(&[], start..block_len);
    }
}

pub fn create_instrument(spec: &InstrumentSpec, sample_rate: f32) -> Option<Box<dyn Instrument>> {
    if spec.bypassed {
        return None;
    }
    if spec.kind.starts_with("vst3:") || spec.kind.starts_with("clap:") {
        return super::plugin::create_external_instrument(spec, sample_rate);
    }
    if spec.kind != "builtin:testtone" {
        return None;
    }
    let mut instrument = TestTone::new();
    instrument.prepare(sample_rate, MAX_BLOCK_SIZE);
    for (id, value) in &spec.params {
        instrument.set_param(id, *value);
    }
    Some(Box::new(instrument))
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
    Steal,
}
#[derive(Clone, Copy)]
struct Pending {
    note_id: i32,
    pitch: u8,
    velocity: f32,
    tuning: f32,
}
#[derive(Clone, Copy)]
struct Voice {
    note_id: i32,
    pitch: u8,
    velocity: f32,
    tuning: f32,
    phase: f32,
    env: f32,
    stage: Stage,
    age: u64,
    sustained: bool,
    pending: Option<Pending>,
    steal_left: usize,
}
impl Default for Voice {
    fn default() -> Self {
        Self {
            note_id: -1,
            pitch: 0,
            velocity: 0.0,
            tuning: 0.0,
            phase: 0.0,
            env: 0.0,
            stage: Stage::Idle,
            age: 0,
            sustained: false,
            pending: None,
            steal_left: 0,
        }
    }
}

pub struct VoiceAllocator<const N: usize> {
    voices: [Voice; N],
    clock: u64,
}
impl<const N: usize> VoiceAllocator<N> {
    fn new() -> Self {
        Self {
            voices: [Voice::default(); N],
            clock: 0,
        }
    }
    fn note_on(&mut self, note: Pending, limit: usize, steal_samples: usize) {
        self.clock = self.clock.wrapping_add(1);
        let count = limit.clamp(1, N);
        let index = (0..count)
            .find(|&i| self.voices[i].stage == Stage::Idle)
            .or_else(|| {
                (0..count)
                    .filter(|&i| self.voices[i].stage == Stage::Release)
                    .min_by_key(|&i| self.voices[i].age)
            })
            .unwrap_or_else(|| (0..count).min_by_key(|&i| self.voices[i].age).unwrap_or(0));
        let voice = &mut self.voices[index];
        if voice.stage == Stage::Idle {
            start_voice(voice, note, self.clock);
        } else {
            voice.pending = Some(note);
            voice.stage = Stage::Steal;
            voice.steal_left = steal_samples.max(1);
        }
    }
    fn note_off(&mut self, note_id: i32, sustain: bool) {
        for voice in &mut self.voices {
            if voice.stage != Stage::Idle && voice.note_id == note_id {
                if sustain {
                    voice.sustained = true
                } else {
                    voice.stage = Stage::Release
                }
            }
        }
    }
    fn release_sustained(&mut self) {
        for voice in &mut self.voices {
            if voice.sustained {
                voice.sustained = false;
                voice.stage = Stage::Release
            }
        }
    }
    fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            if voice.stage != Stage::Idle {
                voice.stage = Stage::Release;
                voice.sustained = false;
                voice.pending = None
            }
        }
    }
}
fn start_voice(voice: &mut Voice, note: Pending, age: u64) {
    *voice = Voice {
        note_id: note.note_id,
        pitch: note.pitch,
        velocity: note.velocity,
        tuning: note.tuning,
        phase: 0.0,
        env: 0.0,
        stage: Stage::Attack,
        age,
        sustained: false,
        pending: None,
        steal_left: 0,
    };
}

pub struct TestTone {
    allocator: VoiceAllocator<MAX_VOICES>,
    sample_rate: f32,
    waveform: u8,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    gain: f32,
    polyphony: usize,
    velocity_curve: f32,
    sustain_pedal: bool,
    pitch_bend: f32,
    modulation: f32,
    modulation_phase: f32,
    expression: f32,
}
impl TestTone {
    pub fn new() -> Self {
        Self {
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            waveform: 0,
            attack: 0.01,
            decay: 0.15,
            sustain: 0.7,
            release: 0.3,
            gain: db_to_gain(-12.0),
            polyphony: 16,
            velocity_curve: 1.0,
            sustain_pedal: false,
            pitch_bend: 0.0,
            modulation: 0.0,
            modulation_phase: 0.0,
            expression: 1.0,
        }
    }
}
impl Instrument for TestTone {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize) {
        self.sample_rate = sample_rate.max(8_000.0);
    }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, frames: usize) {
        debug_assert!(events
            .windows(2)
            .all(|pair| pair[0].sample_offset <= pair[1].sample_offset));
        let mut event_index = 0;
        for sample in 0..frames {
            while event_index < events.len() && events[event_index].sample_offset as usize == sample
            {
                self.handle(events[event_index].kind);
                event_index += 1;
            }
            let vibrato =
                (self.modulation_phase * std::f32::consts::TAU).sin() * self.modulation * 0.5;
            self.modulation_phase =
                (self.modulation_phase + 5.0 / self.sample_rate.max(1.0)).fract();
            let mut mixed = 0.0;
            for voice in &mut self.allocator.voices[..self.polyphony.clamp(1, MAX_VOICES)] {
                if voice.stage == Stage::Idle {
                    continue;
                }
                if voice.stage == Stage::Steal {
                    voice.env *= 0.94;
                    voice.steal_left = voice.steal_left.saturating_sub(1);
                    if voice.steal_left == 0 {
                        if let Some(note) = voice.pending.take() {
                            self.allocator.clock = self.allocator.clock.wrapping_add(1);
                            start_voice(voice, note, self.allocator.clock);
                        }
                    }
                }
                advance_envelope(
                    voice,
                    self.sample_rate,
                    self.attack,
                    self.decay,
                    self.sustain,
                    self.release,
                );
                if voice.stage == Stage::Idle {
                    continue;
                }
                let semitones = voice.pitch as f32 - 69.0
                    + (voice.tuning / 100.0)
                    + self.pitch_bend * 2.0
                    + vibrato;
                let frequency = 440.0 * 2.0_f32.powf(semitones / 12.0);
                let dt = (frequency / self.sample_rate).min(0.49);
                let oscillator = oscillator(self.waveform, voice.phase, dt);
                voice.phase = (voice.phase + dt).fract();
                mixed += oscillator * voice.env * voice.velocity.max(0.0).powf(self.velocity_curve);
            }
            let value = (mixed * self.gain * self.expression * 0.28).tanh();
            out.channels[0][sample] += value;
            out.channels[1][sample] += value;
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "waveform" => self.waveform = value.round().clamp(0.0, 3.0) as u8,
            "attack" => self.attack = value.clamp(0.001, 5.0),
            "decay" => self.decay = value.clamp(0.005, 8.0),
            "sustain" => self.sustain = value.clamp(0.0, 1.0),
            "release" => self.release = value.clamp(0.005, 12.0),
            "gainDb" => self.gain = db_to_gain(value.clamp(-60.0, 6.0)),
            "polyphony" => self.polyphony = value.round().clamp(1.0, MAX_VOICES as f32) as usize,
            "velocityCurve" => self.velocity_curve = value.clamp(0.25, 4.0),
            _ => {}
        }
    }
    fn reset(&mut self) {
        self.allocator.all_notes_off();
        self.sustain_pedal = false;
        self.pitch_bend = 0.0;
        self.modulation = 0.0;
        self.modulation_phase = 0.0;
        self.expression = 1.0;
    }
    fn tail_samples(&self) -> usize {
        (self.release * self.sample_rate) as usize
    }
    fn active_voice_count(&self) -> usize {
        self.allocator
            .voices
            .iter()
            .filter(|voice| voice.stage != Stage::Idle)
            .count()
    }
}
impl TestTone {
    fn handle(&mut self, event: NoteEventKind) {
        match event {
            NoteEventKind::NoteOn {
                note_id,
                pitch,
                velocity,
                tuning_cents,
            } => self.allocator.note_on(
                Pending {
                    note_id,
                    pitch,
                    velocity: velocity.clamp(0.0, 1.0),
                    tuning: tuning_cents,
                },
                self.polyphony,
                (self.sample_rate * 0.003) as usize,
            ),
            NoteEventKind::NoteOff {
                note_id,
                pitch,
                velocity,
            } => {
                let _ = (pitch, velocity);
                self.allocator.note_off(note_id, self.sustain_pedal)
            }
            NoteEventKind::Controller { cc: 64, value } => {
                let was = self.sustain_pedal;
                self.sustain_pedal = value >= 0.5;
                if was && !self.sustain_pedal {
                    self.allocator.release_sustained()
                }
            }
            NoteEventKind::Controller { cc: 1, value } => self.modulation = value.clamp(0.0, 1.0),
            NoteEventKind::Controller { cc: 11, value } => self.expression = value.clamp(0.0, 1.0),
            NoteEventKind::PitchBend { value } => self.pitch_bend = value.clamp(-1.0, 1.0),
            NoteEventKind::AllNotesOff => self.reset(),
            _ => {}
        }
    }
}
fn advance_envelope(
    voice: &mut Voice,
    sr: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
) {
    match voice.stage {
        Stage::Attack => {
            voice.env += 1.0 / (attack * sr).max(1.0);
            if voice.env >= 1.0 {
                voice.env = 1.0;
                voice.stage = Stage::Decay
            }
        }
        Stage::Decay => {
            voice.env += (sustain - voice.env) * (1.0 - (-1.0 / (decay * sr)).exp());
            if (voice.env - sustain).abs() < 0.0001 {
                voice.stage = Stage::Sustain
            }
        }
        Stage::Sustain => voice.env = sustain,
        Stage::Release => {
            voice.env *= (-1.0 / (release * sr)).exp();
            if voice.env < 0.00005 {
                voice.env = 0.0;
                voice.stage = Stage::Idle
            }
        }
        Stage::Steal | Stage::Idle => {}
    }
}
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}
fn oscillator(waveform: u8, phase: f32, dt: f32) -> f32 {
    match waveform {
        1 => 1.0 - 4.0 * (phase - 0.5).abs(),
        2 => (2.0 * phase - 1.0) - poly_blep(phase, dt),
        3 => {
            let mut value = if phase < 0.5 { 1.0 } else { -1.0 };
            value += poly_blep(phase, dt);
            value -= poly_blep((phase + 0.5).fract(), dt);
            value
        }
        _ => (phase * 2.0 * PI).sin(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tempo_round_trip_is_stable() {
        let map = TempoMap::new(120.0, 48_000);
        for ticks in [0, 1, 240, 960, 15_360] {
            assert!(
                (map.samples_to_ticks(map.ticks_to_samples(ticks)) as i64 - ticks as i64).abs()
                    <= 1
            );
        }
    }
    #[test]
    fn event_offsets_change_the_exact_sample() {
        let mut synth = TestTone::new();
        synth.prepare(48_000.0, 64);
        let mut out = AudioBuffer::new();
        synth.process(
            &[NoteEvent {
                sample_offset: 17,
                kind: NoteEventKind::NoteOn {
                    note_id: 1,
                    pitch: 69,
                    velocity: 1.0,
                    tuning_cents: 0.0,
                },
            }],
            &mut out,
            64,
        );
        assert!(out.channels[0][..17].iter().all(|sample| *sample == 0.0));
        assert!(out.channels[0][18..]
            .iter()
            .any(|sample| sample.abs() > 0.0));
    }
}
