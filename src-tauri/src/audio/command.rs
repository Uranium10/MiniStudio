// Fixed-shape commands crossing the control-to-audio SPSC queue.
use super::{graph::AudioGraph, instrument::NoteEvent};

#[derive(Clone, Copy)]
pub enum ChainKind {
    Track,
    Bus,
    Master,
}
#[derive(Clone, Copy)]
pub struct EffectRef {
    pub chain: ChainKind,
    pub owner: usize,
    pub effect: usize,
}
pub enum AudioCommand {
    SetPlaying(bool),
    SeekTo(u64),
    Stop,
    SetTrackGain {
        track: usize,
        gain: f32,
    },
    SetTrackPan {
        track: usize,
        pan: f32,
    },
    SetTrackMute {
        track: usize,
        muted: bool,
    },
    SetTrackSolo {
        track: usize,
        solo: bool,
    },
    SetSendGain {
        track: usize,
        send: usize,
        gain: f32,
    },
    SetBusGain {
        bus: usize,
        gain: f32,
    },
    SetMasterGain {
        gain: f32,
    },
    SetEffectParam {
        target: EffectRef,
        param: [u8; 32],
        len: usize,
        value: f32,
    },
    SetInstrumentParam {
        track: usize,
        param: [u8; 32],
        len: usize,
        value: f32,
    },
    LiveMidi {
        track: usize,
        event: NoteEvent,
    },
    SwapGraph(Box<AudioGraph>),
}
pub fn param_key(value: &str) -> ([u8; 32], usize) {
    let mut out = [0; 32];
    let bytes = value.as_bytes();
    let len = bytes.len().min(out.len());
    out[..len].copy_from_slice(&bytes[..len]);
    (out, len)
}
