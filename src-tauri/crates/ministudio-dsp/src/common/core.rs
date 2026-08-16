pub struct AudioBuffer {
    pub channels: [Vec<f32>; MAX_CHANNELS],
}
impl AudioBuffer {
    pub fn new() -> Self {
        Self {
            channels: [vec![0.0; MAX_BLOCK_SIZE], vec![0.0; MAX_BLOCK_SIZE]],
        }
    }
    pub fn clear(&mut self, frames: usize) {
        for channel in &mut self.channels {
            channel[..frames].fill(0.0);
        }
    }
}

#[derive(Clone, Copy)]
pub struct Smoother {
    current: f32,
    target: f32,
    coeff: f32,
}
impl Smoother {
    pub fn new(value: f32, sample_rate: f32, time_sec: f32) -> Self {
        Self {
            current: value,
            target: value,
            coeff: (-1.0 / (time_sec * sample_rate)).exp(),
        }
    }
    pub fn set_target(&mut self, value: f32) {
        self.target = value;
    }
    #[inline(always)]
    pub fn next(&mut self) -> f32 {
        self.current += (self.target - self.current) * (1.0 - self.coeff);
        self.current
    }
    pub fn advance(&mut self, samples: usize) -> f32 {
        self.current = self.target
            + (self.current - self.target) * self.coeff.powi(samples.min(i32::MAX as usize) as i32);
        self.current
    }
}

pub trait DspEffect: Send {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    /// `events` are sorted by ascending `sample_offset`; offsets are exact
    /// positions inside this block. Effects that do not consume MIDI receive
    /// an empty slice so the realtime path never copies irrelevant events.
    fn process(&mut self, events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize);
    fn process_with_sidechain(
        &mut self,
        events: &[NoteEvent],
        buffer: &mut AudioBuffer,
        sidechain: Option<&AudioBuffer>,
        frames: usize,
    ) {
        let _ = sidechain;
        self.process(events, buffer, frames)
    }
    fn set_param(&mut self, id: &str, value: f32);
    /// Musical tempo at the start of the current processing segment.
    fn set_tempo(&mut self, _bpm: f64) {}
    fn set_bypassed(&mut self, bypassed: bool);
    fn reset(&mut self);
    fn tail_samples(&self) -> usize {
        0
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn wants_midi(&self) -> bool {
        false
    }
    fn response(&self, _points: usize) -> Option<EqFrequencyResponse> {
        None
    }
    fn multiband_levels(&self) -> Option<[[f32; MAX_CHANNELS]; 3]> {
        None
    }
    fn effect_spectrum(&self) -> Option<[f32; DISTORTION_SPECTRUM_BINS]> {
        None
    }
    fn limiter_metrics(&self) -> Option<[f32; LIMITER_METER_VALUES]> {
        None
    }
    fn plugin_control(&self) -> Option<Arc<dyn PluginControl>> {
        None
    }
}
