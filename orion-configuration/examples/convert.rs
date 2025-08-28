// SPDX-FileCopyrightText: © 2025 kmesh authors
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 kmesh authors
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

#![allow(clippy::print_stdout)]

use orion_configuration::config::Bootstrap;
use orion_error::{Context, Result};
use std::fs::File;

fn main() -> Result<()> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("bootstrap.yaml").with_context_msg("failed to open bootstrap.yaml")?,
    )
    .with_context_msg("failed to convert envoy to orion")?;
    let yaml = serde_yaml::to_string(&bootstrap).with_context_msg("failed to serialize orion")?;
    std::fs::write("orion.yaml", yaml.as_bytes())?;
    let bootstrap: Bootstrap =
        serde_yaml::from_reader(File::open("orion.yaml").with_context_msg("failed to open orion.yaml")?)
            .with_context_msg("failed to read yaml from file")?;
    let yaml = serde_yaml::to_string(&bootstrap).with_context_msg("failed to round-trip serialize orion")?;
    println!("{yaml}");
    Ok(())
}
