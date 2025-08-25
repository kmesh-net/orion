// SPDX-FileCopyrightText: © 2025 Huawei Cloud Computing Technologies Co., Ltd
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 Huawei Cloud Computing Technologies Co., Ltd
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

use std::sync::OnceLock;

use lasso::ThreadedRodeo;

static GLOBAL_INTERNER: OnceLock<ThreadedRodeo> = OnceLock::new();

pub fn to_static_str(s: &str) -> &'static str {
    let interner = GLOBAL_INTERNER.get_or_init(ThreadedRodeo::new);
    let key = interner.get_or_intern(s);
    let static_ref = interner.resolve(&key);
    unsafe { std::mem::transmute::<&str, &'static str>(static_ref) }
}
