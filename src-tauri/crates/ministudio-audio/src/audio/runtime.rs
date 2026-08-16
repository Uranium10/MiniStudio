//! Allocation-free runtime scheduling policy for audio graph nodes.
//!
//! The graph owns topology and buffers; this module owns only per-node execution
//! state. It has no plug-in-format, project-model, UI, or operating-system types.

use ministudio_dsp::{RuntimeCapabilities, RuntimeTail};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeState {
    #[default]
    Running,
    Tail,
    Sleeping,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SleepPolicy {
    #[default]
    Auto,
    AlwaysProcess,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcessDecision {
    #[default]
    Process,
    SkipAndClear,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockActivity {
    pub main_input: bool,
    pub sidechain: bool,
    pub midi: bool,
    pub automation: bool,
    pub modulation: bool,
    pub parameter: bool,
    pub bypass: bool,
    pub routing: bool,
    pub transport: bool,
    pub plugin_state: bool,
}

impl BlockActivity {
    #[inline]
    fn wakes(self, capabilities: RuntimeCapabilities) -> bool {
        self.main_input
            || (self.sidechain && capabilities.wakes_on_sidechain)
            || (self.midi && capabilities.wakes_on_midi)
            || (self.automation && capabilities.wakes_on_automation)
            || (self.modulation && capabilities.wakes_on_modulation)
            || self.parameter
            || self.bypass
            || self.routing
            || self.plugin_state
            || (self.transport && capabilities.wakes_on_transport)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    Automation,
    Modulation,
    Parameter,
    Bypass,
    Routing,
    Transport,
    PluginState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerSnapshot {
    pub running_nodes: u32,
    pub tail_nodes: u32,
    pub sleeping_nodes: u32,
    pub total_nodes: u32,
    pub skipped_process_calls: u64,
    pub wake_count: u64,
    pub sleep_count: u64,
}

impl SchedulerSnapshot {
    #[inline]
    pub fn observe(&mut self, runtime: &NodeRuntime, decision: ProcessDecision) {
        self.total_nodes = self.total_nodes.saturating_add(1);
        match runtime.state {
            RuntimeState::Running => self.running_nodes = self.running_nodes.saturating_add(1),
            RuntimeState::Tail => self.tail_nodes = self.tail_nodes.saturating_add(1),
            RuntimeState::Sleeping => self.sleeping_nodes = self.sleeping_nodes.saturating_add(1),
        }
        if decision == ProcessDecision::SkipAndClear {
            self.skipped_process_calls = self.skipped_process_calls.saturating_add(1);
        }
        self.wake_count = self.wake_count.saturating_add(runtime.block_wakes as u64);
        self.sleep_count = self.sleep_count.saturating_add(runtime.block_sleeps as u64);
    }
}

pub struct NodeRuntime {
    state: RuntimeState,
    capabilities: RuntimeCapabilities,
    policy: SleepPolicy,
    tail_remaining: usize,
    pending_wake: bool,
    block_wakes: u8,
    block_sleeps: u8,
}

impl NodeRuntime {
    pub fn new(capabilities: RuntimeCapabilities) -> Self {
        Self {
            state: RuntimeState::Running,
            capabilities,
            policy: SleepPolicy::Auto,
            tail_remaining: 0,
            pending_wake: true,
            block_wakes: 0,
            block_sleeps: 0,
        }
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn policy(&self) -> SleepPolicy {
        self.policy
    }

    pub fn set_policy(&mut self, policy: SleepPolicy) {
        self.policy = policy;
        self.wake(WakeReason::Routing);
    }

    pub fn update_capabilities(&mut self, capabilities: RuntimeCapabilities) {
        self.capabilities = capabilities;
        self.wake(WakeReason::PluginState);
    }

    pub fn wake(&mut self, _reason: WakeReason) {
        self.pending_wake = true;
    }

    pub fn reset(&mut self) {
        self.state = RuntimeState::Running;
        self.tail_remaining = 0;
        self.pending_wake = true;
        self.block_wakes = 0;
        self.block_sleeps = 0;
    }

    #[inline]
    pub fn begin_block(
        &mut self,
        activity: BlockActivity,
        scheduler_enabled: bool,
        frames: usize,
    ) -> ProcessDecision {
        self.block_wakes = 0;
        self.block_sleeps = 0;

        if !scheduler_enabled
            || self.policy == SleepPolicy::AlwaysProcess
            || !self.capabilities.sleep_safe
            || self.capabilities.requires_continuous_time
        {
            self.transition_awake();
            self.pending_wake = false;
            return ProcessDecision::Process;
        }

        let wakes = self.pending_wake || activity.wakes(self.capabilities);
        self.pending_wake = false;
        if wakes {
            self.transition_awake();
            self.tail_remaining = match self.capabilities.tail {
                RuntimeTail::Finite(samples) => samples,
                _ => 0,
            };
            return ProcessDecision::Process;
        }

        match self.capabilities.tail {
            RuntimeTail::None => self.transition_sleeping(),
            RuntimeTail::Finite(samples) => {
                if self.state == RuntimeState::Running {
                    self.state = RuntimeState::Tail;
                    self.tail_remaining = samples;
                }
                if self.tail_remaining == 0 {
                    self.transition_sleeping()
                } else {
                    self.state = RuntimeState::Tail;
                    self.tail_remaining = self.tail_remaining.saturating_sub(frames);
                    ProcessDecision::Process
                }
            }
            RuntimeTail::Infinite | RuntimeTail::Unknown => {
                self.state = RuntimeState::Tail;
                ProcessDecision::Process
            }
        }
    }

    #[inline]
    fn transition_awake(&mut self) {
        if self.state == RuntimeState::Sleeping {
            self.block_wakes = 1;
        }
        self.state = RuntimeState::Running;
    }

    #[inline]
    fn transition_sleeping(&mut self) -> ProcessDecision {
        if self.state != RuntimeState::Sleeping {
            self.block_sleeps = 1;
        }
        self.state = RuntimeState::Sleeping;
        ProcessDecision::SkipAndClear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(runtime: &mut NodeRuntime) -> ProcessDecision {
        let _ = runtime.begin_block(BlockActivity::default(), true, 64);
        runtime.begin_block(BlockActivity::default(), true, 64)
    }

    #[test]
    fn no_tail_node_sleeps_and_wakes_on_audio() {
        let mut runtime = NodeRuntime::new(RuntimeCapabilities::no_tail());
        assert_eq!(settle(&mut runtime), ProcessDecision::SkipAndClear);
        assert_eq!(runtime.state(), RuntimeState::Sleeping);
        assert_eq!(
            runtime.begin_block(
                BlockActivity {
                    main_input: true,
                    ..BlockActivity::default()
                },
                true,
                64,
            ),
            ProcessDecision::Process
        );
        assert_eq!(runtime.state(), RuntimeState::Running);
    }

    #[test]
    fn finite_tail_runs_to_completion() {
        let mut runtime = NodeRuntime::new(RuntimeCapabilities::finite_tail(128));
        let _ = runtime.begin_block(
            BlockActivity {
                main_input: true,
                ..BlockActivity::default()
            },
            true,
            64,
        );
        assert_eq!(
            runtime.begin_block(BlockActivity::default(), true, 64),
            ProcessDecision::Process
        );
        assert_eq!(runtime.state(), RuntimeState::Tail);
        assert_eq!(
            runtime.begin_block(BlockActivity::default(), true, 64),
            ProcessDecision::Process
        );
        assert_eq!(
            runtime.begin_block(BlockActivity::default(), true, 64),
            ProcessDecision::SkipAndClear
        );
    }

    #[test]
    fn unknown_and_infinite_nodes_never_sleep() {
        for tail in [RuntimeTail::Unknown, RuntimeTail::Infinite] {
            let mut capabilities = RuntimeCapabilities::no_tail();
            capabilities.tail = tail;
            let mut runtime = NodeRuntime::new(capabilities);
            for _ in 0..64 {
                assert_eq!(
                    runtime.begin_block(BlockActivity::default(), true, 64),
                    ProcessDecision::Process
                );
            }
        }
    }

    #[test]
    fn every_event_wake_source_is_visible_before_processing() {
        let mut capabilities = RuntimeCapabilities::no_tail();
        capabilities.wakes_on_midi = true;
        let activities = [
            BlockActivity {
                midi: true,
                ..BlockActivity::default()
            },
            BlockActivity {
                automation: true,
                ..BlockActivity::default()
            },
            BlockActivity {
                modulation: true,
                ..BlockActivity::default()
            },
            BlockActivity {
                sidechain: true,
                ..BlockActivity::default()
            },
        ];
        for activity in activities {
            let mut runtime = NodeRuntime::new(capabilities);
            let _ = settle(&mut runtime);
            assert_eq!(runtime.state(), RuntimeState::Sleeping);
            assert_eq!(
                runtime.begin_block(activity, true, 64),
                ProcessDecision::Process
            );
        }
    }

    #[test]
    fn kill_switch_and_per_node_fallback_always_process() {
        let mut runtime = NodeRuntime::new(RuntimeCapabilities::no_tail());
        let _ = settle(&mut runtime);
        assert_eq!(
            runtime.begin_block(BlockActivity::default(), false, 64),
            ProcessDecision::Process
        );
        runtime.set_policy(SleepPolicy::AlwaysProcess);
        assert_eq!(
            runtime.begin_block(BlockActivity::default(), true, 64),
            ProcessDecision::Process
        );
    }
}
