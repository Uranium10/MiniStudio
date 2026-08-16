//! Format-neutral helper placement and compatibility policy.
//!
//! VST3/CLAP SDK calls stay in their adapters. This crate owns the process grouping contract and
//! its path-independent compatibility memory, so vendor workarounds cannot leak into the engine.

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use ministudio_plugin_api::{IsolationPlacement, PluginFormat, PluginInstanceId};
use serde::{Deserialize, Serialize};

pub const MAX_GROUP_INSTANCES: u8 = 4;
const CACHE_VERSION: u32 = 1;
const CACHE_RELATIVE_PATH: &str = "plugin-runtime/compatibility-v1.json";

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleKey {
    pub format: PluginFormat,
    /// Hash of the binary contents/metadata, never its installation path.
    pub fingerprint: String,
    pub architecture: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFault {
    Crash,
    Hang,
    DeadlineMiss,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityRecord {
    pub force_dedicated: bool,
    pub crashes: u32,
    pub hangs: u32,
    pub deadline_misses: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompatibilityFile {
    version: u32,
    modules: HashMap<String, CompatibilityRecord>,
}

impl Default for CompatibilityFile {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            modules: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub struct CompatibilityCache {
    file: CompatibilityFile,
}

impl CompatibilityCache {
    pub fn load(user_data_dir: &Path) -> Self {
        let path = cache_path(user_data_dir);
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(file) = serde_json::from_slice::<CompatibilityFile>(&bytes) else {
            return Self::default();
        };
        if file.version != CACHE_VERSION {
            return Self::default();
        }
        Self { file }
    }

    pub fn record(&self, key: &ModuleKey) -> Option<&CompatibilityRecord> {
        self.file.modules.get(&cache_key(key))
    }

    pub fn record_fault(&mut self, key: &ModuleKey, fault: RuntimeFault) {
        let record = self.file.modules.entry(cache_key(key)).or_default();
        match fault {
            RuntimeFault::Crash => record.crashes = record.crashes.saturating_add(1),
            RuntimeFault::Hang => record.hangs = record.hangs.saturating_add(1),
            RuntimeFault::DeadlineMiss => {
                record.deadline_misses = record.deadline_misses.saturating_add(1)
            }
        }
        // One proven fault is sufficient to stop sharing a process. Dedicated mode remains
        // recoverable and avoids taking unrelated plug-ins down with this module.
        record.force_dedicated = true;
    }

    pub fn save(&self, user_data_dir: &Path) -> Result<(), String> {
        let path = cache_path(user_data_dir);
        let parent = path
            .parent()
            .ok_or_else(|| "compatibility cache has no parent directory".to_owned())?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&self.file).map_err(|error| error.to_string())?;
        let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, &path).map_err(|error| error.to_string())
    }
}

#[derive(Default)]
pub struct PlacementPlanner {
    /// Grouping is enabled only after dedicated-helper stability gates pass.
    grouping_enabled: bool,
    groups: HashMap<ModuleKey, Vec<Vec<PluginInstanceId>>>,
    placements: HashMap<PluginInstanceId, ModuleKey>,
}

impl PlacementPlanner {
    pub fn set_grouping_enabled(&mut self, enabled: bool) {
        self.grouping_enabled = enabled;
    }

    pub fn place(
        &mut self,
        instance_id: PluginInstanceId,
        key: ModuleKey,
        compatibility: &CompatibilityCache,
    ) -> IsolationPlacement {
        if !self.grouping_enabled
            || compatibility
                .record(&key)
                .is_some_and(|record| record.force_dedicated)
        {
            return IsolationPlacement::Dedicated;
        }

        let groups = self.groups.entry(key.clone()).or_default();
        let group_index = groups
            .iter()
            .position(|group| group.len() < usize::from(MAX_GROUP_INSTANCES))
            .unwrap_or_else(|| {
                groups.push(Vec::with_capacity(usize::from(MAX_GROUP_INSTANCES)));
                groups.len() - 1
            });
        groups[group_index].push(instance_id.clone());
        self.placements.insert(instance_id, key.clone());
        IsolationPlacement::ModuleGroup {
            fingerprint: key.fingerprint,
            capacity: MAX_GROUP_INSTANCES,
        }
    }

    pub fn release(&mut self, instance_id: &PluginInstanceId) {
        let Some(key) = self.placements.remove(instance_id) else {
            return;
        };
        let Some(groups) = self.groups.get_mut(&key) else {
            return;
        };
        for group in groups.iter_mut() {
            group.retain(|candidate| candidate != instance_id);
        }
        groups.retain(|group| !group.is_empty());
        if groups.is_empty() {
            self.groups.remove(&key);
        }
    }
}

fn cache_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(CACHE_RELATIVE_PATH)
}

fn cache_key(key: &ModuleKey) -> String {
    format!(
        "{}:{}:{}",
        match key.format {
            PluginFormat::Vst3 => "vst3",
            PluginFormat::Clap => "clap",
        },
        key.architecture,
        key.fingerprint
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ModuleKey {
        ModuleKey {
            format: PluginFormat::Vst3,
            fingerprint: "sha256-deadbeef".into(),
            architecture: "x86_64".into(),
        }
    }

    #[test]
    fn dedicated_is_the_stabilization_default() {
        let mut planner = PlacementPlanner::default();
        assert_eq!(
            planner.place(
                PluginInstanceId("a".into()),
                key(),
                &CompatibilityCache::default()
            ),
            IsolationPlacement::Dedicated
        );
    }

    #[test]
    fn a_group_never_exceeds_four_instances() {
        let mut planner = PlacementPlanner::default();
        planner.set_grouping_enabled(true);
        let cache = CompatibilityCache::default();
        for index in 0..9 {
            planner.place(PluginInstanceId(format!("instance-{index}")), key(), &cache);
        }
        let sizes = planner
            .groups
            .get(&key())
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>();
        assert_eq!(sizes, vec![4, 4, 1]);
    }

    #[test]
    fn one_fault_promotes_the_fingerprint_to_dedicated() {
        let mut planner = PlacementPlanner::default();
        planner.set_grouping_enabled(true);
        let mut cache = CompatibilityCache::default();
        cache.record_fault(&key(), RuntimeFault::Hang);
        assert_eq!(
            planner.place(PluginInstanceId("a".into()), key(), &cache),
            IsolationPlacement::Dedicated
        );
    }

    #[test]
    fn cache_contains_no_installation_path() {
        let root = std::env::temp_dir().join(format!(
            "ministudio-compatibility-test-{}",
            std::process::id()
        ));
        let mut cache = CompatibilityCache::default();
        cache.record_fault(&key(), RuntimeFault::Crash);
        cache.save(&root).unwrap();
        let bytes = fs::read_to_string(cache_path(&root)).unwrap();
        assert!(bytes.contains("sha256-deadbeef"));
        assert!(!bytes.contains("Program Files"));
        fs::remove_dir_all(root).unwrap();
    }
}
