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

use orion_configuration::config::runtime::{Affinity, CoreId};
use orion_lib::Result;
use std::collections::{BTreeMap, HashSet};

use crate::runtime::RuntimeId;

#[allow(unused_macros)]
#[macro_export]
macro_rules! core_ids {
    ($($x:expr),*) => (
        vec![$($x),*].into_iter().map(CoreId::new).collect::<Vec<CoreId>>()
    );
}

/// Return the number of the available cores to the caller thread.
#[inline]
pub fn get_avail_core_num() -> Result<usize> {
    #[cfg(target_os = "linux")]
    {
        affinity::get_thread_affinity()
            .map_err(|err| format!("get_avail_core_num: {err}").into())
            .map(|cpus| cpus.len())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Fallback implementation for non-Linux systems.
        // This assumes the thread can run on any available logical core.
        Ok(std::thread::available_parallelism()?.get())
    }
}

/// Retrieve the current set of cores available to the caller thread.
#[inline]
pub fn get_core_ids() -> Result<Vec<CoreId>> {
    #[cfg(target_os = "linux")]
    {
        affinity::get_thread_affinity()
            .map_err(|err| format!("get_cores_id: {err}").into())
            .map(|cores| cores.into_iter().map(CoreId::new).collect())
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Fallback implementation for non-Linux systems.
        // This assumes the thread can run on any available logical core.
        // 1. Get the number of logical cores from the standard library.
        let core_count = std::thread::available_parallelism()?.get();

        // 2. Create a vector of CoreIds from 0 to (core_count - 1).
        let core_ids = (0..core_count).map(CoreId::new).collect();

        // 3. Return the list of all available cores.
        Ok(core_ids)
    }
}

/// Set the current set of cores available to the caller thread.
#[inline]
pub fn set_cores_for_current(cores: &[CoreId]) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        affinity::set_thread_affinity(cores.iter().map(|x| **x).collect::<Vec<usize>>())
            .map_err(|err| format!("set_cores_for_current: {err}").into())
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(orion_error::Error::new("set_cores_for_current: not supported on this platform"))
    }
}

/// Returns core IDs grouped by NUMA node, with each inner vector representing the cores for a specific node
#[inline]
pub fn get_cores_ids_per_node() -> Result<Vec<Vec<CoreId>>> {
    let cores = get_core_ids()?;
    std::fs::read_to_string("/proc/cpuinfo").map_err(Into::into).and_then(|cpuinfo| group_by_numa(cores, &cpuinfo))
}

/// Groups core IDs by NUMA node based on CPU information. Returns a vector of vectors,
/// where each inner vector contains core IDs for a specific NUMA node, or an `Err` if no mapping is found.
fn group_by_numa(cores: Vec<CoreId>, cpuinfo: &str) -> Result<Vec<Vec<CoreId>>> {
    let get_values = |needle: &str, haystack: &str| {
        haystack
            .lines()
            .filter(|l| l.starts_with(needle))
            .filter_map(|s| {
                let xs = s.split(':').collect::<Vec<_>>();
                if xs.len() == 2 { xs[1].trim().parse::<usize>().ok() } else { None }
            })
            .collect::<Vec<_>>()
    };

    let processor_map: BTreeMap<usize, usize> = get_values("processor", cpuinfo)
        .into_iter()
        .zip(get_values("physical id", cpuinfo))
        .collect::<BTreeMap<_, _>>();

    if processor_map.is_empty() {
        return Err("cpuinfo: parser error".into());
    }

    let mut groups: BTreeMap<CoreId, Vec<CoreId>> = BTreeMap::new();
    for core in cores {
        if let Some(key) = processor_map.get(&core) {
            groups.entry(CoreId::new(*key)).or_default().push(core);
        } else {
            return Err(format!("cpuinfo: could not find mapping for core {core}").into());
        }
    }

    Ok(groups.into_values().collect::<Vec<_>>())
}

pub trait AffinityStrategy {
    fn run_strategy(&self, runtime_id: RuntimeId, cores_wanted: usize) -> Result<Vec<CoreId>>;
}

impl AffinityStrategy for Affinity {
    fn run_strategy(&self, runtime_id: RuntimeId, cores_wanted: usize) -> Result<Vec<CoreId>> {
        match self {
            Affinity::Auto => {
                let cores_avail = get_cores_ids_per_node()?;
                run_strategy(runtime_id, cores_wanted, None, cores_avail)
            },
            Affinity::Nodes(cs) => {
                let cores_avail = get_cores_ids_per_node()?;
                run_strategy(runtime_id, cores_wanted, Some(cs.clone()), cores_avail)
            },
            Affinity::Runtimes(rs) => {
                let v = rs
                    .get(*runtime_id)
                    .ok_or_else(|| format!("could not find configuration for runtime {runtime_id}"))
                    .cloned()?;

                let v = v.into_iter().take(cores_wanted).collect::<Vec<_>>();
                if v.len() != cores_wanted {
                    return Err(format!(
                        "not enough cores for runtime {runtime_id} - wanted: {}, available: {}",
                        cores_wanted,
                        v.len()
                    )
                    .into());
                }

                Ok(v)
            },
        }
    }
}

// Applies an automatic core affinity strategy for the runtime (identified by its ID) based on the provided information,
// including the required number of cores, any specified core affinity, and the cores available to the calling thread.
fn run_strategy(
    runtime_id: RuntimeId,
    cores_wanted: usize,
    cores_affinity: Option<Vec<Vec<CoreId>>>,
    cores_avail: Vec<Vec<CoreId>>,
) -> Result<Vec<CoreId>> {
    let avail_set = cores_avail.clone().into_iter().flatten().collect::<HashSet<_>>();

    // Retrieve the affinity set or use the available cores if not specified.
    // NOTE: the available cores do not account for NUMA architectures,
    // as they are retrieved as a flat vector.

    let aff = cores_affinity.unwrap_or(cores_avail);

    // ensure that all the cores specified in the strategy are available for the process...
    let aff_set = aff.clone().into_iter().flatten().collect::<HashSet<_>>();

    if !avail_set.is_superset(&aff_set) {
        return Err(format!(
            "the cores {:?} are available",
            aff_set.difference(&avail_set).copied().collect::<Vec<_>>()
        )
        .into());
    }

    // Perform a round-robin selection among the NUMA vectors, selecting the node and
    // then the chunk of cores to use for binding.

    let node = runtime_id
        .0
        .checked_rem(aff.len())
        .and_then(|idx| aff.get(idx))
        .ok_or_else(|| "unexpected affinity vector length".to_owned())?;

    let cores = node
        .iter()
        .skip(
            runtime_id.0.checked_div(aff.len()).ok_or_else(|| "unexpected affinity vector length".to_owned())?
                * cores_wanted,
        )
        .take(cores_wanted)
        .copied()
        .collect::<Vec<_>>();

    if cores.len() == cores_wanted {
        Ok(cores)
    } else {
        Err(format!("not enough cores for runtime {runtime_id} - wanted: {}, available: {}", cores_wanted, cores.len())
            .into())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::too_many_lines)]
    use super::*;

    #[test]
    fn test_group_by_numa_no_cpuinfo() {
        let cores = core_ids![0, 1, 2, 3];
        assert!(group_by_numa(cores, "").is_err());
    }

    #[test]
    fn test_group_by_numa_bad_cpuinfo() {
        let cores = core_ids![0, 1, 2, 3];
        assert!(group_by_numa(cores, "deadbeef").is_err());
    }

    #[test]
    fn test_group_by_numa_single() {
        let cores = core_ids![0, 1, 2, 3];
        let cpuinfo = r"processor       : 0
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 0
cpu cores       : 4
apicid          : 0
initial apicid  : 0
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 1
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 0
cpu cores       : 4
apicid          : 1
initial apicid  : 1
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 2
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 1
cpu cores       : 4
apicid          : 2
initial apicid  : 2
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 3
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 1
cpu cores       : 4
apicid          : 3
initial apicid  : 3
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:";
        assert_eq!(group_by_numa(cores, cpuinfo).unwrap(), vec![core_ids![0, 1, 2, 3]]);
    }

    #[test]
    fn test_group_by_numa_err() {
        let cores = core_ids![4, 5, 6, 7];
        let cpuinfo = r"processor       : 0
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 0
cpu cores       : 4
apicid          : 0
initial apicid  : 0
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 1
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 0
cpu cores       : 4
apicid          : 1
initial apicid  : 1
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 2
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 1
cpu cores       : 4
apicid          : 2
initial apicid  : 2
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 3
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 1
cpu cores       : 4
apicid          : 3
initial apicid  : 3
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:";
        assert!(group_by_numa(cores, cpuinfo).is_err());
    }

    #[test]
    fn test_group_by_numa_dual_nodes() {
        let cores = core_ids![0, 1, 2, 3];
        let cpuinfo = r"processor       : 0
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 0
cpu cores       : 4
apicid          : 0
initial apicid  : 0
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 1
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 1
siblings        : 8
core id         : 0
cpu cores       : 4
apicid          : 1
initial apicid  : 1
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 2
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 0
siblings        : 8
core id         : 1
cpu cores       : 4
apicid          : 2
initial apicid  : 2
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:

processor       : 3
vendor_id       : GenuineIntel
cpu family      : 6
model           : 140
model name      : 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
stepping        : 1
microcode       : 0xffffffff
cpu MHz         : 2803.213
cache size      : 12288 KB
physical id     : 1
siblings        : 8
core id         : 1
cpu cores       : 4
apicid          : 3
initial apicid  : 3
fpu             : yes
fpu_exception   : yes
cpuid level     : 21
wp              : yes
flags           : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid pni pclmulqdq ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch invpcid_single ssbd ibrs ibpb stibp ibrs_enhanced fsgsbase bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx512vbmi umip avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq rdpid fsrm avx512_vp2intersect md_clear flush_l1d arch_capabilities
bugs            : spectre_v1 spectre_v2 spec_store_bypass swapgs retbleed eibrs_pbrsb gds
bogomips        : 5606.42
clflush size    : 64
cache_alignment : 64
address sizes   : 39 bits physical, 48 bits virtual
power management:";
        assert_eq!(group_by_numa(cores, cpuinfo).unwrap(), vec![core_ids![0, 2], core_ids![1, 3]]);
    }

    #[test]
    fn affinity_strategy_unspecified() {
        let avail = vec![core_ids![0, 1, 2, 3, 4, 5, 6, 7, 8]];
        let cores_affinity = None;
        assert_eq!(run_strategy(RuntimeId(0), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![0]);
        assert_eq!(run_strategy(RuntimeId(1), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![1]);
        assert_eq!(run_strategy(RuntimeId(0), 2, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![0, 1]);
        assert_eq!(run_strategy(RuntimeId(1), 2, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![2, 3]);
        assert_eq!(
            run_strategy(RuntimeId(0), 4, cores_affinity.clone(), avail.clone()).unwrap(),
            core_ids![0, 1, 2, 3]
        );
    }

    #[test]
    fn affinity_strategy_single() {
        let avail = vec![core_ids![0, 1, 2, 3, 4, 5, 6, 7, 8]];
        let cores_affinity = Some(vec![core_ids![0, 1, 2, 3]]);
        assert_eq!(run_strategy(RuntimeId(0), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![0]);
        assert_eq!(run_strategy(RuntimeId(1), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![1]);
        assert_eq!(run_strategy(RuntimeId(0), 2, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![0, 1]);
        assert_eq!(run_strategy(RuntimeId(1), 2, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![2, 3]);
        assert_eq!(
            run_strategy(RuntimeId(0), 4, cores_affinity.clone(), avail.clone()).unwrap(),
            core_ids![0, 1, 2, 3]
        );
        assert!(run_strategy(RuntimeId(1), 4, cores_affinity.clone(), avail.clone()).is_err());
    }

    #[test]
    fn affinity_strategy_numa() {
        let avail = vec![core_ids![0, 1, 2, 3, 4, 5, 6, 7, 8]];
        let cores_affinity = Some(vec![core_ids![0, 1, 2], core_ids![3, 4, 5], core_ids![6, 7, 8]]);

        assert_eq!(run_strategy(RuntimeId(0), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![0]);
        assert_eq!(run_strategy(RuntimeId(1), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![3]);
        assert_eq!(run_strategy(RuntimeId(2), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![6]);
        assert_eq!(run_strategy(RuntimeId(3), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![1]);
        assert_eq!(run_strategy(RuntimeId(4), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![4]);
        assert_eq!(run_strategy(RuntimeId(5), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![7]);
        assert_eq!(run_strategy(RuntimeId(6), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![2]);
        assert_eq!(run_strategy(RuntimeId(7), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![5]);
        assert_eq!(run_strategy(RuntimeId(8), 1, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![8]);
        assert!(run_strategy(RuntimeId(9), 1, cores_affinity.clone(), avail.clone()).is_err());

        assert_eq!(run_strategy(RuntimeId(0), 3, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![0, 1, 2]);
        assert_eq!(run_strategy(RuntimeId(1), 3, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![3, 4, 5]);
        assert_eq!(run_strategy(RuntimeId(2), 3, cores_affinity.clone(), avail.clone()).unwrap(), core_ids![6, 7, 8]);
        assert!(run_strategy(RuntimeId(3), 3, cores_affinity.clone(), avail.clone()).is_err());
    }
}
