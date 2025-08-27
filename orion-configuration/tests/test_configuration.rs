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

use orion_configuration::config::{deserialize_yaml, Bootstrap, Config};
use orion_error::{Context, Error};
use std::{fs::File, path::PathBuf};

#[test]
fn empty_config() {
    let _cfg: Config = deserialize_yaml(&PathBuf::from("tests/config_empty.yaml")).unwrap();
}

#[test]
fn bad_config() {
    let r: Result<Config, _> = deserialize_yaml(&PathBuf::from("tests/config_bad.yaml"));
    assert!(r.is_err());
}

#[test]
fn access_log_file_default() -> Result<(), Error> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/access_log/hcm_file_default.yaml").with_context_msg("failed to open bootstrap.yaml")?,
    )
    .with_context_msg("failed to convert envoy to orion");
    bootstrap.map(|_| ())
}

#[test]
fn access_log_file_with_format() -> Result<(), Error> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/access_log/hcm_file_with_format.yaml").with_context_msg("failed to open file")?,
    )
    .with_context_msg("failed to convert envoy to orion");
    bootstrap.map(|_| ())
}

#[test]
fn access_log_file_bad_name() -> Result<(), Error> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/access_log/hcm_file_bad_name.yaml").with_context_msg("failed to open file")?,
    )
    .with_context_msg("failed to convert envoy to orion");
    let err = bootstrap.unwrap_err();
    println!("{err:?}");
    Ok(())
}

#[test]
fn access_log_file_name_mismatch() -> Result<(), Error> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/access_log/hcm_file_name_mismatch.yaml").with_context_msg("failed to open file")?,
    )
    .with_context_msg("failed to convert envoy to orion");
    let err = bootstrap.unwrap_err();
    println!("{err:?}");
    Ok(())
}

#[test]
fn access_log_file_with_bad_format() -> Result<(), Error> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/access_log/hcm_file_with_bad_format.yaml").with_context_msg("failed to open file")?,
    )
    .with_context_msg("failed to convert envoy to orion");
    let err = bootstrap.unwrap_err();
    println!("{err:?}");
    Ok(())
}

#[test]
fn access_log_file_without_path() -> Result<(), Error> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/access_log/hcm_file_without_path.yaml").with_context_msg("failed to open file")?,
    )
    .with_context_msg("failed to convert envoy to orion");
    let err = bootstrap.unwrap_err();
    println!("{err:?}");
    Ok(())
}
