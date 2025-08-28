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

use orion_configuration::config::bootstrap::Bootstrap;

use crate::{
    ConversionContext, Error, Result, SecretManager, clusters::cluster::PartialClusterType,
    listeners::listener::ListenerFactory,
};

pub fn get_listeners_and_clusters(
    bootstrap: Bootstrap,
) -> Result<(SecretManager, Vec<ListenerFactory>, Vec<PartialClusterType>)> {
    let static_resources = bootstrap.static_resources;
    let secrets = static_resources.secrets;
    let mut secret_manager = SecretManager::new();
    secrets.into_iter().try_for_each(|secret| secret_manager.add(&secret).map(|_| ()))?;

    let listeners = static_resources
        .listeners
        .into_iter()
        .map(|l| ListenerFactory::try_from(ConversionContext::new((l, &secret_manager))))
        .collect::<Result<Vec<_>>>()?;
    let clusters = static_resources
        .clusters
        .into_iter()
        .map(|c| PartialClusterType::try_from((c, &secret_manager)))
        .collect::<Result<Vec<_>>>()?;
    if clusters.is_empty() {
        //shouldn't happen with new config
        return Err::<(SecretManager, Vec<_>, Vec<_>), Error>("No clusters configured".into());
    }
    Ok((secret_manager, listeners, clusters))
}
