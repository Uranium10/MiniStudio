//! Format-neutral plug-in lifecycle supervisor.

use std::collections::HashMap;

use ministudio_plugin_api::{
    EditorLifecycle, EditorSession, Generation, PluginEvent, PluginInstanceId, RuntimeHealth,
};

/// Pure state-machine owner for native editor sessions.
///
/// Platform calls and IPC live outside this type. Keeping transitions pure makes stale-reply,
/// pinning and foreground policy deterministic and testable.
#[derive(Default)]
pub struct PluginSupervisor {
    sessions: HashMap<PluginInstanceId, EditorSession>,
    next_generation: Generation,
}

impl PluginSupervisor {
    /// Begin opening an editor and return the new generation.
    pub fn begin_open(
        &mut self,
        instance_id: PluginInstanceId,
        pinned: bool,
        foreground: bool,
    ) -> Generation {
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let generation = self.next_generation;
        self.sessions.insert(
            instance_id.clone(),
            EditorSession {
                instance_id,
                generation,
                lifecycle: EditorLifecycle::Opening,
                pinned,
                foreground,
                health: RuntimeHealth {
                    realtime_healthy: true,
                    control_healthy: true,
                    editor_healthy: true,
                },
            },
        );
        generation
    }

    /// Mark an editor close as pending without removing generation protection.
    pub fn begin_close(&mut self, instance_id: &PluginInstanceId) -> Option<Generation> {
        let session = self.sessions.get_mut(instance_id)?;
        session.lifecycle = EditorLifecycle::Closing;
        Some(session.generation)
    }

    /// Apply a helper event only if it belongs to the current generation.
    pub fn apply_event(
        &mut self,
        instance_id: &PluginInstanceId,
        generation: Generation,
        event: PluginEvent,
    ) -> bool {
        let Some(session) = self.sessions.get_mut(instance_id) else {
            return false;
        };
        if session.generation != generation {
            return false;
        }
        match event {
            PluginEvent::EditorLifecycle { lifecycle } => session.lifecycle = lifecycle,
            PluginEvent::PinApplied { pinned } => session.pinned = pinned,
            PluginEvent::EditorUnresponsive => {
                session.lifecycle = EditorLifecycle::Unresponsive;
                session.health.editor_healthy = false;
            }
            PluginEvent::RealtimeDeadlineMiss => session.health.realtime_healthy = false,
            PluginEvent::HelperExited { .. } => {
                session.health.control_healthy = false;
                session.health.realtime_healthy = false;
                session.health.editor_healthy = false;
                session.lifecycle = EditorLifecycle::Closed;
            }
            PluginEvent::EditorAction { .. } => {}
        }
        true
    }

    /// Update desired pin state. Platform acknowledgement arrives as `PinApplied`.
    pub fn set_pin_intent(&mut self, instance_id: &PluginInstanceId, pinned: bool) -> bool {
        let Some(session) = self.sessions.get_mut(instance_id) else {
            return false;
        };
        session.pinned = pinned;
        true
    }

    /// Return unpinned editors superseded by a new foreground request.
    pub fn superseded_unpinned(&self, current: &PluginInstanceId) -> Vec<PluginInstanceId> {
        self.sessions
            .values()
            .filter(|session| {
                &session.instance_id != current
                    && !session.pinned
                    && session.lifecycle != EditorLifecycle::Closed
            })
            .map(|session| session.instance_id.clone())
            .collect()
    }

    /// Forget a retired instance; later stale replies are rejected.
    pub fn retire(&mut self, instance_id: &PluginInstanceId) -> Option<EditorSession> {
        self.sessions.remove(instance_id)
    }

    /// Retire sessions whose project targets no longer exist after a graph replacement.
    pub fn retain_instances(
        &mut self,
        mut keep: impl FnMut(&PluginInstanceId) -> bool,
    ) -> Vec<EditorSession> {
        let retired = self
            .sessions
            .values()
            .filter(|session| !keep(&session.instance_id))
            .cloned()
            .collect::<Vec<_>>();
        self.sessions.retain(|instance_id, _| keep(instance_id));
        retired
    }

    /// Immutable session snapshot.
    pub fn session(&self, instance_id: &PluginInstanceId) -> Option<&EditorSession> {
        self.sessions.get(instance_id)
    }

    /// Whether an asynchronous result still belongs to the authoritative session.
    pub fn is_current(&self, instance_id: &PluginInstanceId, generation: Generation) -> bool {
        self.sessions
            .get(instance_id)
            .is_some_and(|session| session.generation == generation)
    }

    /// Whether a completed open request is still allowed to publish an editor.
    pub fn accepts_open_completion(
        &self,
        instance_id: &PluginInstanceId,
        generation: Generation,
    ) -> bool {
        self.sessions.get(instance_id).is_some_and(|session| {
            session.generation == generation
                && matches!(
                    session.lifecycle,
                    EditorLifecycle::Opening | EditorLifecycle::Open
                )
        })
    }

    /// Copy a session for control/UI snapshots without exposing the mutable registry.
    pub fn session_snapshot(&self, instance_id: &PluginInstanceId) -> Option<EditorSession> {
        self.sessions.get(instance_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_editor_reply_cannot_resurrect_new_session() {
        let id = PluginInstanceId("instrument-a".into());
        let mut supervisor = PluginSupervisor::default();
        let old = supervisor.begin_open(id.clone(), false, true);
        let new = supervisor.begin_open(id.clone(), false, true);
        assert!(new > old);
        assert!(!supervisor.apply_event(
            &id,
            old,
            PluginEvent::EditorLifecycle {
                lifecycle: EditorLifecycle::Open,
            },
        ));
        assert_eq!(
            supervisor.session(&id).unwrap().lifecycle,
            EditorLifecycle::Opening
        );
    }

    #[test]
    fn pinned_editor_is_not_superseded() {
        let pinned = PluginInstanceId("pinned".into());
        let current = PluginInstanceId("current".into());
        let other = PluginInstanceId("other".into());
        let mut supervisor = PluginSupervisor::default();
        supervisor.begin_open(pinned.clone(), true, false);
        supervisor.begin_open(other.clone(), false, false);
        supervisor.begin_open(current.clone(), false, true);
        assert_eq!(supervisor.superseded_unpinned(&current), vec![other]);
    }

    #[test]
    fn gui_hang_does_not_mark_realtime_unhealthy() {
        let id = PluginInstanceId("instrument-a".into());
        let mut supervisor = PluginSupervisor::default();
        let generation = supervisor.begin_open(id.clone(), false, true);
        assert!(supervisor.apply_event(&id, generation, PluginEvent::EditorUnresponsive));
        let health = supervisor.session(&id).unwrap().health;
        assert!(!health.editor_healthy);
        assert!(health.realtime_healthy);
    }

    #[test]
    fn generation_check_rejects_a_retired_session() {
        let id = PluginInstanceId("instrument-a".into());
        let mut supervisor = PluginSupervisor::default();
        let generation = supervisor.begin_open(id.clone(), false, true);
        assert!(supervisor.is_current(&id, generation));
        supervisor.retire(&id);
        assert!(!supervisor.is_current(&id, generation));
    }

    #[test]
    fn a_close_fence_rejects_a_late_open_completion() {
        let id = PluginInstanceId("instrument-a".into());
        let mut supervisor = PluginSupervisor::default();
        let generation = supervisor.begin_open(id.clone(), false, true);
        supervisor.begin_close(&id);
        assert!(!supervisor.accepts_open_completion(&id, generation));
    }

    #[test]
    fn graph_retirement_invalidates_removed_targets() {
        let keep = PluginInstanceId("keep".into());
        let remove = PluginInstanceId("remove".into());
        let mut supervisor = PluginSupervisor::default();
        supervisor.begin_open(keep.clone(), false, true);
        supervisor.begin_open(remove.clone(), false, false);
        let retired = supervisor.retain_instances(|id| id == &keep);
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].instance_id, remove);
        assert!(supervisor.session(&keep).is_some());
    }
}
