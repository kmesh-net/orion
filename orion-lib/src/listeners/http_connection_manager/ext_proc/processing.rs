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

use crate::body::channel_body::FrameBridge;
use crate::event_error::EventKind;
use crate::listeners::http_connection_manager::ext_proc::kind;
use crate::listeners::http_connection_manager::ext_proc::mutation::apply_trailer_mutations;
use crate::listeners::http_connection_manager::ext_proc::r#override::{
    OverridableGlobalModes, OverridableModeSelector,
};
use crate::listeners::http_connection_manager::ext_proc::status::{Action, ProcessingStatus, ReadyStatus};
use crate::listeners::http_connection_manager::ext_proc::worker_config::ExternalProcessingWorkerConfig;
use crate::listeners::http_connection_manager::ext_proc::EnvoyHeaderMap;
use crate::{body::response_flags::ResponseFlags, listeners::synthetic_http_response::SyntheticHttpResponse};
use bytes::Bytes;
use http_body::Frame;
use orion_configuration::config::network_filters::http_connection_manager::http_filters::ext_proc::{
    BodyProcessingMode, HeaderProcessingMode, ProcessingMode, RouteCacheAction, TrailerProcessingMode,
};
use orion_data_plane_api::envoy_data_plane_api::envoy::service::ext_proc::v3::common_response::ResponseStatus;
use orion_data_plane_api::envoy_data_plane_api::envoy::service::ext_proc::v3::HttpTrailers;
use orion_data_plane_api::envoy_data_plane_api::envoy::{
    extensions::filters::http::ext_proc::v3::ProcessingMode as EnvoyProcessingMode,
    service::ext_proc::v3::{
        body_mutation::Mutation, processing_request::Request as ProcessingRequestType, BodyResponse, HeadersResponse,
        HttpBody, HttpHeaders, ProcessingRequest, TrailersResponse,
    },
};

use orion_format::types::ResponseFlags as FmtResponseFlags;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use tokio::sync::oneshot;
use tracing::{debug, warn};

pub struct RequestProcessing<M: kind::Mode>(Processing<M, kind::RequestMsg>);

impl<M: kind::Mode> Deref for RequestProcessing<M> {
    type Target = Processing<M, kind::RequestMsg>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<M: kind::Mode> DerefMut for RequestProcessing<M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct ResponseProcessing<M: kind::Mode>(Processing<M, kind::ResponseMsg>);

impl<M: kind::Mode> Deref for ResponseProcessing<M> {
    type Target = Processing<M, kind::ResponseMsg>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<M: kind::Mode> DerefMut for ResponseProcessing<M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub struct FrameBuffer {
    data_buffer: Option<Frame<Bytes>>,
    trailers_buffer: Option<Frame<Bytes>>,
}

impl FrameBuffer {
    fn new() -> Self {
        Self { data_buffer: None, trailers_buffer: None }
    }

    // park can either return a DATA frame or None
    pub fn park(&mut self, frame: Frame<Bytes>) -> Option<Frame<Bytes>> {
        if frame.is_data() {
            let frame_to_emit = self.data_buffer.take();

            // DATA
            if self.trailers_buffer.is_some() {
                // This case should never occur, as no frames are expected after the final TRAILERS.
                warn!(target: "ext_proc", "FrameBuffer::park: unexpected frame after TRAILERS!");
            } else {
                self.data_buffer = Some(frame);
            }
            frame_to_emit
        } else {
            // TRAILERS
            let data = self.data_buffer.take();
            self.trailers_buffer = Some(frame);
            data
        }
    }

    // take can either return a DATA or TRAILERS frame
    pub fn take(&mut self) -> Option<Frame<Bytes>> {
        if let Some(frame) = self.data_buffer.take() {
            Some(frame)
        } else {
            self.trailers_buffer.take()
        }
    }

    #[inline]
    #[allow(unused)]
    pub fn has_data(&self) -> bool {
        self.data_buffer.is_some()
    }

    #[inline]
    #[allow(unused)]
    pub fn has_trailers(&self) -> bool {
        self.trailers_buffer.is_some()
    }
}

#[allow(clippy::struct_excessive_bools)]
pub struct Processing<M: kind::Mode, Msg: kind::MsgKind> {
    http_headers: Option<http::HeaderMap>,
    pub trailers: Option<http::HeaderMap>,
    pub frame_bridge: FrameBridge,
    pub reply_channel: Option<oneshot::Sender<ProcessingStatus>>,
    http_version: Option<http::Version>,
    send_body_without_waiting_for_header_response: bool,
    pub failure_mode_allow: bool,
    pub streaming_body_enabled: bool,
    pub end_of_stream: bool,
    pub frames_buffer: FrameBuffer,
    pub inflight_frames: Vec<Frame<Bytes>>,
    pub parked_trailers: Option<Frame<Bytes>>,
    _mode: std::marker::PhantomData<M>,
    _msg: std::marker::PhantomData<Msg>,
}

impl<M: kind::Mode + Default, Msg: kind::MsgKind> From<&ExternalProcessingWorkerConfig> for Processing<M, Msg> {
    fn from(config: &ExternalProcessingWorkerConfig) -> Self {
        debug!(target: "ext_proc", "From<&ExternalProcessingWorkerConfig for Processing<>");

        Self {
            send_body_without_waiting_for_header_response: config.send_body_without_waiting_for_header_response,
            failure_mode_allow: config.failure_mode_allow,

            http_headers: None,
            frame_bridge: FrameBridge::default(),
            trailers: None,
            reply_channel: None,
            http_version: None,
            streaming_body_enabled: false,
            end_of_stream: false,
            frames_buffer: FrameBuffer::new(),
            inflight_frames: vec![],
            parked_trailers: None,
            _mode: std::marker::PhantomData,
            _msg: std::marker::PhantomData,
        }
    }
}

impl<M: kind::Mode + Default> From<&ExternalProcessingWorkerConfig> for RequestProcessing<M> {
    fn from(value: &ExternalProcessingWorkerConfig) -> Self {
        debug!(target: "ext_proc", "From<&ExternalProcessingWorkerConfig for RequestProcessing<{}>", std::any::type_name::<M>());
        Self(Processing::<M, kind::RequestMsg>::from(value))
    }
}

impl<M: kind::Mode + Default> From<&ExternalProcessingWorkerConfig> for ResponseProcessing<M> {
    fn from(value: &ExternalProcessingWorkerConfig) -> Self {
        debug!(target: "ext_proc", "From<&ExternalProcessingWorkerConfig for ResponseProcessing<{}>", std::any::type_name::<M>());
        Self(Processing::<M, kind::ResponseMsg>::from(value))
    }
}

impl Processing<kind::Processing, kind::RequestMsg> {
    #[allow(clippy::unused_self)]
    pub fn apply_mode_overrides(
        &self,
        envoy_mode: &EnvoyProcessingMode,
        allowed_override_modes: &[ProcessingMode],
        override_mode: &OverridableGlobalModes,
    ) {
        // --- Body Mode Override ---
        if let Ok(mode) = BodyProcessingMode::try_from(envoy_mode.request_body_mode) {
            if allowed_override_modes.is_empty()
                || allowed_override_modes.iter().any(|allowed| allowed.request_body_mode == mode)
            {
                override_mode.set_body_mode::<kind::RequestMsg>(mode);
            }
        }

        // --- Trailer Mode Override ---
        if let Ok(mode) = TrailerProcessingMode::try_from(envoy_mode.request_trailer_mode) {
            if mode != TrailerProcessingMode::Default
                && (allowed_override_modes.is_empty()
                    || allowed_override_modes.iter().any(|allowed| allowed.request_trailer_mode == mode))
            {
                override_mode.set_trailer_mode::<kind::RequestMsg>(mode);
            }
        }
    }
}

impl Processing<kind::Processing, kind::ResponseMsg> {
    #[allow(clippy::unused_self)]
    pub fn apply_mode_overrides(
        &self,
        envoy_mode: &EnvoyProcessingMode,
        allowed_override_modes: &[ProcessingMode],
        override_mode: &OverridableGlobalModes,
    ) {
        // --- Header Mode Override ---
        if let Ok(mode) = HeaderProcessingMode::try_from(envoy_mode.response_header_mode) {
            if mode != HeaderProcessingMode::Default
                && (allowed_override_modes.is_empty()
                    || allowed_override_modes.iter().any(|allowed| allowed.response_header_mode == mode))
            {
                override_mode.set_header_mode::<kind::ResponseMsg>(mode);
            }
        }

        // --- Body Mode Override ---
        if let Ok(mode) = BodyProcessingMode::try_from(envoy_mode.response_body_mode) {
            if allowed_override_modes.is_empty()
                || allowed_override_modes.iter().any(|allowed| allowed.response_body_mode == mode)
            {
                override_mode.set_body_mode::<kind::ResponseMsg>(mode);
            }
        }

        // --- Trailer Mode Override ---
        if let Ok(mode) = TrailerProcessingMode::try_from(envoy_mode.response_trailer_mode) {
            if mode != TrailerProcessingMode::Default
                && (allowed_override_modes.is_empty()
                    || allowed_override_modes.iter().any(|allowed| allowed.response_trailer_mode == mode))
            {
                override_mode.set_trailer_mode::<kind::ResponseMsg>(mode);
            }
        }
    }
}

impl<Msg: kind::MsgKind + OverridableModeSelector> Processing<kind::Processing, Msg> {
    #[must_use = "must handle the returned Action"]
    pub async fn handle_headers_response(
        &mut self,
        response: HeadersResponse,
        route_cache_action: &RouteCacheAction,
        override_mode: &OverridableGlobalModes,
        timeout_active: &mut bool,
    ) -> Action<ProcessingRequest> {
        let status;

        if let Some(response_data) = response.response {
            let should_clear_route_cache = match route_cache_action {
                RouteCacheAction::Clear => true,
                RouteCacheAction::Retain => false,
                RouteCacheAction::Default => response_data.clear_route_cache,
            };

            status = if Msg::IS_REQUEST {
                ProcessingStatus::RequestReady(ReadyStatus {
                    headers_modifications: response_data.header_mutation,
                    clear_route_cache: should_clear_route_cache,
                })
            } else {
                ProcessingStatus::ResponseReady(ReadyStatus {
                    headers_modifications: response_data.header_mutation,
                    clear_route_cache: should_clear_route_cache,
                })
            };

            let embedded_status = ResponseStatus::try_from(response_data.status).unwrap_or(ResponseStatus::Continue);

            if matches!(embedded_status, ResponseStatus::ContinueAndReplace) {
                debug!(target: "ext_proc", "handle_headers_response: ResponseStatus:ContinueAndReplace");
                let body_replacement =
                    match response_data.body_mutation.and_then(|body_mutation| body_mutation.mutation) {
                        Some(Mutation::Body(bytes)) => Some(Frame::data(bytes.into())),
                        Some(Mutation::ClearBody(true)) => Some(Frame::data(Bytes::new())),
                        Some(Mutation::ClearBody(false)) | None => None,
                        Some(Mutation::StreamedResponse(chunk)) => Some(Frame::data(chunk.body.into())),
                    };

                // replace body if specified
                if let Some(new_body) = body_replacement {
                    debug!(target: "ext_proc", "handle_headers_response: body replacement requested: {new_body:?}");
                    _ = self.frame_bridge.inject_frame(Ok(new_body)).await;
                }

                // replace trailers if specified
                if let Some(trailers) = response_data.trailers {
                    debug!(target: "ext_proc", "handle_headers_response: trailers replacement requested: {trailers:?}");
                    let new_trailers: http::HeaderMap = EnvoyHeaderMap(trailers).into();
                    _ = self.frame_bridge.inject_frame(Ok(Frame::trailers(new_trailers))).await;
                }

                debug!(target: "ext_proc", "frame bridge closed (handle header response)!");
                self.frame_bridge_close(timeout_active).await;
            } else {
                debug!(target: "ext_proc", "handle_headers_response: ResponseStatus:Continue: headers processed");
                if !override_mode.should_process_body::<Msg>() && !override_mode.should_process_trailers::<Msg>() {
                    debug!(target: "ext_proc", "handle_headers_response: complete to stream original body and close!");
                    self.frame_bridge.complete().await;
                    debug!(target: "ext_proc", "frame bridge closed (handle header response)!");
                    self.frame_bridge_close(timeout_active).await;
                } else {
                    debug!(target: "ext_proc", "handle_headers_response: streaming body enabled...");
                    self.streaming_body_enabled = true;
                }
            }
        } else {
            // todo(Nicola): this is UB; we are not sure what to do if response.response is None
            status = if Msg::IS_REQUEST {
                ProcessingStatus::RequestReady(ReadyStatus { headers_modifications: None, clear_route_cache: false })
            } else {
                ProcessingStatus::ResponseReady(ReadyStatus { headers_modifications: None, clear_route_cache: false })
            };

            self.streaming_body_enabled = true;
        }

        *timeout_active = false;
        Action::Return(status)
    }

    #[allow(clippy::too_many_lines)]
    #[must_use = "must handle the returned Action"]
    pub async fn handle_body_response(
        &mut self,
        body_response: BodyResponse,
        route_cache_action: Option<&RouteCacheAction>,
        timeout_active: &mut bool,
    ) -> Action<ProcessingRequest> {
        debug!(target: "ext_proc", "handle_body_response: inflight frames ({})", self.inflight_frames.len());

        if let Some(response_data) = body_response.response {
            let chunk_replacement = match response_data.body_mutation.and_then(|body_mutation| body_mutation.mutation) {
                Some(Mutation::Body(bytes)) => Some(Frame::data(bytes.into())),
                Some(Mutation::ClearBody(true)) => Some(Frame::data(Bytes::new())),
                Some(Mutation::ClearBody(false)) | None => None,
                Some(Mutation::StreamedResponse(chunk)) => Some(Frame::data(chunk.body.into())),
            };

            let mut status = if Msg::IS_REQUEST {
                ProcessingStatus::RequestReady(ReadyStatus::default())
            } else {
                ProcessingStatus::ResponseReady(ReadyStatus::default())
            };

            if let Some(new_chunk) = chunk_replacement {
                debug!(target: "ext_proc", "handle_body_response: chunk replacement -> {new_chunk:?}");
                _ = self.frame_bridge.inject_frame(Ok(new_chunk)).await;
                if !self.inflight_frames.is_empty() {
                    self.inflight_frames.drain(0..1);
                }
            } else {
                debug!(target: "ext_proc", "handle_body_response: no chunk replacement requested");
                if !self.inflight_frames.is_empty() {
                    if let Some(frame) = self.inflight_frames.drain(0..1).next() {
                        _ = self.frame_bridge.inject_frame(Ok(frame)).await;
                    }
                }
            }

            if let Some(route_cache_action) = route_cache_action {
                if let Some(header_modifications) = response_data.header_mutation {
                    let should_clear_route_cache = match route_cache_action {
                        RouteCacheAction::Clear => true,
                        RouteCacheAction::Retain => false,
                        RouteCacheAction::Default => response_data.clear_route_cache,
                    };

                    if Msg::IS_REQUEST {
                        status.with_request_ready(|req_ready| {
                            req_ready.headers_modifications = Some(header_modifications);
                            req_ready.clear_route_cache = should_clear_route_cache;
                        });
                    } else {
                        status.with_response_ready(|resp_ready| {
                            resp_ready.headers_modifications = Some(header_modifications);
                            resp_ready.clear_route_cache = should_clear_route_cache;
                        });
                    }
                }
            }

            let embedded_status = ResponseStatus::try_from(response_data.status).unwrap_or(ResponseStatus::Continue);

            if matches!(embedded_status, ResponseStatus::ContinueAndReplace) {
                debug!(target: "ext_proc", "handle_body_response: CONTINUE_AND_REPLACE: sending message status {status:?}");
                self.frame_bridge_close(timeout_active).await;
                return Action::Return(status);
            }

            if self.end_of_stream && self.inflight_frames.is_empty() {
                debug!(target: "ext_proc", "handle_body_response: end_of_stream (closing frame bridge)!");
                self.frame_bridge_close(timeout_active).await;
            }

            Action::Return(status)
        } else {
            Action::Return(
                self.status_error("handle_body_response: No response data in body response", self.failure_mode_allow),
            )
        }
    }

    #[must_use = "must handle the returned Action"]
    pub async fn handle_trailers_response(
        &mut self,
        trailers_response: TrailersResponse,
        timeout_active: &mut bool,
    ) -> Action<ProcessingRequest> {
        if let Some(mut trailers) = self.trailers.take() {
            // update the local version of trailers, if required if let Some(trailers) = self.body_context.trailers.as_mut() {
            debug!(target: "ext_proc", "handle_trailers_response: mutating trailers...");
            if let Some(ref trailers_updates) = trailers_response.header_mutation {
                let _ = apply_trailer_mutations(&mut trailers, trailers_updates, None);
            }

            _ = self.frame_bridge.inject_frame(Ok(Frame::trailers(trailers))).await;

            self.end_of_stream = true;
            self.frame_bridge_close(timeout_active).await;

            let status = if Msg::IS_REQUEST {
                ProcessingStatus::RequestReady(ReadyStatus::default())
            } else {
                ProcessingStatus::ResponseReady(ReadyStatus::default())
            };

            Action::Return(status)
        } else {
            debug!(target: "ext_proc", "frame bridge closed (handle trailers response)!");
            self.frame_bridge_close(timeout_active).await;
            Action::Return(
                self.status_error("handle_trailers_response: No trailers to process", self.failure_mode_allow),
            )
        }
    }

    pub async fn inject_inflight_frames_and_complete(&mut self) {
        debug!(target: "ext_proc", "interrupt_and_complete!");
        for frame in self.inflight_frames.drain(..) {
            _ = self.frame_bridge.inject_frame(Ok(frame)).await;
        }
        self.frame_bridge.complete().await;
    }
}

impl<M: kind::Mode + Default, Msg: kind::MsgKind + OverridableModeSelector> Processing<M, Msg> {
    #[must_use = "must handle the returned Action"]
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        headers: Option<http::HeaderMap>,
        frame_bridge: FrameBridge,
        reply_channel: oneshot::Sender<ProcessingStatus>,
        http_version: http::Version,
        override_mode: &OverridableGlobalModes,
    ) -> Action<ProcessingRequest> {
        self.reply_channel = Some(reply_channel);
        self.http_version = Some(http_version);
        self.http_headers = headers;
        self.frame_bridge = frame_bridge;

        if self.http_headers.is_some() {
            return self.process_headers(override_mode);
        }

        debug!(target: "ext_proc", "process: turning streaming_body_enabled -> true");
        self.streaming_body_enabled = true;

        let status = if Msg::IS_REQUEST {
            ProcessingStatus::RequestReady(ReadyStatus::default())
        } else {
            ProcessingStatus::ResponseReady(ReadyStatus::default())
        };

        Action::Return(status)
    }

    #[must_use = "must handle the returned Action"]
    fn process_headers(&mut self, override_mode: &OverridableGlobalModes) -> Action<ProcessingRequest> {
        debug!(target: "ext_proc", "process_request headers {:?}", self.http_headers);
        let Some(headers) = &self.http_headers else {
            return Action::Return(self.status_error(
                format!("process_request: Unexpected headers provided: {:?}", self.http_headers).as_str(),
                self.failure_mode_allow,
            ));
        };

        let end_of_stream = self.frame_bridge.is_empty_body()
            || (!override_mode.should_process_body::<Msg>() && !override_mode.should_process_trailers::<Msg>());

        let envmap: EnvoyHeaderMap = headers.into();

        let processing_request = if Msg::IS_REQUEST {
            ProcessingRequest {
                request: Some(ProcessingRequestType::RequestHeaders(HttpHeaders {
                    headers: Some(envmap.0),
                    attributes: HashMap::default(),
                    end_of_stream,
                })),
                metadata_context: None,
                attributes: HashMap::default(),
                observability_mode: M::OBSERVABILITY,
                protocol_config: None,
            }
        } else {
            ProcessingRequest {
                request: Some(ProcessingRequestType::ResponseHeaders(HttpHeaders {
                    headers: Some(envmap.0),
                    attributes: HashMap::default(),
                    end_of_stream,
                })),
                metadata_context: None,
                attributes: HashMap::default(),
                observability_mode: M::OBSERVABILITY,
                protocol_config: None,
            }
        };

        let send_body_or_trailers =
            override_mode.should_process_body::<Msg>() || override_mode.should_process_trailers::<Msg>();

        if M::OBSERVABILITY || (self.send_body_without_waiting_for_header_response && send_body_or_trailers) {
            self.streaming_body_enabled = true;
        }

        Action::Send(processing_request)
    }

    #[must_use = "must handle the returned Action"]
    pub fn handle_outgoing_body_chunk(
        &mut self,
        mut chunk: Frame<Bytes>,
        end_of_stream: bool,
    ) -> Action<ProcessingRequest> {
        debug!(target: "ext_proc", "handle_body_chunk: sending data frame: {}, end_of_stream: {end_of_stream}",
            if chunk.is_data() { "DATA" } else if chunk.is_trailers() { "TRAILERS" } else { "OTHER" });

        let processing_request = if let Some(bytes) = chunk.data_mut() {
            // DATA
            let data = std::mem::take(bytes);

            let http_body = HttpBody {
                body: data.into(),
                end_of_stream,
                end_of_stream_without_message: false,
                grpc_message_compressed: false,
            };
            if Msg::IS_REQUEST {
                ProcessingRequest {
                    request: Some(ProcessingRequestType::RequestBody(http_body)),
                    metadata_context: None,
                    attributes: HashMap::default(),
                    observability_mode: M::OBSERVABILITY,
                    protocol_config: None,
                }
            } else {
                ProcessingRequest {
                    request: Some(ProcessingRequestType::ResponseBody(http_body)),
                    metadata_context: None,
                    attributes: HashMap::default(),
                    observability_mode: M::OBSERVABILITY,
                    protocol_config: None,
                }
            }
        } else if let Some(traiers) = chunk.trailers_mut() {
            // TRAILERS
            let data = std::mem::take(traiers);

            // store trailers for potential update later
            self.trailers = Some(data.clone());

            let envoy_trailers: EnvoyHeaderMap = (&data).into();

            if Msg::IS_REQUEST {
                ProcessingRequest {
                    request: Some(ProcessingRequestType::RequestTrailers(HttpTrailers {
                        trailers: Some(envoy_trailers.0),
                    })),
                    metadata_context: None,
                    attributes: HashMap::default(),
                    observability_mode: M::OBSERVABILITY,
                    protocol_config: None,
                }
            } else {
                ProcessingRequest {
                    request: Some(ProcessingRequestType::ResponseTrailers(HttpTrailers {
                        trailers: Some(envoy_trailers.0),
                    })),
                    metadata_context: None,
                    attributes: HashMap::default(),
                    observability_mode: M::OBSERVABILITY,
                    protocol_config: None,
                }
            }
        } else {
            let msg = "handle_body_chunk: unexpected non-data frame to send";
            debug!(target: "ext_proc", msg);
            return Action::Return(self.status_error(msg, self.failure_mode_allow));
        };

        debug!(target: "ext_proc", "handle_body_chunk: prepared processing_request: {processing_request:?}");
        Action::Send(processing_request)
    }

    pub fn status_timeout(&mut self, failure_mode_allow: bool) -> ProcessingStatus {
        if failure_mode_allow {
            ProcessingStatus::HaltedOnError
        } else {
            let http_version = self.http_version.unwrap_or(http::Version::HTTP_11);
            ProcessingStatus::EndWithDirectResponse(
                SyntheticHttpResponse::gateway_timeout(
                    EventKind::ExtProcError.into(),
                    ResponseFlags(FmtResponseFlags::UPSTREAM_REQUEST_TIMEOUT),
                )
                .into_response(http_version),
            )
        }
    }

    pub fn status_error(&mut self, msg: &str, failure_mode_allow: bool) -> ProcessingStatus {
        if failure_mode_allow {
            ProcessingStatus::HaltedOnError
        } else {
            let http_version = self.http_version.unwrap_or(http::Version::HTTP_11);
            ProcessingStatus::EndWithDirectResponse(
                SyntheticHttpResponse::internal_error_with_msg(
                    msg,
                    EventKind::ExtProcError.into(),
                    ResponseFlags(FmtResponseFlags::NO_FILTER_CONFIG_FOUND),
                )
                .into_response(http_version),
            )
        }
    }

    pub fn status_internal_error(&mut self, msg: &str) -> ProcessingStatus {
        let http_version = self.http_version.unwrap_or(http::Version::HTTP_11);
        ProcessingStatus::EndWithDirectResponse(
            SyntheticHttpResponse::internal_error_with_msg(
                msg,
                EventKind::ExtProcError.into(),
                ResponseFlags(FmtResponseFlags::UNAUTHORIZED_EXTERNAL_SERVICE),
            )
            .into_response(http_version),
        )
    }

    pub async fn frame_bridge_close(&mut self, timeout_active: &mut bool) {
        *timeout_active = false;
        self.streaming_body_enabled = false;
        if let Some(trailers) = self.parked_trailers.take() {
            _ = self.frame_bridge.inject_frame(Ok(trailers)).await;
        }
        self.frame_bridge.close();
    }
}
