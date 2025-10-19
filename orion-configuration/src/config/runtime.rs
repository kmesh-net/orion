// Copyright 2025 The kmesh Authors
//
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//

use serde::{Deserialize, Serialize};
use std::{
    env::var,
    fmt::Display,
    num::{NonZeroU32, NonZeroUsize},
    ops::Deref,
};
use tracing;

use crate::options::Options;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Runtime {
    #[serde(default = "non_zero_num_cpus")]
    pub num_cpus: NonZeroUsize,
    #[serde(default = "one")]
    pub num_runtimes: NonZeroU32,
    #[serde(default = "one")]
    pub num_service_threads: NonZeroU32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub global_queue_interval: Option<NonZeroU32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub affinity_strategy: Option<Affinity>,
    //may be zero?
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_interval: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_io_events_per_tick: Option<NonZeroUsize>,
}

fn one() -> NonZeroU32 {
    NonZeroU32::MIN
}

impl Runtime {
    #[must_use]
    pub fn update_from_env_and_options(self, opt: &Options) -> Self {
        Runtime {
            num_cpus: var("ORION_CPU_LIMIT")
                .ok()
                .and_then(|v| v.parse::<NonZeroUsize>().ok())
                .or_else(|| var("ORION_GATEWAY_CORES").ok().and_then(|v| v.parse::<NonZeroUsize>().ok()))
                .or(opt.num_cpus)
                .unwrap_or(self.num_cpus),

            num_service_threads: var("ORION_SERVICE_THREADS")
                .ok()
                .and_then(|v| v.parse::<NonZeroU32>().ok())
                .or(opt.num_service_threads)
                .unwrap_or(self.num_service_threads),

            num_runtimes: var("ORION_GATEWAY_RUNTIMES")
                .ok()
                .and_then(|v| v.parse::<NonZeroU32>().ok())
                .or(opt.num_runtimes)
                .unwrap_or(self.num_runtimes),

            global_queue_interval: var("ORION_RT_GLOBAL_QUEUE_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<NonZeroU32>().ok())
                .or(opt.global_queue_interval)
                .or(self.global_queue_interval),

            event_interval: var("ORION_RT_EVENT_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .or(opt.event_interval)
                .or(self.event_interval),

            max_io_events_per_tick: var("ORION_RT_MAX_IO_EVENT_PER_TICK")
                .ok()
                .and_then(|v| v.parse::<NonZeroUsize>().ok())
                .or(opt.max_io_events_per_tick)
                .or(self.max_io_events_per_tick),

            affinity_strategy: self.affinity_strategy,
        }
    }

    //upcasts to usize to make it easier to do math with it and num_cpus
    pub fn num_runtimes(&self) -> usize {
        self.num_runtimes.get() as usize
    }
    pub fn num_cpus(&self) -> usize {
        self.num_cpus.get()
    }
}

#[allow(clippy::expect_used)]
pub(crate) fn non_zero_num_cpus() -> NonZeroUsize {
    let cpus = detect_available_cpus();
    NonZeroUsize::try_from(cpus).expect("found zero cpus")
}

#[cfg(target_os = "linux")]
fn detect_available_cpus() -> usize {
    match get_container_cpu_limit() {
        Ok(container_cpus) => {
            tracing::debug!("Detected container CPU limit: {}", container_cpus);
            container_cpus
        },
        Err(e) => {
            tracing::debug!("Could not detect container CPU limit: {}. Falling back to system CPU count.", e);
            num_cpus::get()
        },
    }
}

#[cfg(not(target_os = "linux"))]
fn detect_available_cpus() -> usize {
    num_cpus::get()
}

fn get_container_cpu_limit() -> crate::Result<usize> {
    get_cgroup_v2_cpu_limit().or_else(|_| get_cgroup_v1_cpu_limit())
}

fn get_cgroup_v2_cpu_limit() -> crate::Result<usize> {
    if !std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        return Err("cgroups v2 not available".into());
    }

    if let Ok(content) = std::fs::read_to_string("/sys/fs/cgroup/cpu.max") {
        return parse_cgroup_v2_cpu_max(&content);
    }
    Err("No cgroups v2 CPU limit found".into())
}

fn parse_cgroup_v2_cpu_max(content: &str) -> crate::Result<usize> {
    let parts: Vec<&str> = content.trim().split_whitespace().collect();
    if parts.len() == 2 && parts[0] != "max" {
        let quota: i64 = parts[0].parse()?;
        let period: i64 = parts[1].parse()?;
        if quota > 0 && period > 0 {
            let cpus = ((quota as f64) / (period as f64)).ceil() as usize;
            if cpus > 0 {
                return Ok(cpus);
            }
        }
    }
    Err("No valid cgroups v2 CPU limit found".into())
}

fn get_cgroup_v1_cpu_limit() -> crate::Result<usize> {
    let cgroup_path = get_cgroup_v1_cpu_path()?;
    let quota_path = format!("{cgroup_path}/cpu.cfs_quota_us");
    let period_path = format!("{cgroup_path}/cpu.cfs_period_us");

    let quota_content = std::fs::read_to_string(&quota_path)?;
    let period_content = std::fs::read_to_string(&period_path)?;

    parse_cgroup_v1_cpu_limit(&quota_content, &period_content)
}

fn parse_cgroup_v1_cpu_limit(quota_content: &str, period_content: &str) -> crate::Result<usize> {
    let quota: i64 = quota_content.trim().parse()?;
    let period: i64 = period_content.trim().parse()?;

    if quota > 0 && period > 0 {
        let cpus = ((quota as f64) / (period as f64)).ceil() as usize;
        if cpus > 0 {
            return Ok(cpus);
        }
    }

    Err("No valid cgroups v1 CPU limit found".into())
}

fn get_cgroup_v1_cpu_path() -> crate::Result<String> {
    let cgroup_content = std::fs::read_to_string("/proc/self/cgroup")?;
    parse_cgroup_v1_cpu_path(&cgroup_content)
}

fn parse_cgroup_v1_cpu_path(cgroup_content: &str) -> crate::Result<String> {
    if let Some(path) = cgroup_content.lines().find_map(|line| {
        let mut parts = line.split(':');
        if let (Some(_), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next()) {
            if controllers.split(',').any(|c| c == "cpu") {
                return Some(format!("/sys/fs/cgroup/cpu{path}"));
            }
        }
        None
    }) {
        return Ok(path);
    }

    if std::path::Path::new("/sys/fs/cgroup/cpu").exists() {
        Ok("/sys/fs/cgroup/cpu".to_owned())
    } else {
        Err("CPU cgroup path not found".into())
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            num_cpus: non_zero_num_cpus(),
            num_runtimes: one(),
            num_service_threads: one(),
            global_queue_interval: None,
            event_interval: None,
            max_io_events_per_tick: None,
            affinity_strategy: None,
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
pub struct CoreId(usize);

impl CoreId {
    pub fn new(id: usize) -> CoreId {
        CoreId(id)
    }
}

impl Display for CoreId {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for CoreId {
    type Target = usize;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Deserialize, Serialize, PartialEq, Eq, Debug, Clone)]
#[serde(tag = "type", content = "map")]
pub enum Affinity {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "nodes")]
    Nodes(Vec<Vec<CoreId>>),
    #[serde(rename = "runtimes")]
    Runtimes(Vec<Vec<CoreId>>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cgroup_v1_cpu_limit_parsing() {
        // Test normal case: 200000/100000 = 2 CPUs
        let quota_content = "200000\n";
        let period_content = "100000\n";
        let result = parse_cgroup_v1_cpu_limit(quota_content, period_content);
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn test_cgroup_v1_cpu_limit_parsing_fractional() {
        // Test fractional case: 150000/100000 = 1.5, should ceil to 2 CPUs
        let quota_content = "150000\n";
        let period_content = "100000\n";
        let result = parse_cgroup_v1_cpu_limit(quota_content, period_content);
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn test_cgroup_v1_cpu_limit_parsing_invalid() {
        // Test with invalid quota (negative or zero)
        let quota_content = "-1\n";
        let period_content = "100000\n";
        let result = parse_cgroup_v1_cpu_limit(quota_content, period_content);
        assert!(result.is_err());

        let quota_content = "0\n";
        let period_content = "100000\n";
        let result = parse_cgroup_v1_cpu_limit(quota_content, period_content);
        assert!(result.is_err());
    }

    #[test]
    fn test_cgroup_v2_cpu_max_parsing() {
        // Test normal case: 200000/100000 = 2 CPUs
        let content = "200000 100000\n";
        let result = parse_cgroup_v2_cpu_max(content);
        assert_eq!(result.unwrap(), 2);

        // Test max case (no limit)
        let content = "max 100000\n";
        let result = parse_cgroup_v2_cpu_max(content);
        assert!(result.is_err());

        // Test fractional case: 150000/100000 = 1.5, should ceil to 2 CPUs
        let content = "150000 100000\n";
        let result = parse_cgroup_v2_cpu_max(content);
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn test_container_cpu_detection_fallback() {
        let system_cpus = num_cpus::get();
        let detected_cpus = detect_available_cpus();
        assert!(detected_cpus > 0);
        println!("System CPUs: {system_cpus}, Detected CPUs: {detected_cpus}");
    }

    #[test]
    fn test_runtime_env_override() {
        std::env::set_var("ORION_GATEWAY_CORES", "4");

        let runtime = Runtime::default();
        let options = crate::options::Options {
            config_files: crate::options::ConfigFiles {
                config: None,
                #[cfg(feature = "envoy-conversions")]
                bootstrap_override: None,
            },
            num_cpus: None,
            num_runtimes: None,
            num_service_threads: None,
            global_queue_interval: None,
            event_interval: None,
            max_io_events_per_tick: None,
            clusters_manager_queue_length: None,
            core_ids: None,
        };
        let updated_runtime = runtime.update_from_env_and_options(&options);

        assert_eq!(updated_runtime.num_cpus(), 4);
        std::env::remove_var("ORION_GATEWAY_CORES");
    }

    #[test]
    fn test_non_zero_num_cpus() {
        let cpus = non_zero_num_cpus();
        assert!(cpus.get() > 0);
    }
}
