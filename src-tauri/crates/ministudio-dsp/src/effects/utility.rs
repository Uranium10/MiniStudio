pub struct Utility {
    sample_rate: f32,
    input_mode: u8,
    invert_left: bool,
    invert_right: bool,
    width: Smoother,
    gain: Smoother,
    balance: Smoother,
    mono: bool,
    bass_mono: bool,
    bass_frequency: f32,
    bass_split: Crossover,
    mute: bool,
    dc_block: bool,
    dc_x: [f32; MAX_CHANNELS],
    dc_y: [f32; MAX_CHANNELS],
    bypassed: bool,
}
impl Utility {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            input_mode: 0,
            invert_left: false,
            invert_right: false,
            width: Smoother::new(1.0, 48_000.0, 0.01),
            gain: Smoother::new(1.0, 48_000.0, 0.01),
            balance: Smoother::new(0.0, 48_000.0, 0.01),
            mono: false,
            bass_mono: false,
            bass_frequency: 120.0,
            bass_split: Crossover::new(120.0),
            mute: false,
            dc_block: false,
            dc_x: [0.0; MAX_CHANNELS],
            dc_y: [0.0; MAX_CHANNELS],
            bypassed: false,
        }
    }
}
impl DspEffect for Utility {
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.sample_rate = sample_rate;
        self.bass_split
            .configure(self.bass_frequency, self.sample_rate)
    }
    fn process(&mut self, _events: &[NoteEvent], buffer: &mut AudioBuffer, frames: usize) {
        if self.bypassed {
            return;
        }
        for frame in 0..frames {
            let input = [buffer.channels[0][frame], buffer.channels[1][frame]];
            let (mut left, mut right) = match self.input_mode {
                1 => (input[0], input[0]),
                2 => (input[1], input[1]),
                3 => (input[1], input[0]),
                _ => (input[0], input[1]),
            };
            if self.invert_left {
                left = -left
            }
            if self.invert_right {
                right = -right
            }
            if self.bass_mono {
                let (low_left, high_left) = self.bass_split.process(0, left);
                let (low_right, high_right) = self.bass_split.process(1, right);
                let low_mono = (low_left + low_right) * 0.5;
                left = low_mono + high_left;
                right = low_mono + high_right
            }
            let mid = (left + right) * 0.5;
            let width = self.width.next().clamp(0.0, 2.0);
            let side = if self.mono {
                0.0
            } else {
                (left - right) * 0.5 * width
            };
            left = mid + side;
            right = mid - side;
            let balance = self.balance.next().clamp(-1.0, 1.0);
            let smoothed_gain = self.gain.next();
            let gain = if self.mute { 0.0 } else { smoothed_gain };
            left *= gain * if balance > 0.0 { 1.0 - balance } else { 1.0 };
            right *= gain * if balance < 0.0 { 1.0 + balance } else { 1.0 };
            if self.dc_block {
                let filtered_left = left - self.dc_x[0] + 0.995 * self.dc_y[0];
                let filtered_right = right - self.dc_x[1] + 0.995 * self.dc_y[1];
                self.dc_x = [left, right];
                self.dc_y = [filtered_left, filtered_right];
                left = denormal(filtered_left);
                right = denormal(filtered_right)
            }
            buffer.channels[0][frame] = left;
            buffer.channels[1][frame] = right
        }
    }
    fn set_param(&mut self, id: &str, value: f32) {
        match id {
            "inputMode" => self.input_mode = value.round().clamp(0.0, 3.0) as u8,
            "invertLeft" => self.invert_left = value >= 0.5,
            "invertRight" => self.invert_right = value >= 0.5,
            "width" => self.width.set_target(value.clamp(0.0, 2.0)),
            "gainDb" => self.gain.set_target(db_to_gain(value.clamp(-60.0, 24.0))),
            "balance" => self.balance.set_target(value.clamp(-1.0, 1.0)),
            "mono" => self.mono = value >= 0.5,
            "bassMono" => {
                self.bass_mono = value >= 0.5;
                self.bass_split.reset()
            }
            "bassFreq" => {
                self.bass_frequency = value.clamp(40.0, 500.0);
                self.bass_split
                    .configure(self.bass_frequency, self.sample_rate)
            }
            "mute" => self.mute = value >= 0.5,
            "dcBlock" => self.dc_block = value >= 0.5,
            _ => {}
        }
    }
    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed
    }
    fn reset(&mut self) {
        self.bass_split.reset();
        self.dc_x = [0.0; MAX_CHANNELS];
        self.dc_y = [0.0; MAX_CHANNELS]
    }
    fn runtime_capabilities(&self) -> RuntimeCapabilities {
        if self.bass_mono || self.dc_block {
            RuntimeCapabilities::always_process()
        } else {
            RuntimeCapabilities::no_tail()
        }
    }
}
