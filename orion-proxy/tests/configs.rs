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

#![allow(clippy::expect_used)]

use orion_configuration::config::{Config, Runtime};
use orion_configuration::options::Options;
use orion_lib::configuration::get_listeners_and_clusters;
use orion_lib::RUNTIME_CONFIG;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_test::traced_test;

/// we cannot run the tests concurrently because some of them rely on
/// the current working directory ($PWD). This function is just a wrapper with
/// a lock.
fn with_current_dir<F, T>(p: &Path, f: F) -> T
where
    F: FnOnce() -> T,
{
    static TEST_CURRENT_DIR_MUTEX: Mutex<()> = Mutex::new(());
    let _guard = TEST_CURRENT_DIR_MUTEX.lock().expect("Failed to lock test mutex");
    let _ = RUNTIME_CONFIG.set(Runtime::default());
    let save = std::env::current_dir().expect("Failed to get current dir");
    std::env::set_current_dir(p).expect("Failed to set current dir");
    let r = f();
    std::env::set_current_dir(save).expect("Failed to restore current dir");
    r
}

fn check_config_file(file_path: &str) -> Result<(), orion_error::Error> {
    // file_path is relative to crate root
    let bootstrap = Config::new(&Options::from_path_to_envoy(file_path))?.bootstrap;
    // but anciliary files are stored in workspace root - adjust PWD
    let d =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize().expect("Failed to get cargo crate root");
    with_current_dir(&d, || get_listeners_and_clusters(bootstrap).map(|_| ()))
}

#[traced_test]
#[test]
fn bootstrap_demo_static() -> Result<(), orion_error::Error> {
    check_config_file("conf/demo/demo-static.yaml")
}

#[traced_test]
#[test]
fn bootstrap_demo_dynamic() -> Result<(), orion_error::Error> {
    check_config_file("conf/demo/demo-dynamic.yaml")
}
