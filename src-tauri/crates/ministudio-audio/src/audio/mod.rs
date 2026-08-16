// Native audio engine modules and hard real-time capacity limits.
pub mod asset;
pub mod command;
pub mod device;
pub mod dsp;
pub mod engine;
pub mod error;
pub mod graph;
pub mod instrument;
pub mod metrics;
pub mod midi_service;
pub mod plugin;
pub mod runtime;
pub mod tempo;
pub mod types;

pub const MAX_CHANNELS: usize = 2;
pub const MAX_BLOCK_SIZE: usize = 2048;
pub const MAX_TRACKS: usize = 64;
pub const MAX_EFFECT_METERS: usize = 64;
pub const COMMAND_CAPACITY: usize = 1024;
pub const RETIRED_GRAPH_CAPACITY: usize = 8;
