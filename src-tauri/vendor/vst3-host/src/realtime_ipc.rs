//! Fixed-capacity realtime transport for process-isolated plug-ins.
//!
//! Control and state commands continue to use the bounded JSON protocol, but audio samples,
//! timestamped MIDI and sample-accurate parameter changes cross through one preallocated shared
//! memory block. The audio callback performs only bounded copies plus two Windows event calls.

use crate::{
    audio::BusAudioBuffers,
    error::{Error, Result},
    midi::{MidiChannel, MidiEvent},
    Plugin,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering as AtomicOrdering},
    Arc, Mutex,
};

/// Maximum number of audio buses represented in one realtime block.
pub const MAX_REALTIME_BUSES: usize = 32;
/// Maximum total channels in either direction.
pub const MAX_REALTIME_CHANNELS: usize = 64;
/// Maximum frames per realtime block.
pub const MAX_REALTIME_FRAMES: usize = 4096;
/// Maximum timestamped MIDI messages per block.
pub const MAX_REALTIME_MIDI_EVENTS: usize = 1024;
/// Maximum sample-accurate parameter changes per block.
pub const MAX_REALTIME_PARAMETER_EVENTS: usize = 1024;

const MAGIC: u32 = u32::from_le_bytes(*b"MSRT");
const VERSION: u32 = 2;
const STATE_IDLE: u32 = 0;
const STATE_REQUESTED: u32 = 1;
const STATE_PROCESSING: u32 = 2;
const STATE_DONE: u32 = 3;
const STATE_SHUTDOWN: u32 = 4;
const STATUS_OK: i32 = 0;
const STATUS_BAD_SHAPE: i32 = -1;
const STATUS_PLUGIN_ERROR: i32 = -2;

fn editor_action_bit(action: crate::process_isolation::EditorHostAction) -> u32 {
    use crate::process_isolation::EditorHostAction::*;
    1 << match action {
        Closed => 0,
        TogglePower => 1,
        TogglePin => 2,
        ToggleBypass => 3,
        SavePreset => 4,
        LoadPreset => 5,
        ShowSidechain => 6,
        AutomationOff => 7,
        AutomationWrite => 8,
        AutomationRead => 9,
        AutomationLatch => 10,
    }
}

fn encode_editor_actions(actions: &[crate::process_isolation::EditorHostAction]) -> u32 {
    actions
        .iter()
        .copied()
        .fold(0, |bits, action| bits | editor_action_bit(action))
}

fn decode_editor_actions(bits: u32) -> Vec<crate::process_isolation::EditorHostAction> {
    use crate::process_isolation::EditorHostAction::*;
    [
        Closed,
        TogglePower,
        TogglePin,
        ToggleBypass,
        SavePreset,
        LoadPreset,
        ShowSidechain,
        AutomationOff,
        AutomationWrite,
        AutomationRead,
        AutomationLatch,
    ]
    .into_iter()
    .filter(|action| bits & editor_action_bit(*action) != 0)
    .collect()
}

/// Portable names required for a helper process to open the host-created transport objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeDescriptor {
    /// Named Windows file mapping.
    pub mapping_name: String,
    /// Auto-reset event signalled by the host when a block is ready.
    pub request_event_name: String,
    /// Auto-reset event signalled by the helper when a block is complete.
    pub response_event_name: String,
    /// Auto-reset event signalled when helper-owned editor actions are available.
    pub editor_action_event_name: String,
    /// Auto-reset event signalled when the realtime client latches a fault.
    pub fault_event_name: String,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RealtimeMidiMessage {
    sample_offset: u32,
    status: u8,
    data1: u8,
    data2: u8,
    _padding: u8,
}

impl RealtimeMidiMessage {
    const EMPTY: Self = Self {
        sample_offset: 0,
        status: 0,
        data1: 0,
        data2: 0,
        _padding: 0,
    };
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RealtimeParameterChange {
    id: u32,
    sample_offset: u32,
    value_bits: u64,
}

impl RealtimeParameterChange {
    const EMPTY: Self = Self {
        id: 0,
        sample_offset: 0,
        value_bits: 0,
    };
}

#[repr(C)]
struct RealtimeBlock {
    magic: u32,
    version: u32,
    state: u32,
    status: i32,
    request_sequence: u64,
    response_sequence: u64,
    frames: u32,
    input_bus_count: u16,
    output_bus_count: u16,
    input_channel_count: u16,
    output_channel_count: u16,
    midi_count: u16,
    parameter_count: u16,
    editor_action_bits: AtomicU32,
    input_bus_channels: [u16; MAX_REALTIME_BUSES],
    output_bus_channels: [u16; MAX_REALTIME_BUSES],
    input_bus_active: [u8; MAX_REALTIME_BUSES],
    output_bus_active: [u8; MAX_REALTIME_BUSES],
    midi: [RealtimeMidiMessage; MAX_REALTIME_MIDI_EVENTS],
    parameters: [RealtimeParameterChange; MAX_REALTIME_PARAMETER_EVENTS],
    input_samples: [f32; MAX_REALTIME_CHANNELS * MAX_REALTIME_FRAMES],
    output_samples: [f32; MAX_REALTIME_CHANNELS * MAX_REALTIME_FRAMES],
}

#[cfg(target_os = "windows")]
struct RealtimeMapping {
    mapping: usize,
    request_event: usize,
    response_event: usize,
    editor_action_event: usize,
    fault_event: usize,
    block: *mut RealtimeBlock,
}

#[cfg(target_os = "windows")]
unsafe impl Send for RealtimeMapping {}
#[cfg(target_os = "windows")]
unsafe impl Sync for RealtimeMapping {}

#[cfg(target_os = "windows")]
impl Drop for RealtimeMapping {
    fn drop(&mut self) {
        use winapi::um::{handleapi::CloseHandle, memoryapi::UnmapViewOfFile};
        unsafe {
            if !self.block.is_null() {
                UnmapViewOfFile(self.block.cast());
            }
            CloseHandle(self.request_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.response_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.editor_action_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.fault_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.mapping as winapi::shared::ntdef::HANDLE);
        }
    }
}

/// Host-side shared-memory endpoint. One value belongs to one isolated plug-in instance.
pub struct RealtimeClient {
    descriptor: RealtimeDescriptor,
    #[cfg(target_os = "windows")]
    shared: Arc<RealtimeMapping>,
    sequence: u64,
    sample_rate: f64,
    faulted: Arc<AtomicBool>,
    deadline_misses: Arc<AtomicU64>,
    pending_midi: [RealtimeMidiMessage; MAX_REALTIME_MIDI_EVENTS],
    pending_midi_count: usize,
    pending_parameters: [RealtimeParameterChange; MAX_REALTIME_PARAMETER_EVENTS],
    pending_parameter_count: usize,
}

/// Lock-free control-side reader for native editor toolbar actions.
///
/// Actions share only one atomic word with the realtime mapping; taking them neither acquires
/// the plug-in control mutex nor waits for the helper GUI loop.
pub struct EditorActionReader {
    #[cfg(target_os = "windows")]
    shared: Arc<RealtimeMapping>,
}

/// Event-driven control-side notification for a realtime transport failure.
pub struct RealtimeFaultReader {
    #[cfg(target_os = "windows")]
    shared: std::sync::Weak<RealtimeMapping>,
}

unsafe impl Send for RealtimeFaultReader {}

impl RealtimeFaultReader {
    /// Returns true when a fault event arrived. Timeout is only a lifecycle check interval.
    pub fn wait(&self, timeout: std::time::Duration) -> bool {
        #[cfg(target_os = "windows")]
        {
            use winapi::um::{synchapi::WaitForSingleObject, winbase::WAIT_OBJECT_0};
            let Some(shared) = self.shared.upgrade() else {
                return false;
            };
            let timeout_ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
            return unsafe {
                WaitForSingleObject(
                    shared.fault_event as winapi::shared::ntdef::HANDLE,
                    timeout_ms,
                ) == WAIT_OBJECT_0
            };
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::thread::sleep(timeout);
            false
        }
    }

    /// Whether the audio-side endpoint still owns the shared event mapping.
    pub fn alive(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.shared.strong_count() > 0
        }
        #[cfg(not(target_os = "windows"))]
        false
    }
}

unsafe impl Send for EditorActionReader {}
unsafe impl Sync for EditorActionReader {}

impl EditorActionReader {
    /// Atomically drain all pending helper chrome actions.
    pub fn take(&self) -> Vec<crate::process_isolation::EditorHostAction> {
        #[cfg(target_os = "windows")]
        {
            let bits = unsafe {
                (*self.shared.block)
                    .editor_action_bits
                    .swap(0, AtomicOrdering::AcqRel)
            };
            return decode_editor_actions(bits);
        }
        #[cfg(not(target_os = "windows"))]
        Vec::new()
    }

    /// Sleep in the kernel until the helper publishes an action or the caller's lifecycle check
    /// interval expires. This is event-driven and consumes no idle CPU.
    pub fn wait_and_take(
        &self,
        timeout: std::time::Duration,
    ) -> Vec<crate::process_isolation::EditorHostAction> {
        #[cfg(target_os = "windows")]
        {
            use winapi::um::synchapi::WaitForSingleObject;
            let timeout_ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
            unsafe {
                WaitForSingleObject(
                    self.shared.editor_action_event as winapi::shared::ntdef::HANDLE,
                    timeout_ms,
                );
            }
            return self.take();
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::thread::sleep(timeout);
            self.take()
        }
    }
}

/// Control-plane half of a realtime endpoint.
///
/// The audio adapter exclusively owns [`RealtimeClient`]. This cloneable handle lets recovery
/// reattach a replacement helper to the same named transport and clear its fault latch without
/// borrowing or locking the audio-side client.
#[derive(Clone)]
pub struct RealtimeRecoveryHandle {
    descriptor: RealtimeDescriptor,
    faulted: Arc<AtomicBool>,
    deadline_misses: Arc<AtomicU64>,
}

impl RealtimeRecoveryHandle {
    /// Descriptor a replacement helper uses to attach to the existing endpoint.
    pub fn descriptor(&self) -> RealtimeDescriptor {
        self.descriptor.clone()
    }

    /// Clear a deadline fault after a replacement helper has attached.
    pub fn reset_after_recovery(&self) {
        self.faulted.store(false, AtomicOrdering::Release);
    }

    /// Whether the audio-side endpoint has latched a processing failure.
    pub fn is_faulted(&self) -> bool {
        self.faulted.load(AtomicOrdering::Acquire)
    }

    /// Number of actual realtime response deadlines missed by this endpoint generation.
    pub fn deadline_misses(&self) -> u64 {
        self.deadline_misses.load(AtomicOrdering::Relaxed)
    }
}

// The endpoint is created on the control owner then moved into the single audio-side adapter.
// It is never concurrently accessed; raw handles point at kernel objects whose lifetime it owns.
unsafe impl Send for RealtimeClient {}

impl RealtimeClient {
    /// Create a lock-free reader for helper-owned editor toolbar actions.
    pub fn editor_action_reader(&self) -> EditorActionReader {
        EditorActionReader {
            #[cfg(target_os = "windows")]
            shared: Arc::clone(&self.shared),
        }
    }
    /// Create an event-driven failure reader for the control/recovery actor.
    pub fn fault_reader(&self) -> RealtimeFaultReader {
        RealtimeFaultReader {
            #[cfg(target_os = "windows")]
            shared: Arc::downgrade(&self.shared),
        }
    }
    /// Create a control-plane recovery handle before moving this client to the audio graph.
    pub fn recovery_handle(&self) -> RealtimeRecoveryHandle {
        RealtimeRecoveryHandle {
            descriptor: self.descriptor.clone(),
            faulted: Arc::clone(&self.faulted),
            deadline_misses: Arc::clone(&self.deadline_misses),
        }
    }
    /// Create and initialize a host-owned mapping and its request/response events.
    #[cfg(target_os = "windows")]
    pub fn create(sample_rate: f64) -> std::result::Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        use winapi::{
            shared::minwindef::FALSE,
            um::{
                handleapi::{CloseHandle, INVALID_HANDLE_VALUE},
                memoryapi::{CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS},
                synchapi::CreateEventW,
                winnt::PAGE_READWRITE,
            },
        };

        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let prefix = format!("Local\\MiniStudioVst3Rt-{}-{id}", std::process::id());
        let descriptor = RealtimeDescriptor {
            mapping_name: format!("{prefix}-map"),
            request_event_name: format!("{prefix}-request"),
            response_event_name: format!("{prefix}-response"),
            editor_action_event_name: format!("{prefix}-editor-action"),
            fault_event_name: format!("{prefix}-fault"),
        };
        let mapping_name = wide(&descriptor.mapping_name);
        let bytes = std::mem::size_of::<RealtimeBlock>() as u64;
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null_mut(),
                PAGE_READWRITE,
                (bytes >> 32) as u32,
                bytes as u32,
                mapping_name.as_ptr(),
            )
        };
        if mapping.is_null() {
            return Err(last_error("CreateFileMappingW"));
        }
        let block = unsafe {
            MapViewOfFile(
                mapping,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<RealtimeBlock>(),
            ) as *mut RealtimeBlock
        };
        if block.is_null() {
            unsafe { CloseHandle(mapping) };
            return Err(last_error("MapViewOfFile"));
        }
        let request_name = wide(&descriptor.request_event_name);
        let response_name = wide(&descriptor.response_event_name);
        let editor_action_name = wide(&descriptor.editor_action_event_name);
        let fault_name = wide(&descriptor.fault_event_name);
        let request_event =
            unsafe { CreateEventW(std::ptr::null_mut(), FALSE, FALSE, request_name.as_ptr()) };
        let response_event =
            unsafe { CreateEventW(std::ptr::null_mut(), FALSE, FALSE, response_name.as_ptr()) };
        let editor_action_event = unsafe {
            CreateEventW(
                std::ptr::null_mut(),
                FALSE,
                FALSE,
                editor_action_name.as_ptr(),
            )
        };
        let fault_event =
            unsafe { CreateEventW(std::ptr::null_mut(), FALSE, FALSE, fault_name.as_ptr()) };
        if request_event.is_null()
            || response_event.is_null()
            || editor_action_event.is_null()
            || fault_event.is_null()
        {
            unsafe {
                if !request_event.is_null() {
                    CloseHandle(request_event);
                }
                if !response_event.is_null() {
                    CloseHandle(response_event);
                }
                if !editor_action_event.is_null() {
                    CloseHandle(editor_action_event);
                }
                if !fault_event.is_null() {
                    CloseHandle(fault_event);
                }
                winapi::um::memoryapi::UnmapViewOfFile(block.cast());
                CloseHandle(mapping);
            }
            return Err(last_error("CreateEventW"));
        }
        unsafe {
            std::ptr::write_bytes(block.cast::<u8>(), 0, std::mem::size_of::<RealtimeBlock>());
            (*block).magic = MAGIC;
            (*block).version = VERSION;
            (*block).state = STATE_IDLE;
        }
        Ok(Self {
            descriptor,
            shared: Arc::new(RealtimeMapping {
                mapping: mapping as usize,
                request_event: request_event as usize,
                response_event: response_event as usize,
                editor_action_event: editor_action_event as usize,
                fault_event: fault_event as usize,
                block,
            }),
            sequence: 0,
            sample_rate,
            faulted: Arc::new(AtomicBool::new(false)),
            deadline_misses: Arc::new(AtomicU64::new(0)),
            pending_midi: [RealtimeMidiMessage::EMPTY; MAX_REALTIME_MIDI_EVENTS],
            pending_midi_count: 0,
            pending_parameters: [RealtimeParameterChange::EMPTY; MAX_REALTIME_PARAMETER_EVENTS],
            pending_parameter_count: 0,
        })
    }

    /// Return an error on platforms whose shared-memory backend is not implemented yet.
    #[cfg(not(target_os = "windows"))]
    pub fn create(_sample_rate: f64) -> std::result::Result<Self, String> {
        Err("realtime process transport is currently implemented on Windows only".to_string())
    }

    /// Descriptor sent once over the control protocol during helper attachment.
    pub fn descriptor(&self) -> RealtimeDescriptor {
        self.descriptor.clone()
    }

    /// Queue one short MIDI message for the next audio block without IPC or allocation.
    pub fn queue_midi(&mut self, event: MidiEvent, sample_offset: i32) -> Result<()> {
        if self.pending_midi_count >= MAX_REALTIME_MIDI_EVENTS {
            return Err(Error::MidiError("realtime MIDI block is full".to_string()));
        }
        let Some((status, data1, data2)) = encode_midi(event) else {
            return Err(Error::MidiError(
                "unsupported realtime MIDI event".to_string(),
            ));
        };
        self.pending_midi[self.pending_midi_count] = RealtimeMidiMessage {
            sample_offset: sample_offset.max(0) as u32,
            status,
            data1,
            data2,
            _padding: 0,
        };
        self.pending_midi_count += 1;
        Ok(())
    }

    /// Queue one normalized parameter value for the next audio block.
    pub fn queue_parameter(&mut self, id: u32, value: f64, sample_offset: i32) -> Result<()> {
        if self.pending_parameter_count >= MAX_REALTIME_PARAMETER_EVENTS {
            return Err(Error::ProcessError(
                "realtime parameter block is full".to_string(),
            ));
        }
        self.pending_parameters[self.pending_parameter_count] = RealtimeParameterChange {
            id,
            sample_offset: sample_offset.max(0) as u32,
            value_bits: value.to_bits(),
        };
        self.pending_parameter_count += 1;
        Ok(())
    }

    /// Process one bus-preserving block through the helper audio worker.
    #[cfg(target_os = "windows")]
    pub fn process_buses(&mut self, buffers: &mut BusAudioBuffers) -> Result<()> {
        use std::sync::atomic::{fence, Ordering};
        use winapi::um::{
            synchapi::{SetEvent, WaitForSingleObject},
            winbase::WAIT_OBJECT_0,
        };

        if self.faulted.load(AtomicOrdering::Acquire) {
            silence_outputs(buffers);
            return Err(Error::PluginTimeout);
        }
        let frames = active_frames(buffers);
        let input_channels: usize = buffers.inputs.iter().map(|bus| bus.channels.len()).sum();
        let output_channels: usize = buffers.outputs.iter().map(|bus| bus.channels.len()).sum();
        if frames > MAX_REALTIME_FRAMES
            || buffers.inputs.len() > MAX_REALTIME_BUSES
            || buffers.outputs.len() > MAX_REALTIME_BUSES
            || input_channels > MAX_REALTIME_CHANNELS
            || output_channels > MAX_REALTIME_CHANNELS
        {
            silence_outputs(buffers);
            return Err(Error::ProcessError(
                "realtime bus shape exceeds shared-memory limits".to_string(),
            ));
        }

        let block = unsafe { &mut *self.shared.block };
        if block.magic != MAGIC || block.version != VERSION || block.state != STATE_IDLE {
            self.faulted.store(true, AtomicOrdering::Release);
            self.signal_fault();
            silence_outputs(buffers);
            return Err(Error::ProcessError(
                "realtime shared-memory state is invalid".to_string(),
            ));
        }
        block.frames = frames as u32;
        block.input_bus_count = buffers.inputs.len() as u16;
        block.output_bus_count = buffers.outputs.len() as u16;
        block.input_channel_count = input_channels as u16;
        block.output_channel_count = output_channels as u16;
        block.midi_count = self.pending_midi_count as u16;
        block.parameter_count = self.pending_parameter_count as u16;
        block.midi[..self.pending_midi_count]
            .copy_from_slice(&self.pending_midi[..self.pending_midi_count]);
        block.parameters[..self.pending_parameter_count]
            .copy_from_slice(&self.pending_parameters[..self.pending_parameter_count]);

        let mut channel_index = 0usize;
        for (bus_index, bus) in buffers.inputs.iter().enumerate() {
            block.input_bus_channels[bus_index] = bus.channels.len() as u16;
            block.input_bus_active[bus_index] = u8::from(bus.active);
            for channel in &bus.channels {
                let destination = &mut block.input_samples[channel_index * MAX_REALTIME_FRAMES
                    ..(channel_index + 1) * MAX_REALTIME_FRAMES];
                let count = frames.min(channel.len());
                destination[..count].copy_from_slice(&channel[..count]);
                destination[count..frames].fill(0.0);
                channel_index += 1;
            }
        }
        for (bus_index, bus) in buffers.outputs.iter().enumerate() {
            block.output_bus_channels[bus_index] = bus.channels.len() as u16;
            block.output_bus_active[bus_index] = u8::from(bus.active);
        }
        self.pending_midi_count = 0;
        self.pending_parameter_count = 0;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        block.request_sequence = self.sequence;
        block.status = STATUS_OK;
        fence(Ordering::Release);
        block.state = STATE_REQUESTED;
        let request = self.shared.request_event as winapi::shared::ntdef::HANDLE;
        if unsafe { SetEvent(request) } == 0 {
            self.faulted.store(true, AtomicOrdering::Release);
            self.signal_fault();
            silence_outputs(buffers);
            return Err(Error::ProcessError(last_error("SetEvent(request)")));
        }
        let response = self.shared.response_event as winapi::shared::ntdef::HANDLE;
        let block_ms = (frames as f64 * 1000.0 / self.sample_rate.max(1.0)).ceil() as u32;
        let deadline_ms = block_ms.saturating_mul(4).clamp(5, 50);
        match unsafe { WaitForSingleObject(response, deadline_ms) } {
            WAIT_OBJECT_0 => {}
            258 => {
                self.deadline_misses.fetch_add(1, AtomicOrdering::Relaxed);
                self.faulted.store(true, AtomicOrdering::Release);
                self.signal_fault();
                silence_outputs(buffers);
                return Err(Error::PluginTimeout);
            }
            _ => {
                self.faulted.store(true, AtomicOrdering::Release);
                self.signal_fault();
                silence_outputs(buffers);
                return Err(Error::ProcessError(last_error(
                    "WaitForSingleObject(response)",
                )));
            }
        }
        fence(Ordering::Acquire);
        if block.response_sequence != self.sequence || block.state != STATE_DONE {
            self.faulted.store(true, AtomicOrdering::Release);
            self.signal_fault();
            silence_outputs(buffers);
            return Err(Error::ProcessError(
                "isolated realtime response sequence mismatch".to_string(),
            ));
        }
        if block.status != STATUS_OK {
            block.state = STATE_IDLE;
            silence_outputs(buffers);
            return Err(Error::ProcessError(format!(
                "isolated realtime worker returned status {}",
                block.status
            )));
        }
        let mut channel_index = 0usize;
        for bus in &mut buffers.outputs {
            for channel in &mut bus.channels {
                let source = &block.output_samples[channel_index * MAX_REALTIME_FRAMES
                    ..(channel_index + 1) * MAX_REALTIME_FRAMES];
                let count = frames.min(channel.len());
                channel[..count].copy_from_slice(&source[..count]);
                channel[count..].fill(0.0);
                channel_index += 1;
            }
        }
        block.state = STATE_IDLE;
        Ok(())
    }

    /// Non-Windows fallback used only to keep cross-platform feature builds explicit.
    #[cfg(not(target_os = "windows"))]
    pub fn process_buses(&mut self, buffers: &mut BusAudioBuffers) -> Result<()> {
        silence_outputs(buffers);
        Err(Error::ProcessError(
            "realtime process transport is unavailable on this platform".to_string(),
        ))
    }

    /// Clear a latched timeout after a helper has been replaced and reattached.
    pub fn reset_after_recovery(&self) {
        self.faulted.store(false, AtomicOrdering::Release);
        #[cfg(target_os = "windows")]
        unsafe {
            (*self.shared.block).state = STATE_IDLE;
            (*self.shared.block).status = STATUS_OK;
        }
    }

    /// Whether a block timeout or protocol mismatch has latched this endpoint offline.
    pub fn is_faulted(&self) -> bool {
        self.faulted.load(AtomicOrdering::Acquire)
    }

    /// Update the deadline calculation after the plug-in is reconfigured.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sample_rate = sample_rate;
        }
    }

    #[cfg(target_os = "windows")]
    fn signal_fault(&self) {
        unsafe {
            winapi::um::synchapi::SetEvent(
                self.shared.fault_event as winapi::shared::ntdef::HANDLE,
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn signal_fault(&self) {}
}

/// Helper-side audio worker and mapped transport lifetime.
pub struct RealtimeServer {
    #[cfg(target_os = "windows")]
    block: usize,
    #[cfg(target_os = "windows")]
    request_event: usize,
    #[cfg(target_os = "windows")]
    response_event: usize,
    #[cfg(target_os = "windows")]
    editor_action_event: usize,
    #[cfg(target_os = "windows")]
    mapping: usize,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl RealtimeServer {
    /// Publish helper-owned toolbar actions without using the JSON control request loop.
    pub fn publish_editor_actions(
        &self,
        actions: &[crate::process_isolation::EditorHostAction],
    ) {
        #[cfg(target_os = "windows")]
        if !actions.is_empty() {
            let bits = encode_editor_actions(actions);
            unsafe {
                (*(self.block as *mut RealtimeBlock))
                    .editor_action_bits
                    .fetch_or(bits, AtomicOrdering::Release);
                winapi::um::synchapi::SetEvent(
                    self.editor_action_event as winapi::shared::ntdef::HANDLE,
                );
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = actions;
    }
    /// Open the host-created objects, preallocate bus buffers, and start the helper audio worker.
    #[cfg(target_os = "windows")]
    pub fn attach(
        descriptor: &RealtimeDescriptor,
        plugin: Arc<Mutex<Plugin>>,
    ) -> std::result::Result<Self, String> {
        use winapi::{
            shared::minwindef::FALSE,
            um::{
                memoryapi::{MapViewOfFile, OpenFileMappingW, FILE_MAP_ALL_ACCESS},
                synchapi::OpenEventW,
                winnt::{EVENT_MODIFY_STATE, SYNCHRONIZE},
            },
        };
        let mut buffers = plugin
            .lock()
            .map_err(|_| "plugin instance lock poisoned".to_string())?
            .create_bus_audio_buffers(MAX_REALTIME_FRAMES)
            .map_err(|error| error.to_string())?;
        let mapping_name = wide(&descriptor.mapping_name);
        let mapping =
            unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, mapping_name.as_ptr()) };
        if mapping.is_null() {
            return Err(last_error("OpenFileMappingW"));
        }
        let block = unsafe {
            MapViewOfFile(
                mapping,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<RealtimeBlock>(),
            ) as *mut RealtimeBlock
        };
        if block.is_null() {
            unsafe { winapi::um::handleapi::CloseHandle(mapping) };
            return Err(last_error("MapViewOfFile(helper)"));
        }
        if unsafe { (*block).magic } != MAGIC || unsafe { (*block).version } != VERSION {
            unsafe {
                winapi::um::memoryapi::UnmapViewOfFile(block.cast());
                winapi::um::handleapi::CloseHandle(mapping);
            }
            return Err("realtime mapping version mismatch".to_string());
        }
        let request_name = wide(&descriptor.request_event_name);
        let response_name = wide(&descriptor.response_event_name);
        let editor_action_name = wide(&descriptor.editor_action_event_name);
        let access = EVENT_MODIFY_STATE | SYNCHRONIZE;
        let request_event = unsafe { OpenEventW(access, FALSE, request_name.as_ptr()) };
        let response_event = unsafe { OpenEventW(access, FALSE, response_name.as_ptr()) };
        let editor_action_event = unsafe { OpenEventW(access, FALSE, editor_action_name.as_ptr()) };
        if request_event.is_null() || response_event.is_null() || editor_action_event.is_null() {
            unsafe {
                if !request_event.is_null() {
                    winapi::um::handleapi::CloseHandle(request_event);
                }
                if !response_event.is_null() {
                    winapi::um::handleapi::CloseHandle(response_event);
                }
                if !editor_action_event.is_null() {
                    winapi::um::handleapi::CloseHandle(editor_action_event);
                }
                winapi::um::memoryapi::UnmapViewOfFile(block.cast());
                winapi::um::handleapi::CloseHandle(mapping);
            }
            return Err(last_error("OpenEventW"));
        }
        let block_address = block as usize;
        let request_address = request_event as usize;
        let response_address = response_event as usize;
        let worker = std::thread::Builder::new()
            .name("vst3-helper-audio".to_string())
            .spawn(move || {
                run_server_loop(
                    block_address,
                    request_address,
                    response_address,
                    plugin,
                    &mut buffers,
                );
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            block: block_address,
            request_event: request_address,
            response_event: response_address,
            editor_action_event: editor_action_event as usize,
            mapping: mapping as usize,
            worker: Some(worker),
        })
    }

    /// Non-Windows fallback.
    #[cfg(not(target_os = "windows"))]
    pub fn attach(
        _descriptor: &RealtimeDescriptor,
        _plugin: Arc<Mutex<Plugin>>,
    ) -> std::result::Result<Self, String> {
        Err("realtime process transport is currently implemented on Windows only".to_string())
    }
}

#[cfg(target_os = "windows")]
impl Drop for RealtimeServer {
    fn drop(&mut self) {
        use winapi::um::{handleapi::CloseHandle, memoryapi::UnmapViewOfFile, synchapi::SetEvent};
        unsafe {
            let block = self.block as *mut RealtimeBlock;
            (*block).state = STATE_SHUTDOWN;
            SetEvent(self.request_event as winapi::shared::ntdef::HANDLE);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        unsafe {
            UnmapViewOfFile((self.block as *mut RealtimeBlock).cast());
            CloseHandle(self.request_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.response_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.editor_action_event as winapi::shared::ntdef::HANDLE);
            CloseHandle(self.mapping as winapi::shared::ntdef::HANDLE);
        }
    }
}

#[cfg(target_os = "windows")]
fn run_server_loop(
    block_address: usize,
    request_address: usize,
    response_address: usize,
    plugin: Arc<Mutex<Plugin>>,
    buffers: &mut BusAudioBuffers,
) {
    use std::sync::atomic::{fence, Ordering};
    use winapi::um::{
        synchapi::{SetEvent, WaitForSingleObject},
        winbase::{INFINITE, WAIT_OBJECT_0},
    };
    let block = block_address as *mut RealtimeBlock;
    let request = request_address as winapi::shared::ntdef::HANDLE;
    let response = response_address as winapi::shared::ntdef::HANDLE;
    loop {
        if unsafe { WaitForSingleObject(request, INFINITE) } != WAIT_OBJECT_0 {
            break;
        }
        fence(Ordering::Acquire);
        let shared = unsafe { &mut *block };
        if shared.state == STATE_SHUTDOWN {
            break;
        }
        if shared.state != STATE_REQUESTED {
            continue;
        }
        shared.state = STATE_PROCESSING;
        let sequence = shared.request_sequence;
        let frames = shared.frames as usize;
        let valid = frames <= MAX_REALTIME_FRAMES
            && shared.input_bus_count as usize <= buffers.inputs.len()
            && shared.output_bus_count as usize <= buffers.outputs.len()
            && shared.input_channel_count as usize <= MAX_REALTIME_CHANNELS
            && shared.output_channel_count as usize <= MAX_REALTIME_CHANNELS
            && shared.midi_count as usize <= MAX_REALTIME_MIDI_EVENTS
            && shared.parameter_count as usize <= MAX_REALTIME_PARAMETER_EVENTS;
        if !valid {
            shared.status = STATUS_BAD_SHAPE;
        } else {
            prepare_server_buffers(shared, buffers, frames);
            let processed = plugin.lock().map_err(|_| ()).and_then(|mut plugin| {
                for event in &shared.midi[..shared.midi_count as usize] {
                    if let Some(midi) = decode_midi(*event) {
                        let _ = plugin.send_midi_event_at(midi, event.sample_offset as i32);
                    }
                }
                for parameter in &shared.parameters[..shared.parameter_count as usize] {
                    let _ = plugin.set_parameter_at(
                        parameter.id,
                        f64::from_bits(parameter.value_bits),
                        parameter.sample_offset as i32,
                    );
                }
                plugin.process_bus_audio(buffers).map_err(|_| ())?;
                Ok(())
            });
            if processed.is_ok() {
                copy_server_outputs(shared, buffers, frames);
                shared.status = STATUS_OK;
            } else {
                shared.status = STATUS_PLUGIN_ERROR;
            }
        }
        shared.response_sequence = sequence;
        fence(Ordering::Release);
        shared.state = STATE_DONE;
        unsafe {
            SetEvent(response);
        }
    }
}

#[cfg(target_os = "windows")]
fn prepare_server_buffers(shared: &RealtimeBlock, buffers: &mut BusAudioBuffers, frames: usize) {
    let mut channel_index = 0usize;
    for (bus_index, bus) in buffers.inputs.iter_mut().enumerate() {
        bus.active =
            bus_index < shared.input_bus_count as usize && shared.input_bus_active[bus_index] != 0;
        for channel in &mut bus.channels {
            channel.resize(MAX_REALTIME_FRAMES, 0.0);
            channel.truncate(frames);
            if channel_index < shared.input_channel_count as usize {
                let source = &shared.input_samples[channel_index * MAX_REALTIME_FRAMES
                    ..(channel_index + 1) * MAX_REALTIME_FRAMES];
                channel.copy_from_slice(&source[..frames]);
            } else {
                channel.fill(0.0);
            }
            channel_index += 1;
        }
    }
    for (bus_index, bus) in buffers.outputs.iter_mut().enumerate() {
        bus.active = bus_index < shared.output_bus_count as usize
            && shared.output_bus_active[bus_index] != 0;
        for channel in &mut bus.channels {
            channel.resize(MAX_REALTIME_FRAMES, 0.0);
            channel.truncate(frames);
            channel.fill(0.0);
        }
    }
    buffers.block_size = frames;
}

#[cfg(target_os = "windows")]
fn copy_server_outputs(shared: &mut RealtimeBlock, buffers: &BusAudioBuffers, frames: usize) {
    let mut channel_index = 0usize;
    for bus in &buffers.outputs {
        for channel in &bus.channels {
            if channel_index >= shared.output_channel_count as usize {
                return;
            }
            let destination = &mut shared.output_samples
                [channel_index * MAX_REALTIME_FRAMES..(channel_index + 1) * MAX_REALTIME_FRAMES];
            let count = frames.min(channel.len());
            destination[..count].copy_from_slice(&channel[..count]);
            destination[count..frames].fill(0.0);
            channel_index += 1;
        }
    }
}

fn active_frames(buffers: &BusAudioBuffers) -> usize {
    buffers
        .outputs
        .iter()
        .chain(&buffers.inputs)
        .flat_map(|bus| &bus.channels)
        .map(Vec::len)
        .next()
        .unwrap_or(buffers.block_size)
}

fn silence_outputs(buffers: &mut BusAudioBuffers) {
    for channel in buffers.outputs.iter_mut().flat_map(|bus| &mut bus.channels) {
        channel.fill(0.0);
    }
}

#[allow(unreachable_patterns)]
fn encode_midi(event: MidiEvent) -> Option<(u8, u8, u8)> {
    Some(match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => (0x90 | channel.as_index(), note, velocity),
        MidiEvent::NoteOff {
            channel,
            note,
            velocity,
        } => (0x80 | channel.as_index(), note, velocity),
        MidiEvent::ControlChange {
            channel,
            controller,
            value,
        } => (0xb0 | channel.as_index(), controller, value),
        MidiEvent::ProgramChange { channel, program } => (0xc0 | channel.as_index(), program, 0),
        MidiEvent::PitchBend { channel, value } => (
            0xe0 | channel.as_index(),
            (value & 0x7f) as u8,
            ((value >> 7) & 0x7f) as u8,
        ),
        MidiEvent::ChannelAftertouch { channel, pressure } => {
            (0xd0 | channel.as_index(), pressure, 0)
        }
        MidiEvent::PolyAftertouch {
            channel,
            note,
            pressure,
        } => (0xa0 | channel.as_index(), note, pressure),
        _ => return None,
    })
}

fn decode_midi(event: RealtimeMidiMessage) -> Option<MidiEvent> {
    let channel = MidiChannel::from_index(event.status & 0x0f)?;
    Some(match event.status & 0xf0 {
        0x80 => MidiEvent::NoteOff {
            channel,
            note: event.data1,
            velocity: event.data2,
        },
        0x90 => MidiEvent::NoteOn {
            channel,
            note: event.data1,
            velocity: event.data2,
        },
        0xa0 => MidiEvent::PolyAftertouch {
            channel,
            note: event.data1,
            pressure: event.data2,
        },
        0xb0 => MidiEvent::ControlChange {
            channel,
            controller: event.data1,
            value: event.data2,
        },
        0xc0 => MidiEvent::ProgramChange {
            channel,
            program: event.data1,
        },
        0xd0 => MidiEvent::ChannelAftertouch {
            channel,
            pressure: event.data1,
        },
        0xe0 => MidiEvent::PitchBend {
            channel,
            value: u16::from(event.data1) | (u16::from(event.data2) << 7),
        },
        _ => return None,
    })
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn last_error(operation: &str) -> String {
    format!("{operation} failed: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_midi_round_trips_without_allocation_or_pitch_changes() {
        let messages = [
            MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 60,
                velocity: 101,
            },
            MidiEvent::NoteOff {
                channel: MidiChannel::Ch16,
                note: 127,
                velocity: 12,
            },
            MidiEvent::PitchBend {
                channel: MidiChannel::Ch3,
                value: 12_345,
            },
        ];
        for message in messages {
            let (status, data1, data2) = encode_midi(message).expect("encodable MIDI");
            let decoded = decode_midi(RealtimeMidiMessage {
                sample_offset: 17,
                status,
                data1,
                data2,
                _padding: 0,
            });
            assert_eq!(decoded, Some(message));
        }
    }

    #[test]
    fn shared_block_stays_within_the_intended_fixed_capacity() {
        assert!(std::mem::size_of::<RealtimeBlock>() < 3 * 1024 * 1024);
        assert_eq!(std::mem::align_of::<RealtimeBlock>(), 8);
    }
}
