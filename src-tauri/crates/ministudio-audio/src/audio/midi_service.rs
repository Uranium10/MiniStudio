//! MIDI device lifetime and routing service.
//!
//! The service owns every physical connection and routes callbacks through an atomic graph index.
//! OS discovery/open/close operations are exposed as detached steps so callers never hold the
//! engine control lock while entering a platform MIDI service.

use super::{
    instrument::{LiveMidiMessage, NoteEvent, NoteEventKind},
    types::MidiInputPortInfo,
};
use crossbeam_queue::ArrayQueue;
use midir::{Ignore, MidiInput, MidiInputConnection};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI32, AtomicUsize, Ordering},
        Arc,
    },
};

const NO_MIDI_TRACK: usize = usize::MAX;
const LIVE_MIDI_CAPACITY: usize = 2048;

struct MidiInputRoute {
    _connection: Option<MidiInputConnection<()>>,
    target_track_id: String,
    target_index: Arc<AtomicUsize>,
}

/// Result of resolving a requested route without entering the operating-system MIDI service.
pub enum MidiConnectPlan {
    /// An existing connection was atomically retargeted.
    Routed,
    /// A physical connection must be opened outside the engine owner.
    Open(MidiInputRouteRequest),
}

/// Owned inputs required to open one physical MIDI connection off the engine owner.
pub struct MidiInputRouteRequest {
    port_id: String,
    target_track_id: String,
    target_index: Arc<AtomicUsize>,
    queue: Arc<ArrayQueue<LiveMidiMessage>>,
    counter: Arc<AtomicI32>,
}

/// Physical route opened outside the service and ready for a short install transaction.
pub struct OpenedMidiInputRoute {
    port_id: String,
    route: MidiInputRoute,
}

impl OpenedMidiInputRoute {
    /// Project track ID requested when the OS connection was opened.
    pub fn target_track_id(&self) -> &str {
        &self.route.target_track_id
    }
}

/// Route detached under service ownership and safe to drop on a blocking worker.
pub struct DetachedMidiInputRoute(MidiInputRoute);

/// Sole owner of physical MIDI connections, route targets and live packet ingress.
pub struct MidiService {
    input: Arc<ArrayQueue<LiveMidiMessage>>,
    routes: HashMap<String, MidiInputRoute>,
    note_counter: Arc<AtomicI32>,
}

impl Default for MidiService {
    fn default() -> Self {
        Self {
            input: Arc::new(ArrayQueue::new(LIVE_MIDI_CAPACITY)),
            routes: HashMap::new(),
            note_counter: Arc::new(AtomicI32::new(1)),
        }
    }
}

impl MidiService {
    /// Realtime consumer endpoint shared with `AudioCore`.
    pub fn ingress(&self) -> Arc<ArrayQueue<LiveMidiMessage>> {
        Arc::clone(&self.input)
    }

    /// Disconnect every physical route. The returned handles should be dropped off the owner.
    pub fn detach_all(&mut self) -> Vec<DetachedMidiInputRoute> {
        let queue = Arc::clone(&self.input);
        let routes = self
            .routes
            .drain()
            .map(move |(_, route)| {
                queue_all_notes_off(&queue, route.target_index.load(Ordering::Acquire));
                DetachedMidiInputRoute(route)
            })
            .collect();
        routes
    }

    /// Cheap immutable route metadata for a detached OS enumeration.
    pub fn route_snapshot(&self) -> HashMap<String, String> {
        self.routes
            .iter()
            .map(|(port_id, route)| (port_id.clone(), route.target_track_id.clone()))
            .collect()
    }

    /// Enumerate ports. Call on a disposable probe/blocking worker, never under engine ownership.
    pub fn scan_inputs(
        connected_routes: &HashMap<String, String>,
    ) -> Result<Vec<MidiInputPortInfo>, String> {
        let input = MidiInput::new("MiniStudio MIDI scan").map_err(|error| error.to_string())?;
        input
            .ports()
            .iter()
            .enumerate()
            .map(|(index, port)| {
                let id = index.to_string();
                Ok(MidiInputPortInfo {
                    name: input.port_name(port).map_err(|error| error.to_string())?,
                    connected: connected_routes.contains_key(&id),
                    target_track_id: connected_routes.get(&id).cloned(),
                    id,
                })
            })
            .collect()
    }

    /// Resolve or atomically retarget a route without invoking the operating system.
    pub fn prepare_input(
        &mut self,
        port_id: &str,
        track_id: &str,
        track_index: usize,
    ) -> MidiConnectPlan {
        if let Some(route) = self.routes.get_mut(port_id) {
            let previous = route.target_index.load(Ordering::Acquire);
            if route.target_track_id == track_id && previous == track_index {
                return MidiConnectPlan::Routed;
            }
            // Release the old graph destination before publishing the new atomic route. A held
            // key whose later NoteOff arrives after retargeting can otherwise stick forever.
            if previous != NO_MIDI_TRACK {
                let _ = self.input.push(LiveMidiMessage {
                    track: previous,
                    event: all_notes_off(),
                });
            }
            route.target_track_id = track_id.to_owned();
            route.target_index.store(track_index, Ordering::Release);
            return MidiConnectPlan::Routed;
        }
        MidiConnectPlan::Open(MidiInputRouteRequest {
            port_id: port_id.to_owned(),
            target_track_id: track_id.to_owned(),
            target_index: Arc::new(AtomicUsize::new(track_index)),
            queue: Arc::clone(&self.input),
            counter: Arc::clone(&self.note_counter),
        })
    }

    /// Open a physical route from a detached request.
    pub fn open_input(request: MidiInputRouteRequest) -> Result<OpenedMidiInputRoute, String> {
        let mut input =
            MidiInput::new("MiniStudio MIDI input").map_err(|error| error.to_string())?;
        input.ignore(Ignore::None);
        let port_index = request
            .port_id
            .parse::<usize>()
            .map_err(|_| "invalid MIDI port")?;
        let port = input
            .ports()
            .get(port_index)
            .cloned()
            .ok_or("MIDI port is unavailable")?;
        let callback_target = Arc::clone(&request.target_index);
        let initial_track = callback_target.load(Ordering::Acquire);
        let mut note_ids = [[-1_i32; 8]; 128];
        let mut note_counts = [0_usize; 128];
        let mut last_track = initial_track;
        let connection = input
            .connect(
                &port,
                "MiniStudio",
                move |_stamp, message, _| {
                    if message.is_empty() {
                        return;
                    }
                    let track = callback_target.load(Ordering::Acquire);
                    if track == NO_MIDI_TRACK {
                        return;
                    }
                    if track != last_track {
                        note_ids = [[-1_i32; 8]; 128];
                        note_counts = [0_usize; 128];
                        last_track = track;
                    }
                    let status = message[0] & 0xf0;
                    let pitch = message.get(1).copied().unwrap_or(0).min(127);
                    let value = message.get(2).copied().unwrap_or(0);
                    let kind = match status {
                        0x90 if value > 0 => {
                            let id = request.counter.fetch_add(1, Ordering::Relaxed);
                            let count = &mut note_counts[pitch as usize];
                            if *count < 8 {
                                note_ids[pitch as usize][*count] = id;
                                *count += 1;
                            }
                            NoteEventKind::NoteOn {
                                note_id: id,
                                pitch,
                                velocity: f32::from(value) / 127.0,
                                tuning_cents: 0.0,
                            }
                        }
                        0x80 | 0x90 => {
                            let count = &mut note_counts[pitch as usize];
                            let id = if *count > 0 {
                                *count -= 1;
                                note_ids[pitch as usize][*count]
                            } else {
                                -1
                            };
                            NoteEventKind::NoteOff {
                                note_id: id,
                                pitch,
                                velocity: f32::from(value) / 127.0,
                            }
                        }
                        0xb0 => NoteEventKind::Controller {
                            cc: pitch,
                            value: f32::from(value) / 127.0,
                        },
                        0xe0 => {
                            let bend = (u16::from(value) << 7) | u16::from(pitch);
                            NoteEventKind::PitchBend {
                                value: (f32::from(bend) - 8192.0) / 8192.0,
                            }
                        }
                        _ => return,
                    };
                    let _ = request.queue.push(LiveMidiMessage {
                        track,
                        event: NoteEvent {
                            sample_offset: 0,
                            kind,
                        },
                    });
                },
                (),
            )
            .map_err(|error| error.to_string())?;
        Ok(OpenedMidiInputRoute {
            port_id: request.port_id,
            route: MidiInputRoute {
                _connection: Some(connection),
                target_track_id: request.target_track_id,
                target_index: request.target_index,
            },
        })
    }

    /// Install a previously opened route and return any redundant connection for detached drop.
    pub fn install_input(
        &mut self,
        opened: OpenedMidiInputRoute,
        current_track_index: Option<usize>,
    ) -> Option<DetachedMidiInputRoute> {
        let current = current_track_index.unwrap_or(NO_MIDI_TRACK);
        opened.route.target_index.store(current, Ordering::Release);
        let queue = Arc::clone(&self.input);
        if let Some(existing) = self.routes.get_mut(&opened.port_id) {
            let previous = existing.target_index.load(Ordering::Acquire);
            queue_all_notes_off(&queue, previous);
            existing.target_track_id = opened.route.target_track_id.clone();
            existing.target_index.store(current, Ordering::Release);
            return Some(DetachedMidiInputRoute(opened.route));
        }
        self.routes.insert(opened.port_id, opened.route);
        None
    }

    /// Detach one route and release any held notes on its former destination.
    pub fn detach_input(&mut self, port_id: &str) -> Option<DetachedMidiInputRoute> {
        let route = self.routes.remove(port_id)?;
        self.queue_all_notes_off(route.target_index.load(Ordering::Acquire));
        Some(DetachedMidiInputRoute(route))
    }

    /// Drop a detached OS connection outside engine ownership.
    pub fn release_input(route: DetachedMidiInputRoute) {
        drop(route.0);
    }

    /// Atomically refresh every route after a graph replacement.
    pub fn refresh_routes(&mut self, tracks: &HashMap<String, usize>) {
        let queue = Arc::clone(&self.input);
        for route in self.routes.values_mut() {
            let previous = route.target_index.load(Ordering::Acquire);
            let target = tracks
                .get(&route.target_track_id)
                .copied()
                .unwrap_or(NO_MIDI_TRACK);
            if previous != target && previous != NO_MIDI_TRACK {
                let _ = queue.push(LiveMidiMessage {
                    track: previous,
                    event: all_notes_off(),
                });
            }
            route.target_index.store(target, Ordering::Release);
        }
    }

    fn queue_all_notes_off(&self, track: usize) {
        queue_all_notes_off(&self.input, track);
    }
}

fn queue_all_notes_off(queue: &ArrayQueue<LiveMidiMessage>, track: usize) {
    if track == NO_MIDI_TRACK {
        return;
    }
    let _ = queue.push(LiveMidiMessage {
        track,
        event: all_notes_off(),
    });
}

fn all_notes_off() -> NoteEvent {
    NoteEvent {
        sample_offset: 0,
        kind: NoteEventKind::AllNotesOff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_releases_the_previous_track_before_publishing_the_new_one() {
        let mut service = MidiService::default();
        let target = Arc::new(AtomicUsize::new(2));
        service.routes.insert(
            "port".into(),
            MidiInputRoute {
                _connection: None,
                target_track_id: "old".into(),
                target_index: Arc::clone(&target),
            },
        );
        assert!(matches!(
            service.prepare_input("port", "new", 7),
            MidiConnectPlan::Routed
        ));
        let release = service.input.pop().expect("old route release");
        assert_eq!(release.track, 2);
        assert!(matches!(release.event.kind, NoteEventKind::AllNotesOff));
        assert_eq!(target.load(Ordering::Acquire), 7);
    }
}
