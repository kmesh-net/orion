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

use crate::listeners::http_connection_manager::ext_proc::kind;
use crate::listeners::http_connection_manager::ext_proc::worker_config::ExternalProcessingWorkerConfig;

use atomic_enum::atomic_enum;
use orion_configuration::config::network_filters::http_connection_manager::http_filters::ext_proc::{
    BodyProcessingMode, HeaderProcessingMode, ProcessingMode, TrailerProcessingMode,
};
use std::sync::atomic::Ordering;

#[atomic_enum]
#[derive(Default)]
pub enum OverridableHeaderMode {
    #[default]
    Default,
    Send,
    Skip,
}

#[atomic_enum]
pub enum OverridableBodyMode {
    None,
    Streamed,
    Buffered,
    BufferedPartial,
    FullDuplexStreamed,
}

#[atomic_enum]
#[derive(Default)]
pub enum OverridableTrailerMode {
    #[default]
    Default,
    Send,
    Skip,
}

impl From<HeaderProcessingMode> for OverridableHeaderMode {
    fn from(mode: HeaderProcessingMode) -> Self {
        match mode {
            HeaderProcessingMode::Skip => OverridableHeaderMode::Skip,
            HeaderProcessingMode::Send => OverridableHeaderMode::Send,
            HeaderProcessingMode::Default => OverridableHeaderMode::Default,
        }
    }
}

impl From<BodyProcessingMode> for OverridableBodyMode {
    fn from(mode: BodyProcessingMode) -> Self {
        match mode {
            BodyProcessingMode::None => OverridableBodyMode::None,
            BodyProcessingMode::Streamed => OverridableBodyMode::Streamed,
            BodyProcessingMode::Buffered => OverridableBodyMode::Buffered,
            BodyProcessingMode::BufferedPartial => OverridableBodyMode::BufferedPartial,
            BodyProcessingMode::FullDuplexStreamed => OverridableBodyMode::FullDuplexStreamed,
        }
    }
}

impl From<TrailerProcessingMode> for OverridableTrailerMode {
    fn from(mode: TrailerProcessingMode) -> Self {
        match mode {
            TrailerProcessingMode::Skip => OverridableTrailerMode::Skip,
            TrailerProcessingMode::Send => OverridableTrailerMode::Send,
            TrailerProcessingMode::Default => OverridableTrailerMode::Default,
        }
    }
}

// The following code is inspired by Haskell DataKind/TypeFamilies and TypeApplications
//

pub struct OverridableModes<K: kind::MsgKind> {
    header_mode: AtomicOverridableHeaderMode,
    body_mode: AtomicOverridableBodyMode,
    trailer_mode: AtomicOverridableTrailerMode,
    _kind: std::marker::PhantomData<K>,
}

impl<K: kind::MsgKind> OverridableModes<K> {
    #[inline]
    #[allow(dead_code)]
    pub fn header_mode(&self) -> OverridableHeaderMode {
        self.header_mode.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn body_mode(&self) -> OverridableBodyMode {
        self.body_mode.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn trailer_mode(&self) -> OverridableTrailerMode {
        self.trailer_mode.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_header_mode(&self, mode: HeaderProcessingMode) {
        self.header_mode.store(mode.into(), Ordering::Relaxed);
    }

    #[inline]
    pub fn set_body_mode(&self, mode: BodyProcessingMode) {
        self.body_mode.store(mode.into(), Ordering::Relaxed);
    }

    #[inline]
    pub fn set_trailer_mode(&self, mode: TrailerProcessingMode) {
        self.trailer_mode.store(mode.into(), Ordering::Relaxed);
    }

    pub fn spawn(&self) -> Self {
        Self {
            header_mode: AtomicOverridableHeaderMode::new(self.header_mode.load(Ordering::Relaxed)),
            body_mode: AtomicOverridableBodyMode::new(self.body_mode.load(Ordering::Relaxed)),
            trailer_mode: AtomicOverridableTrailerMode::new(self.trailer_mode.load(Ordering::Relaxed)),
            _kind: std::marker::PhantomData,
        }
    }

    pub fn should_process_headers(&self) -> bool {
        match self.header_mode.load(Ordering::Relaxed) {
            OverridableHeaderMode::Default | OverridableHeaderMode::Send => true,
            OverridableHeaderMode::Skip => false,
        }
    }

    pub fn should_process_body(&self) -> bool {
        !matches!(self.body_mode.load(Ordering::Relaxed), OverridableBodyMode::None)
    }

    pub fn should_process_trailers(&self) -> bool {
        match self.trailer_mode.load(Ordering::Relaxed) {
            OverridableTrailerMode::Default | OverridableTrailerMode::Skip => false,
            OverridableTrailerMode::Send => true,
        }
    }
}

impl<K: kind::MsgKind> Default for OverridableModes<K> {
    fn default() -> Self {
        Self {
            header_mode: AtomicOverridableHeaderMode::new(OverridableHeaderMode::Default),
            body_mode: AtomicOverridableBodyMode::new(OverridableBodyMode::None),
            trailer_mode: AtomicOverridableTrailerMode::new(OverridableTrailerMode::Default),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<K: kind::MsgKind> std::fmt::Debug for OverridableModes<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(format!("OverridableModes<{}>", std::any::type_name::<K>()).as_str())
            .field("header_mode", &self.header_mode)
            .field("body_mode", &self.body_mode)
            .field("trailer_mode", &self.trailer_mode)
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct OverridableGlobalModes {
    pub request: OverridableModes<kind::RequestMsg>,
    pub response: OverridableModes<kind::ResponseMsg>,
}

impl OverridableGlobalModes {
    #[inline]
    #[allow(dead_code)]
    pub fn should_process_headers<K: OverridableModeSelector>(&self) -> bool {
        K::get(self).should_process_headers()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn should_process_body<K: OverridableModeSelector>(&self) -> bool {
        K::get(self).should_process_body()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn should_process_trailers<K: OverridableModeSelector>(&self) -> bool {
        K::get(self).should_process_trailers()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn set_header_mode<K: OverridableModeSelector>(&self, mode: HeaderProcessingMode) {
        K::get(self).set_header_mode(mode);
    }

    #[inline]
    #[allow(dead_code)]
    pub fn set_body_mode<K: OverridableModeSelector>(&self, mode: BodyProcessingMode) {
        K::get(self).set_body_mode(mode);
    }

    #[inline]
    #[allow(dead_code)]
    pub fn set_trailer_mode<K: OverridableModeSelector>(&self, mode: TrailerProcessingMode) {
        K::get(self).set_trailer_mode(mode);
    }

    #[inline]
    #[allow(dead_code)]
    pub fn header_mode<K: OverridableModeSelector>(&self) -> OverridableHeaderMode {
        K::get(self).header_mode()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn body_mode<K: OverridableModeSelector>(&self) -> OverridableBodyMode {
        K::get(self).body_mode()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn trailer_mode<K: OverridableModeSelector>(&self) -> OverridableTrailerMode {
        K::get(self).trailer_mode()
    }

    pub fn spawn(&self) -> Self {
        Self { request: self.request.spawn(), response: self.response.spawn() }
    }
}

#[allow(dead_code)]
pub trait ModeSelector: Sized + kind::MsgKind {
    fn header_mode(mode: &ProcessingMode) -> HeaderProcessingMode;
    fn body_mode(mode: &ProcessingMode) -> BodyProcessingMode;
    fn trailer_mode(mode: &ProcessingMode) -> TrailerProcessingMode;
}

impl ModeSelector for kind::RequestMsg {
    #[inline]
    fn header_mode(mode: &ProcessingMode) -> HeaderProcessingMode {
        mode.request_header_mode
    }

    #[inline]
    fn body_mode(mode: &ProcessingMode) -> BodyProcessingMode {
        mode.request_body_mode
    }

    #[inline]
    fn trailer_mode(mode: &ProcessingMode) -> TrailerProcessingMode {
        mode.request_trailer_mode
    }
}

impl ModeSelector for kind::ResponseMsg {
    #[inline]
    fn header_mode(mode: &ProcessingMode) -> HeaderProcessingMode {
        mode.response_header_mode
    }

    #[inline]
    fn body_mode(mode: &ProcessingMode) -> BodyProcessingMode {
        mode.response_body_mode
    }

    #[inline]
    fn trailer_mode(mode: &ProcessingMode) -> TrailerProcessingMode {
        mode.response_trailer_mode
    }
}

pub trait OverridableModeSelector: Sized + kind::MsgKind {
    fn get(global_mode: &OverridableGlobalModes) -> &OverridableModes<Self>;
}

impl OverridableModeSelector for kind::RequestMsg {
    fn get(global_mode: &OverridableGlobalModes) -> &OverridableModes<Self> {
        &global_mode.request
    }
}

impl OverridableModeSelector for kind::ResponseMsg {
    fn get(global_mode: &OverridableGlobalModes) -> &OverridableModes<Self> {
        &global_mode.response
    }
}

impl From<&ExternalProcessingWorkerConfig> for OverridableGlobalModes {
    fn from(config: &ExternalProcessingWorkerConfig) -> Self {
        Self {
            request: OverridableModes {
                header_mode: AtomicOverridableHeaderMode::new(config.processing_mode.request_header_mode.into()),
                body_mode: AtomicOverridableBodyMode::new(config.processing_mode.request_body_mode.into()),
                trailer_mode: AtomicOverridableTrailerMode::new(config.processing_mode.request_trailer_mode.into()),
                _kind: std::marker::PhantomData,
            },
            response: OverridableModes {
                header_mode: AtomicOverridableHeaderMode::new(config.processing_mode.response_header_mode.into()),
                body_mode: AtomicOverridableBodyMode::new(config.processing_mode.response_body_mode.into()),
                trailer_mode: AtomicOverridableTrailerMode::new(config.processing_mode.response_trailer_mode.into()),
                _kind: std::marker::PhantomData,
            },
        }
    }
}
