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

use super::HealthStatus;

pub struct HealthStatusCounter {
    status: Option<HealthStatus>,
    checks: i32,
    healthy_threshold: i32,
    unhealthy_threshold: i32,
}

impl HealthStatusCounter {
    pub fn new(healthy_threshold: u16, unhealthy_threshold: u16) -> Self {
        Self {
            status: None,
            checks: 0,
            healthy_threshold: i32::from(healthy_threshold),
            unhealthy_threshold: -i32::from(unhealthy_threshold),
        }
    }

    pub fn status(&self) -> Option<HealthStatus> {
        self.status
    }

    /// Computes a new successful check. If the health status changes, it is returned.
    pub fn add_success(&mut self) -> Option<HealthStatus> {
        if self.checks < 0 {
            self.checks = 0;
        }
        // No need to count beyond the threshold
        if self.checks < self.healthy_threshold {
            self.checks += 1;
        }
        match self.status {
            None => {
                // During startup, only a single successful health check is required to mark a host healthy
                self.update(HealthStatus::Healthy)
            },
            Some(HealthStatus::Healthy) => None,
            Some(HealthStatus::Unhealthy) => {
                if self.checks >= self.healthy_threshold {
                    self.update(HealthStatus::Healthy)
                } else {
                    None
                }
            },
        }
    }

    /// Computes a new failed check. The unhealthy threshold will be ignored and the endpoint will become unhealthy immediately.
    /// If the health status changes, it is returned.
    pub fn add_failure_ignore_threshold(&mut self) -> Option<HealthStatus> {
        self.add_failure_impl(true)
    }

    /// Computes a new failed check. If the health status changes, it is returned.
    pub fn add_failure(&mut self) -> Option<HealthStatus> {
        self.add_failure_impl(false)
    }

    fn add_failure_impl(&mut self, ignore_thresold: bool) -> Option<HealthStatus> {
        if self.checks > 0 {
            self.checks = 0;
        }
        if self.checks > self.unhealthy_threshold {
            self.checks -= 1;
        }
        match self.status {
            None => {
                // During startup, only a single successful health check is required to mark a host healthy
                self.update(HealthStatus::Unhealthy)
            },
            Some(HealthStatus::Unhealthy) => None,
            Some(HealthStatus::Healthy) => {
                if ignore_thresold || self.checks <= self.unhealthy_threshold {
                    self.update(HealthStatus::Unhealthy)
                } else {
                    None
                }
            },
        }
    }

    fn update(&mut self, new_status: HealthStatus) -> Option<HealthStatus> {
        if Some(new_status) != self.status {
            self.status = Some(new_status);
        }
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::{HealthStatus, HealthStatusCounter};

    #[test]
    fn health_status_counter() {
        const HEALTHY_THRESHOLD: u16 = 5;
        const UNHEALTHY_THRESHOLD: u16 = 10;
        let mut counter = HealthStatusCounter::new(HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

        assert!(counter.status().is_none(), "Expected unknown health status");

        assert_eq!(counter.add_success(), Some(HealthStatus::Healthy), "Expected update on first healthy check");
        assert_eq!(counter.status(), Some(HealthStatus::Healthy), "Expected healthy status");

        // Saturating the healthy state should not cause any new updates
        for _ in 0..HEALTHY_THRESHOLD * 2 {
            assert!(counter.add_success().is_none(), "Unexpected update");
            assert_eq!(counter.status(), Some(HealthStatus::Healthy), "Expected healthy status");
        }

        // Failed checks below unhealthy threshold should not cause a transition
        for _ in 0..UNHEALTHY_THRESHOLD - 1 {
            assert!(counter.add_failure().is_none(), "Unexpected update");
            assert_eq!(counter.status(), Some(HealthStatus::Healthy), "Expected healthy status");
        }

        // Crossing the unhealthy threshold
        assert_eq!(counter.add_failure(), Some(HealthStatus::Unhealthy), "Expected update about unhealthy state");
        assert_eq!(counter.status(), Some(HealthStatus::Unhealthy), "Expected unhealthy status");

        // Saturating the unhealthy state should not cause any new updates
        for _ in 0..UNHEALTHY_THRESHOLD * 2 {
            assert!(counter.add_failure().is_none(), "Unexpected update");
            assert_eq!(counter.status(), Some(HealthStatus::Unhealthy), "Expected unhealthy status");
        }

        // Successful checks below healthy threshold should not cause a transition
        for _ in 0..HEALTHY_THRESHOLD - 1 {
            assert!(counter.add_success().is_none(), "Unexpected update");
            assert_eq!(counter.status(), Some(HealthStatus::Unhealthy), "Expected unhealthy status");
        }

        // Crossing the healthy threshold
        assert_eq!(counter.add_success(), Some(HealthStatus::Healthy), "Expected update about healthy state");
        assert_eq!(counter.status(), Some(HealthStatus::Healthy), "Expected healthy status");

        // A single failed check ignoring the threshold should cause an immmediate transition
        assert_eq!(
            counter.add_failure_ignore_threshold(),
            Some(HealthStatus::Unhealthy),
            "Expected update about unhealthy state"
        );
        assert_eq!(counter.status(), Some(HealthStatus::Unhealthy), "Expected unhealthy status");
    }
}
