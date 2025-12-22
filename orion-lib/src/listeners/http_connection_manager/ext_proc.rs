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

mod kind;
mod mutation;
mod r#override;
mod processing;
mod status;
#[cfg(test)]
mod tests;
mod worker_config;

use crate::body::{
    body_with_metrics::BodyWithMetrics,
    channel_body::{ChannelBody, FrameBridge},
};
use crate::event_error::EventKind;
use http_body_util::{BodyExt, Collected, LengthLimitError, Limited};

use crate::listeners::http_connection_manager::ext_proc::mutation::{
    apply_request_header_mutations, apply_response_header_mutations,
};
use crate::listeners::http_connection_manager::ext_proc::processing::RequestProcessing;
use crate::listeners::http_connection_manager::ext_proc::processing::ResponseProcessing;
use crate::listeners::http_connection_manager::ext_proc::r#override::{OverridableBodyMode, OverridableGlobalModes};
use crate::listeners::http_connection_manager::ext_proc::status::Action;
use crate::listeners::http_connection_manager::ext_proc::status::ProcessingStatus;
use crate::listeners::http_connection_manager::ext_proc::status::ReadyStatus;
use crate::listeners::http_connection_manager::ext_proc::worker_config::ExternalProcessingWorkerConfig;
use crate::{
    body::response_flags::ResponseFlags,
    clusters::clusters_manager::{self, RoutingContext},
    listeners::{http_connection_manager::FilterDecision, synthetic_http_response::SyntheticHttpResponse},
    Error, PolyBody,
};
use bytes::Bytes;
use futures::{future::Either, StreamExt};
use http::header::CONTENT_LENGTH;
use http::{Request, Response, StatusCode};
use http_body::{Body, Frame};
use http_body_util::Full;
use orion_configuration::config::{
    cluster::ClusterSpecifier,
    network_filters::http_connection_manager::http_filters::{
        ext_proc::{
            ExternalProcessor as ExternalProcessorConfig, GrpcServiceSpecifier, HeaderForwardingRules, ProcessingMode,
        },
        ExtProcPerRoute,
    },
};
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::{
        config::core::v3::{HeaderMap, HeaderValue},
        service::ext_proc::v3::{
            external_processor_client::ExternalProcessorClient,
            processing_response::Response as ProcessingResponseType, ImmediateResponse, ProcessingRequest,
            ProcessingResponse, ProtocolConfiguration,
        },
    },
    google,
    tonic::Status,
};
use orion_format::types::ResponseFlags as FmtResponseFlags;
use pingora_timeout::fast_timeout;
use scopeguard::defer;
use std::convert::Infallible;
use std::future::ready;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

const EXT_PROC_BUFFERED_BODY_LIMIT: usize = 4 * 1024 * 1024; // this is the default limit for GRPC payload lenght

#[derive(Debug, Clone, thiserror::Error)]
pub enum ExtProcError {
    #[error("Timeout Error: {0}")]
    Timeout(&'static str),
    #[error("Unsupported Response Type: {0}")]
    UnsupportedResponseType(String),
    #[error("Connection closed by remote GRPC")]
    UnexpectedEof,
    #[error("GRPC error: {0}")]
    GrpcError(String),
}

#[derive(Debug, Clone)]
pub struct ExternalProcessor {
    ext_proc_worker: Option<mpsc::Sender<ProcessingTask>>,
    worker_config: Arc<ExternalProcessingWorkerConfig>,
    forward_rules: Option<Arc<HeaderForwardingRules>>,
    overridable_modes: Arc<OverridableGlobalModes>,
}

impl From<ExternalProcessorConfig> for ExternalProcessor {
    fn from(initial_config: ExternalProcessorConfig) -> Self {
        debug!(target: "ext_proc", "From<ExternalProcessorConfig> for ExternalProcessor");
        Self::from((initial_config, None))
    }
}

impl From<(ExternalProcessorConfig, Option<ExtProcPerRoute>)> for ExternalProcessor {
    fn from((initial_config, per_route_config): (ExternalProcessorConfig, Option<ExtProcPerRoute>)) -> Self {
        debug!(target: "ext_proc", "From<ExternalProcessorConfig, Option<ExtProcPerRoute>> for ExternalProcessor");
        let forward_rules = initial_config.forward_rules.clone().map(Arc::new);
        let worker_config = ExternalProcessingWorkerConfig::from((initial_config, per_route_config));

        let overridable_global_modes = Arc::new(OverridableGlobalModes::from(&worker_config));

        Self {
            ext_proc_worker: None,
            worker_config: Arc::new(worker_config),
            forward_rules,
            overridable_modes: overridable_global_modes,
        }
    }
}

impl Drop for ExternalProcessor {
    fn drop(&mut self) {
        if let Some(sender) = self.ext_proc_worker.take() {
            debug!(target: "ext_proc", "ExternalProcessor::drop (closing sender)");
            drop(sender);
        }
    }
}

impl From<(ExternalProcessorConfig, Option<ExtProcPerRoute>)> for ExternalProcessingWorkerConfig {
    fn from((config, per_route_config): (ExternalProcessorConfig, Option<ExtProcPerRoute>)) -> Self {
        debug!(target: "ext_proc", "From<(ExternalProcessorConfig, Option<ExtProcPerRoute>)> for ExternalProcessingWorkerConfig");
        let mut processing_mode = config.processing_mode.clone().unwrap_or(ProcessingMode::default());
        let mut grpc_service = config.grpc_service;
        let mut failure_mode_allow = config.failure_mode_allow;
        if let Some(per_route) = per_route_config {
            if !per_route.disabled {
                if let Some(overrides) = per_route.overrides {
                    if let Some(override_processing_mode) = overrides.processing_mode {
                        processing_mode = override_processing_mode;
                    }
                    if let Some(override_grpc_service) = overrides.grpc_service {
                        grpc_service = override_grpc_service;
                    }
                    if let Some(override_failure_mode_allow) = overrides.failure_mode_allow {
                        failure_mode_allow = override_failure_mode_allow;
                    }
                }
            }
        }

        Self {
            grpc_service_specifier: grpc_service.clone().service_specifier,
            message_timeout: config.message_timeout.unwrap_or(Duration::from_millis(200)),
            max_message_timeout: config.max_message_timeout,
            observability_mode: config.observability_mode,
            failure_mode_allow,
            disable_immediate_response: config.disable_immediate_response,
            mutation_rules: config.mutation_rules,
            processing_mode,
            allowed_override_modes: config.allowed_override_modes,
            allow_mode_override: config.allow_mode_override,
            route_cache_action: config.route_cache_action,
            send_body_without_waiting_for_header_response: config.send_body_without_waiting_for_header_response,
        }
    }
}

macro_rules! run_action {
    // $self: The 'self' instance.
    // $processor: The specific processing struct (e.g., self.request_processing or self.response_processing).
    // $action: The Action to process.
    // $ctx: A string literal context for logging.
    ($self:ident, $processor:expr, $action:expr, $ctx:expr) => {
        match $action {
            Action::Send(outbound) => {
                debug!(target: "ext_proc", "{ctx} @{typ}: action -> forward {outbound:?}",
                    ctx = $ctx,
                    typ = stringify!($processor),
                    outbound = outbound
                );

                $self.forward_to_external_processor(outbound).await;
            },
            Action::Return(status) => {
                if let Some(reply_channel) = $processor.reply_channel.take() {
                    debug!(target: "ext_proc", "{ctx} @{typ}: action -> return {status:?}",
                        ctx = $ctx,
                        typ = stringify!($processor),
                        status = status);

                    let _ = reply_channel.send(status);
                }
            },
        }
    };
}

macro_rules! with_current_processing {
    (
        $self:ident,
        $processing:ident,
        $($body:block)+
    ) => {
        if $self.request_processing.reply_channel.is_some() {
            $(
                {
                    let $processing = &mut $self.request_processing;
                    $body
                }
            )+
        } else {
            $(
                {
                    let $processing = &mut $self.response_processing;
                    $body
                }
            )+
        }
    };
}

impl ExternalProcessor {
    async fn to_buffered(original: Collected<Bytes>) -> Collected<Bytes> {
        let trailers = original.trailers().cloned().map(Ok::<_, Infallible>);
        let aggregated_bytes = original.to_bytes();
        Full::new(aggregated_bytes).with_trailers(ready(trailers)).collect().await.unwrap()
    }

    #[allow(clippy::too_many_lines)]
    pub async fn apply_request(&mut self, request: &mut Request<BodyWithMetrics<PolyBody>>) -> FilterDecision {
        let modes = &self.overridable_modes.request;
        let process_headers = modes.should_process_headers();
        let process_body = modes.should_process_body();
        let process_trailers = modes.should_process_trailers();
        debug!("Processing ext_proc");
        if !process_headers && !process_body && !process_trailers {
            return FilterDecision::Continue;
        }

        let mut ext_proc_headers = None;

        if process_headers {
            debug!("ext_proc request processing headers");
            ext_proc_headers = Some(self.filter_header_map(request.headers()));
        }

        let body: PolyBody = std::mem::take(&mut request.body_mut().inner);

        let ext_proc_frame_bridge = match (modes.body_mode(), modes.trailer_mode()) {
            (OverridableBodyMode::None, trailers_mode) => {
                // event though body processing is None and trailers processing is Skip, we have to
                // create the bridge, to allow ext_proc mutate the body with ContinueAndReplace action.
                debug!(target: "ext_proc", "request processing body(None) and trailers:{trailers_mode:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                request.body_mut().inner = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Streamed | OverridableBodyMode::FullDuplexStreamed, trailers_mode) => {
                debug!(target: "ext_proc", "request processing body(Streamed) and trailers:{trailers_mode:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                request.body_mut().inner = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Buffered | OverridableBodyMode::BufferedPartial, trailers_mode) => {
                debug!(target: "ext_proc", "request processing body(Buffered) with trailers:{trailers_mode:?}");
                if body.is_end_stream() {
                    let (new_body, bridge) = ChannelBody::new(body);
                    request.body_mut().inner = PolyBody::from(new_body);
                    bridge
                } else {
                    let body = Limited::new(body, EXT_PROC_BUFFERED_BODY_LIMIT);
                    let collected = match body.collect().await {
                        Ok(collected) => collected,
                        Err(e) => {
                            if let Some(_) = e.downcast_ref::<LengthLimitError>() {
                                return self.on_filter_error(
                                    &format!("Request body: {}", e),
                                    None,
                                    request.version(),
                                    Some(StatusCode::PAYLOAD_TOO_LARGE),
                                );
                            }
                            return self.on_filter_error(
                                &format!("Error collecting request body: {}", e),
                                None,
                                request.version(),
                                None,
                            );
                        },
                    };

                    let buffered = Self::to_buffered(collected).await;
                    let (new_body, bridge) = ChannelBody::new(buffered);
                    request.body_mut().inner = PolyBody::from(new_body);
                    bridge
                }
            },
        };

        debug!(target: "ext_proc", "request headers: {ext_proc_headers:?}");
        debug!(target: "ext_proc", "request body: {ext_proc_frame_bridge:?}");

        let processing_data = ProcessingData::Request(ext_proc_headers, ext_proc_frame_bridge);

        let ver = request.version();
        let Ok(response_rx) = self.send_processing_data(processing_data, ver).await else {
            return self.on_filter_error(
                "Failed to schedule sending request data to external processor",
                None,
                ver,
                None,
            );
        };

        let res = match response_rx.await {
            Ok(ProcessingStatus::HaltedOnError) => {
                debug!(target: "ext_proc", "apply_request: HaltedOnError...");
                FilterDecision::Continue
            },
            Ok(ProcessingStatus::EndWithDirectResponse(direct_response)) => {
                debug!(target: "ext_proc", "apply_request: DirectResponse...");
                FilterDecision::DirectResponse(direct_response)
            },
            Ok(ProcessingStatus::RequestReady(ReadyStatus { clear_route_cache, headers_modifications })) => {
                debug!(target: "ext_proc", "apply_request: RequestReady Status{{ clear_route_cache:{clear_route_cache:?}, headers_modifications:{headers_modifications:?} }}...");
                if let Some(headers_modifications) = headers_modifications {
                    debug!(target: "ext_proc", "applying headers mutation...");
                    if let Err(e) = apply_request_header_mutations(
                        request,
                        &headers_modifications,
                        self.worker_config.mutation_rules.as_ref(),
                    ) {
                        return self.on_filter_error(
                            "Invalid header modifications received from external processor",
                            Some(e),
                            request.version(),
                            None,
                        );
                    }
                }

                // at this point it's hard to know if body replacement is being requested or not.
                // So we just remove the content-length header to be safe.

                request.headers_mut().remove(CONTENT_LENGTH);

                if clear_route_cache {
                    return FilterDecision::Reroute;
                }
                FilterDecision::Continue
            },
            Ok(ProcessingStatus::ResponseReady(ReadyStatus { .. })) => {
                warn!(target: "ext_proc", "apply_request: unexpected ResponseReady!");
                return self.on_filter_error(
                    "Unexpected ResponseReady status received during request processing",
                    None,
                    request.version(),
                    None,
                );
            },
            Err(e) => self.on_filter_error(
                format!("External processor: {e:?}").as_str(),
                Some(e.into()),
                request.version(),
                None,
            ),
        };

        debug!(target: "ext_proc", "apply_request completed: {res:?}!");
        request.body_mut().inner.wait_frame().await;
        res
    }

    #[allow(clippy::too_many_lines)]
    pub async fn apply_response(&mut self, response: &mut Response<PolyBody>) -> FilterDecision {
        let modes = &self.overridable_modes.response;
        let process_headers = modes.should_process_headers();
        let process_body = modes.should_process_body();
        let process_trailers = modes.should_process_trailers();

        if !process_headers && !process_body && !process_trailers {
            return FilterDecision::Continue;
        }

        let mut ext_proc_headers = None;

        if process_headers {
            debug!(target: "ext_proc", "response processing headers");
            ext_proc_headers = Some(self.filter_header_map(response.headers()));
        }

        let body: PolyBody = std::mem::take(&mut response.body_mut());

        let ext_proc_frame_bridge = match (modes.body_mode(), modes.trailer_mode()) {
            (OverridableBodyMode::None, trailers_mode) => {
                // event though body processing is None and trailers processing is Skip, we have to
                // create the bridge, to allow ext_proc mutate the body with ContinueAndReplace action.
                debug!(target: "ext_proc", "response processing body(None) and trailers:{trailers_mode:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                *response.body_mut() = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Streamed | OverridableBodyMode::FullDuplexStreamed, trailers_mode) => {
                debug!(target: "ext_proc", "response processing body(Streamed) and trailers:{trailers_mode:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                *response.body_mut() = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Buffered | OverridableBodyMode::BufferedPartial, trailers_mode) => {
                debug!(target: "ext_proc", "response processing body(Buffered) with trailers:{trailers_mode:?}");
                if body.is_end_stream() {
                    let (new_body, bridge) = ChannelBody::new(body);
                    *response.body_mut() = PolyBody::from(new_body);
                    bridge
                } else {
                    let body = Limited::new(body, EXT_PROC_BUFFERED_BODY_LIMIT);
                    let collected = match body.collect().await {
                        Ok(collected) => collected,
                        Err(e) => {
                            if let Some(_) = e.downcast_ref::<LengthLimitError>() {
                                return self.on_filter_error(
                                    &format!("Response body: {}", e),
                                    None,
                                    response.version(),
                                    Some(StatusCode::PAYLOAD_TOO_LARGE),
                                );
                            }
                            return self.on_filter_error(
                                &format!("Error collecting response body: {}", e),
                                None,
                                response.version(),
                                None,
                            );
                        },
                    };

                    let buffered = Self::to_buffered(collected).await;
                    let (new_body, bridge) = ChannelBody::new(buffered);
                    *response.body_mut() = PolyBody::from(new_body);
                    bridge
                }
            },
        };

        debug!(target: "ext_proc", "response headers: {ext_proc_headers:?}");
        debug!(target: "ext_proc", "response body: {ext_proc_frame_bridge:?}");

        let processing_data = ProcessingData::Response(ext_proc_headers, ext_proc_frame_bridge);

        let ver = response.version();
        let Ok(response_rx) = self.send_processing_data(processing_data, ver).await else {
            return self.on_filter_error(
                "Failed to schedule sending response data to external processor",
                None,
                ver,
                None,
            );
        };

        let res = match response_rx.await {
            Ok(ProcessingStatus::HaltedOnError) => {
                debug!(target: "ext_proc", "apply_response: HaltedOnError...");
                FilterDecision::Continue
            },
            Ok(ProcessingStatus::EndWithDirectResponse(direct_response)) => {
                debug!(target: "ext_proc", "apply_response: DirectResponse...");
                FilterDecision::DirectResponse(direct_response)
            },
            Ok(ProcessingStatus::ResponseReady(ReadyStatus { clear_route_cache, headers_modifications })) => {
                debug!(target: "ext_proc", "apply_response: ResponseReady...");
                if let Some(headers_modifications) = headers_modifications {
                    debug!(target: "ext_proc", "applying headers mutation...");
                    if let Err(e) = apply_response_header_mutations(
                        response,
                        &headers_modifications,
                        self.worker_config.mutation_rules.as_ref(),
                    ) {
                        return self.on_filter_error(
                            "Invalid header modifications received from external processor",
                            Some(e),
                            response.version(),
                            None,
                        );
                    }
                }

                response.headers_mut().remove(CONTENT_LENGTH);

                if clear_route_cache {
                    return FilterDecision::Reroute;
                }
                FilterDecision::Continue
            },
            Ok(ProcessingStatus::RequestReady(ReadyStatus { .. })) => {
                warn!(target: "ext_proc", "apply_response: unexpected RequestReady!");
                return self.on_filter_error(
                    "Unexpected RequestReady status received during response processing",
                    None,
                    response.version(),
                    None,
                );
            },
            Err(e) => self.on_filter_error(
                format!("External processor response processing: {e:?}").as_str(),
                Some(e.into()),
                response.version(),
                None,
            ),
        };

        debug!(target: "ext_proc", "apply_response completed: {res:?}!");

        // Delay sending the response until the first frame is ready. This ensures
        // better performance when streaming bodies from the external processor.
        // It works around a limitation in Tokio and Hyper, which perform poorly
        // when the response body is not immediately available. By waiting for
        // the first frame, single-frame responses avoid unnecessary polling
        // cycles.

        response.body_mut().wait_frame().await;
        res
    }

    async fn send_processing_data(
        &mut self,
        data: ProcessingData,
        ver: http::Version,
    ) -> Result<oneshot::Receiver<ProcessingStatus>, SendError<ProcessingTask>> {
        let (response_tx, response_rx) = oneshot::channel();
        let processing_message = ProcessingTask { data, reply_channel: response_tx, http_version: ver };

        let worker_channel = self.get_worker_channel();
        worker_channel.send(processing_message).await.map(|()| response_rx)
    }

    fn on_filter_error(
        &mut self,
        msg: &str,
        error: Option<Error>,
        http_version: http::Version,
        status_code: Option<StatusCode>,
    ) -> FilterDecision {
        if let Some(err) = error {
            info!(target: "ext_proc","{msg}: {err}");
        } else {
            info!(target: "ext_proc", "{msg}");
        }
        if self.worker_config.failure_mode_allow {
            FilterDecision::Continue
        } else {
            FilterDecision::DirectResponse(
                SyntheticHttpResponse::custom_error(
                    status_code.unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    EventKind::ExtProcError.into(),
                    ResponseFlags(FmtResponseFlags::UPSTREAM_CONNECTION_FAILURE),
                )
                .into_response(http_version),
            )
        }
    }

    fn get_worker_channel(&mut self) -> &mpsc::Sender<ProcessingTask> {
        if let Some(ref sender) = self.ext_proc_worker {
            sender
        } else {
            let (sender, receiver) = mpsc::channel::<ProcessingTask>(12);

            // replace the internal overridable modes blueprint with a new spawned instance for the worker
            //

            let overridable_modes = Arc::new(self.overridable_modes.spawn());
            self.overridable_modes = Arc::clone(&overridable_modes);

            if self.worker_config.observability_mode {
                let worker = ExternalProcessingWorker::<kind::Observability>::new(
                    Arc::clone(&self.worker_config),
                    overridable_modes,
                );
                tokio::spawn(worker.observability_loop(receiver));
            } else {
                let worker = ExternalProcessingWorker::<kind::Processing>::new(
                    Arc::clone(&self.worker_config),
                    overridable_modes,
                );
                tokio::spawn(worker.processing_loop(receiver));
            }
            self.ext_proc_worker.insert(sender)
        }
    }

    #[inline]
    fn should_forward_header(&self, header_name: &str) -> bool {
        if let Some(forward_rules) = &self.forward_rules {
            if !forward_rules.disallowed_headers.is_empty() {
                return !forward_rules.disallowed_headers.iter().any(|m| m.matches(header_name));
            }
            if !forward_rules.allowed_headers.is_empty() {
                return forward_rules.allowed_headers.iter().any(|m| m.matches(header_name));
            }
        }
        true
    }

    fn filter_header_map(&self, headers: &http::HeaderMap) -> http::HeaderMap {
        let Some(rules) = &self.forward_rules else {
            return headers.clone();
        };

        if rules.allowed_headers.is_empty() && rules.disallowed_headers.is_empty() {
            return headers.clone();
        }

        let mut filtered_headers = http::HeaderMap::with_capacity(headers.len());
        for (name, value) in headers {
            if self.should_forward_header(name.as_str()) {
                filtered_headers.append(name.clone(), value.clone());
            }
        }
        filtered_headers
    }
}

struct EnvoyHeaderMap(HeaderMap);
impl From<&http::HeaderMap> for EnvoyHeaderMap {
    fn from(headers: &http::HeaderMap) -> Self {
        let mut headers_vec = Vec::with_capacity(headers.len());
        for (name, value) in headers {
            let header_name = name.as_str();
            let header_value = if let Ok(value_str) = value.to_str() {
                HeaderValue { key: header_name.to_owned(), value: value_str.to_owned(), raw_value: Vec::new() }
            } else {
                HeaderValue {
                    key: header_name.to_owned(),
                    value: String::default(),
                    raw_value: value.as_bytes().into(),
                }
            };
            headers_vec.push(header_value);
        }
        EnvoyHeaderMap(HeaderMap { headers: headers_vec })
    }
}

impl From<EnvoyHeaderMap> for http::HeaderMap {
    fn from(envoy_headers: EnvoyHeaderMap) -> Self {
        let mut headers = http::HeaderMap::with_capacity(envoy_headers.0.headers.len());

        for header in envoy_headers.0.headers {
            let Ok(header_name) = http::header::HeaderName::from_bytes(header.key.as_bytes()) else { continue };

            let header_value = if header.value.is_empty() {
                match http::header::HeaderValue::from_maybe_shared(header.raw_value) {
                    Ok(value) => value,
                    Err(_) => continue,
                }
            } else {
                match http::header::HeaderValue::from_maybe_shared(header.value) {
                    Ok(value) => value,
                    Err(_) => continue,
                }
            };

            headers.append(header_name, header_value);
        }

        headers
    }
}

#[derive(Debug)]
struct ProcessingTask {
    data: ProcessingData,
    reply_channel: oneshot::Sender<ProcessingStatus>,
    http_version: http::Version,
}

#[derive(Debug)]
enum ProcessingData {
    Request(Option<http::HeaderMap>, FrameBridge),
    Response(Option<http::HeaderMap>, FrameBridge),
}

struct BidiStream {
    external_sender: mpsc::Sender<ProcessingRequest>,
    inbound_responses: mpsc::Receiver<Result<ProcessingResponse, Status>>,
}

struct TimeoutState {
    duration: Duration,
    active: bool,
    extended: bool,
}

struct ExternalProcessingWorker<S: kind::Mode> {
    config: Arc<ExternalProcessingWorkerConfig>,
    bidi_stream: Option<BidiStream>,
    request_processing: RequestProcessing<S>,
    response_processing: ResponseProcessing<S>,
    handshake: Option<ProtocolConfiguration>,
    timeout_state: TimeoutState,
    overridable_modes: Arc<OverridableGlobalModes>,
}

fn clone_frame(frame: &Frame<Bytes>) -> Frame<Bytes> {
    if let Some(data) = frame.data_ref() {
        Frame::data(data.clone())
    } else if let Some(trailers) = frame.trailers_ref() {
        Frame::trailers(trailers.clone())
    } else {
        // empty frame as fallback
        Frame::data(Bytes::new())
    }
}

#[allow(dead_code)]
enum MergeResult {
    Retry(u32),
    Error(u32),
    None(u32),
}

impl ExternalProcessingWorker<kind::Processing> {
    fn new(config: Arc<ExternalProcessingWorkerConfig>, overridable_global_modes: Arc<OverridableGlobalModes>) -> Self {
        let request_processing = RequestProcessing::<kind::Processing>::from(&*config);
        let response_processing = ResponseProcessing::<kind::Processing>::from(&*config);
        let handshake = Some(ProtocolConfiguration {
            request_body_mode: config.processing_mode.request_body_mode as i32,
            response_body_mode: config.processing_mode.response_body_mode as i32,
            send_body_without_waiting_for_header_response: config.send_body_without_waiting_for_header_response,
        });
        let message_timeout = config.message_timeout;
        Self {
            config,
            bidi_stream: None,
            request_processing,
            response_processing,
            handshake,
            timeout_state: TimeoutState { active: false, duration: message_timeout, extended: false },
            overridable_modes: overridable_global_modes,
        }
    }

    async fn recover_or_failure(&mut self, err: ExtProcError, log_msg: &str) {
        if self.config.failure_mode_allow {
            info!(target: "ext_proc", "{} - continue (failure_mode_allow is true)", log_msg);
            self.request_processing.inject_inflight_frames_and_complete().await;
            self.request_processing.frame_bridge_close(&mut self.timeout_state.active).await;
            self.response_processing.inject_inflight_frames_and_complete().await;
            self.response_processing.frame_bridge_close(&mut self.timeout_state.active).await;
        } else {
            info!(target: "ext_proc", "{} - abort (failure_mode_allow is false)", log_msg);
            _ = self.request_processing.frame_bridge.inject_frame(Err(Box::new(err.clone()))).await;
            self.request_processing.frame_bridge_close(&mut self.timeout_state.active).await;
            _ = self.response_processing.frame_bridge.inject_frame(Err(Box::new(err.clone()))).await;
            self.response_processing.frame_bridge_close(&mut self.timeout_state.active).await;
        }

        match err {
            ExtProcError::Timeout(_) => {
                let status = Action::Return(self.request_processing.status_timeout(self.config.failure_mode_allow));
                run_action!(self, self.request_processing, status, log_msg);
                let status = Action::Return(self.response_processing.status_timeout(self.config.failure_mode_allow));
                run_action!(self, self.response_processing, status, log_msg);
            },
            _ => {
                let status =
                    Action::Return(self.request_processing.status_error(log_msg, self.config.failure_mode_allow));
                run_action!(self, self.request_processing, status, log_msg);
                let status =
                    Action::Return(self.response_processing.status_error(log_msg, self.config.failure_mode_allow));
                run_action!(self, self.response_processing, status, log_msg);
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn processing_loop(mut self, mut processing_request_channel: mpsc::Receiver<ProcessingTask>) {
        debug!(target: "ext_proc", "===== BEGIN =====");

        defer! {
            debug!(target: "ext_proc", "===== END =====");
        }

        let mut request_body_to_ext_proc_complete = false;
        let mut response_body_to_ext_proc_complete = false;

        'transaction_loop: loop {
            let streaming_enabled =
                self.request_processing.streaming_body_enabled || self.response_processing.streaming_body_enabled;
            let outbound_req_enabled =
                self.request_processing.streaming_body_enabled && !request_body_to_ext_proc_complete;
            let outbound_resp_enabled =
                self.response_processing.streaming_body_enabled && !response_body_to_ext_proc_complete;

            debug!(target: "ext_proc", "----- select! [ process_task:{} inbound:true outbound_req:{outbound_req_enabled} outbound_resp:{outbound_resp_enabled} timeout:{} ] -----",
                !streaming_enabled, self.timeout_state.active);

            tokio::select! {
                outbound_processing_request = processing_request_channel.recv(), if !streaming_enabled => {
                    debug!(target: "ext_proc", "processing {outbound_processing_request:?}...");
                    match outbound_processing_request {
                        Some(ProcessingTask{ data: ProcessingData::Request(headers, frame_bridge), reply_channel, http_version}) => {
                            let action = self.request_processing.process(headers, frame_bridge, reply_channel, http_version, &self.overridable_modes);
                            run_action!(self, self.request_processing, action, "process_request");
                        }
                        Some(ProcessingTask{ data: ProcessingData::Response(headers, frame_bridge), reply_channel, http_version}) => {
                            let action = self.response_processing.process(headers, frame_bridge, reply_channel, http_version, &self.overridable_modes);
                            run_action!(self, self.response_processing, action, "process_response");
                        }
                        _ => {
                            debug!(target: "ext_proc", ">> worker channel closed!");
                            break 'transaction_loop
                        },
                    }
                },

                inbound_processing_response = if let Some(stream) = self.bidi_stream.as_mut() {
                        Either::Left(stream.inbound_responses.recv())
                    } else {
                        Either::Right(std::future::pending::<Option<Result<ProcessingResponse, Status>>>())
                    } => {

                    debug!(target: "ext_proc", "<= inbound processing response: {inbound_processing_response:?}");

                    match inbound_processing_response {
                        Some(Ok(ProcessingResponse { override_message_timeout: Some(extended_timeout), ..})) => {
                            debug!(target: "ext_proc", "<- timeout extension received: {extended_timeout:?}");
                            if !self.handle_timeout_extension(extended_timeout) {
                                debug!(target: "ext_proc", "invalid timeout extension - closing stream");
                                break 'transaction_loop;
                            }
                        },
                        Some(Ok(ProcessingResponse { response: Some(ProcessingResponseType::ImmediateResponse(response_attempt)), ..})) => {
                            debug!(target: "ext_proc", "<- ImmediateResponse received");
                            if self.config.disable_immediate_response {
                                info!(target: "ext_proc", "External processor attempted to send immediate response which is disabled by config");

                                 if self.config.failure_mode_allow {
                                     with_current_processing!(self, processing,
                                     {
                                         let status = Action::Return(ProcessingStatus::HaltedOnError);
                                         run_action!(self, processing, status, "immediate_response_disabled");
                                     }
                                     {
                                         processing.inject_inflight_frames_and_complete().await;
                                     });

                                 } else {
                                    with_current_processing!(self, processing,
                                     {
                                         let action = Action::Return(processing.status_internal_error("ext_proc returned an immediate response despite being disabled by configuration"));
                                         run_action!(self, processing, action, "immediate_response_disabled");
                                     }
                                     {
                                         _ = processing.frame_bridge.inject_frame(Err(Box::new(ExtProcError::Timeout("immediate response disabled")))).await;
                                     }
                                    );
                                 }

                            } else {
                                let response = self.build_direct_response(&response_attempt);
                                with_current_processing!(self, processing, {
                                    let status = Action::Return(ProcessingStatus::EndWithDirectResponse(response));
                                    run_action!(self, processing, status, "immediate_response_disabled");
                                });
                            }

                            with_current_processing!(self, processing,
                            {
                                 processing.inject_inflight_frames_and_complete().await;
                            });

                            break 'transaction_loop;
                        },
                        Some(Ok(ProcessingResponse { mode_override, response: Some(ProcessingResponseType::RequestHeaders(headers_response)), ..})) => {
                            debug!(target: "ext_proc", "<- RequestHeaders response received");
                            if self.config.allow_mode_override {
                                if let Some(overrides) = mode_override {
                                    self.request_processing.apply_mode_overrides(&overrides, &self.config.allowed_override_modes, &self.overridable_modes);
                                    self.response_processing.apply_mode_overrides(&overrides, &self.config.allowed_override_modes, &self.overridable_modes);
                                }
                            }

                            let action = self.request_processing.handle_headers_response(
                                headers_response,
                                &self.config.route_cache_action,
                                &self.overridable_modes,
                                &mut self.timeout_state.active,
                            ).await;

                            run_action!(self, self.request_processing, action, "handle_headers_response");
                        },
                        Some(Ok(ProcessingResponse { response: Some(ProcessingResponseType::RequestBody(body_response)), ..})) => {
                            debug!(target: "ext_proc", "<- RequestBody response received");

                            let action = self
                                .request_processing
                                .handle_body_response(body_response, Some(&self.config.route_cache_action), &mut self.timeout_state.active).await;

                            run_action!(self, self.request_processing, action, "body_response");
                        },
                        Some(Ok(ProcessingResponse { response: Some(ProcessingResponseType::RequestTrailers(trailers_response)), ..})) => {
                            debug!(target: "ext_proc", "<- RequestTrailers response received");
                            let action = self.request_processing.handle_trailers_response(trailers_response, &mut self.timeout_state.active).await;
                            run_action!(self, self.request_processing, action, "handle_trailers_response");
                        },
                        Some(Ok(ProcessingResponse { mode_override, response: Some(ProcessingResponseType::ResponseHeaders(headers_response)), ..})) => {
                            debug!(target: "ext_proc", "<- ResponseHeaders response received");
                            if self.config.allow_mode_override {
                                if let Some(overrides) = mode_override {
                                    self.response_processing.apply_mode_overrides(&overrides, &self.config.allowed_override_modes, &self.overridable_modes);
                                }
                            }

                            let action = self.response_processing.handle_headers_response(
                                headers_response,
                                &self.config.route_cache_action,
                                &self.overridable_modes,
                                &mut self.timeout_state.active,
                            ).await;

                            run_action!(self, self.response_processing, action, "handle_headers_response");
                        },
                        Some(Ok(ProcessingResponse { response: Some(ProcessingResponseType::ResponseBody(body_response)), ..})) => {
                            debug!(target: "ext_proc", "<- ResponseBody response received");
                            let empty_response = body_response.response.is_none();
                            let action = self.response_processing.handle_body_response(body_response, None, &mut self.timeout_state.active).await;
                            run_action!(self, self.response_processing, action, "handle_body_response");

                            if empty_response {
                                debug!(target: "ext_proc", "response body response contained no response - closing stream");
                                break 'transaction_loop;
                            }
                        },
                        Some(Ok(ProcessingResponse { response: Some(ProcessingResponseType::ResponseTrailers(trailers_response)), ..})) => {
                            debug!(target: "ext_proc", "<- ResponseTrailers response received");
                            let action = self.response_processing.handle_trailers_response(trailers_response, &mut self.timeout_state.active).await;
                            run_action!(self, self.response_processing, action, "handle_trailers_response");
                        },
                        Some(Ok(r)) => {
                            let msg = format!("unsupported response message received: {r:?}");
                            self.recover_or_failure(ExtProcError::UnsupportedResponseType(format!("{r:?}")), &msg).await;
                            break 'transaction_loop;
                        }
                        None => {
                            let msg = "stream closed by the external processor";
                            self.recover_or_failure(ExtProcError::UnexpectedEof, msg).await;
                            break 'transaction_loop;
                        },
                        Some(Err(e)) => {
                            let msg = format!("gRPC error received from external processor: {}", e.message());
                            self.recover_or_failure(ExtProcError::GrpcError(e.message().into()), &msg).await;
                            break 'transaction_loop;
                        }
                    }
                },

                outbound_request_body_frame = self.request_processing.frame_bridge.next(), if outbound_req_enabled => {
                    debug!(target: "ext_proc", "outbound request body frame: {outbound_request_body_frame:?}");
                    match outbound_request_body_frame {
                        Some(Ok(frame)) => {
                            let body_mode = self.overridable_modes.request.body_mode();

                            debug!(target: "ext_proc", "outbound request body frame: buffering frame...");
                            if let Some(frame_to_send) = self.request_processing.frames_buffer.park(frame) {
                                // invariant: frame_to_send is always a DATA frame at this point. TRAILERS are sent later.
                                match body_mode {
                                    OverridableBodyMode::None => { // body processing is disabled, just inject back the frame
                                        debug!(target: "ext_proc", "outbound request body frame: injecting the frame DATA into the body");
                                        _ = self.request_processing.frame_bridge.inject_frame(Ok(frame_to_send)).await;
                                    },
                                    _ => { // send the merged frame to ext_proc and park a copy for later injection
                                        debug!(target: "ext_proc", "outbound request body frame: sending body chunk of request ({})",  if frame_to_send.is_data() { "DATA" } else { "TRAILERS" });
                                        let action = self.request_processing.handle_outgoing_body_chunk(clone_frame(&frame_to_send), false);
                                        run_action!(self, self.request_processing, action, "handle_body_chunk (merged frame sent)");
                                        // save a copy of the frame to inject into the body bridge later
                                        self.request_processing.inflight_frames.push(frame_to_send);
                                    }
                                }
                            }
                        },
                        Some(Err(_err)) => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming request body to external processing");
                            self.request_processing.status_error("error occurred when streaming request body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "request body stream completing...");

                            while let Some(last_frame) = self.request_processing.frames_buffer.take() {
                                if last_frame.is_data() { // DATA
                                  if self.overridable_modes.request.should_process_body() {
                                      debug!(target: "ext_proc", "sending the last body chunk of request");
                                      let end_of_stream = !self.request_processing.frames_buffer.has_trailers();
                                      let action = self.request_processing.handle_outgoing_body_chunk(clone_frame(&last_frame), end_of_stream);
                                      run_action!(self, self.request_processing, action, "handle_body_chunk (last frame sent)");
                                      // save a copy of the frame to inject into the body bridge later
                                      self.request_processing.inflight_frames.push(last_frame);
                                  } else {
                                      debug!(target: "ext_proc", "injecting the last body chunk of request into the body");
                                      _ = self.request_processing.frame_bridge.inject_frame(Ok(last_frame)).await;
                                  }
                                } else { // TRAILERS
                                    if self.overridable_modes.request.should_process_trailers() {
                                        debug!(target: "ext_proc", "sending the last body chunk of request");
                                        let action = self.request_processing.handle_outgoing_body_chunk(clone_frame(&last_frame), true);
                                        run_action!(self, self.request_processing, action, "handle_body_chunk (last frame sent)");
                                        // save a copy of the frame to inject into the body bridge later
                                        self.request_processing.inflight_frames.push(last_frame);
                                    } else {
                                        debug!(target: "ext_proc", "injecting the last body chunk of request into the body");
                                        self.request_processing.parked_trailers = Some(last_frame);
                                    }
                                }
                            }

                            self.request_processing.end_of_stream = true;

                            if self.request_processing.inflight_frames.is_empty() {
                                debug!(target: "ext_proc", "frame bridge closed (request body)!");
                                self.request_processing.frame_bridge_close(&mut self.timeout_state.active).await;
                            }
                        }
                    }
                },

                outbound_response_body_frame = self.response_processing.frame_bridge.next(), if outbound_resp_enabled => {
                    debug!(target: "ext_proc", "outbound response body frame: {outbound_response_body_frame:?}");
                    match outbound_response_body_frame {
                        Some(Ok(frame)) => {
                            let body_mode = self.overridable_modes.response.body_mode();

                            debug!(target: "ext_proc", "outbound response body frame: buffering frame...");
                            if let Some(frame_to_send) = self.response_processing.frames_buffer.park(frame) {
                                // invariant: frame_to_send is always a DATA frame at this point. TRAILERS are sent later.
                                match body_mode {
                                    OverridableBodyMode::None => { // body processing is disabled, just inject back the frame
                                        debug!(target: "ext_proc", "outbound response body frame: injecting the frame DATA into the body");
                                        _ = self.response_processing.frame_bridge.inject_frame(Ok(frame_to_send)).await;
                                    },
                                    _ => { // send the merged frame to ext_proc and park a copy for later injection
                                        debug!(target: "ext_proc", "outbound response body frame: sending body chunk of response ({})",  if frame_to_send.is_data() { "DATA" } else { "TRAILERS" });
                                        let action = self.response_processing.handle_outgoing_body_chunk(clone_frame(&frame_to_send), false);
                                        run_action!(self, self.response_processing, action, "handle_body_chunk (merged frame sent)");
                                        // save a copy of the frame to inject into the body bridge later
                                        self.response_processing.inflight_frames.push(frame_to_send);
                                    }
                                }
                            }
                        },
                        Some(Err(_err)) => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming response body to external processing");
                            self.response_processing.status_error("error occurred when streaming response body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "response body stream completing...");

                            while let Some(last_frame) = self.response_processing.frames_buffer.take() {
                                if last_frame.is_data() { // DATA
                                  if self.overridable_modes.response.should_process_body() {
                                      debug!(target: "ext_proc", "sending the last body chunk of response");
                                      let end_of_stream = !self.response_processing.frames_buffer.has_trailers();
                                      let action = self.response_processing.handle_outgoing_body_chunk(clone_frame(&last_frame), end_of_stream);
                                      run_action!(self, self.response_processing, action, "handle_body_chunk (last frame sent)");
                                      // save a copy of the frame to inject into the body bridge later
                                      self.response_processing.inflight_frames.push(last_frame);
                                  } else {
                                      debug!(target: "ext_proc", "injecting the last body chunk of response into the body");
                                      _ = self.response_processing.frame_bridge.inject_frame(Ok(last_frame)).await;
                                  }
                                } else { // TRAILERS
                                    if self.overridable_modes.response.should_process_trailers() {
                                        debug!(target: "ext_proc", "sending the last body chunk of response");
                                        let action = self.response_processing.handle_outgoing_body_chunk(clone_frame(&last_frame), true);
                                        run_action!(self, self.response_processing, action, "handle_body_chunk (last frame sent)");
                                        // save a copy of the frame to inject into the body bridge later
                                        self.response_processing.inflight_frames.push(last_frame);
                                    } else {
                                        debug!(target: "ext_proc", "injecting the last body chunk of response into the body");
                                        self.response_processing.parked_trailers = Some(last_frame);
                                    }
                                }
                            }

                            self.response_processing.end_of_stream = true;

                            if self.response_processing.inflight_frames.is_empty() {
                                debug!(target: "ext_proc", "frame bridge closed (response body)!");
                                self.response_processing.frame_bridge_close(&mut self.timeout_state.active).await;
                           }
                        }
                    }
                },

                () = fast_timeout::fast_sleep(self.timeout_state.duration), if self.timeout_state.active => {
                    let msg = "processing_loop: message timeout";
                    self.recover_or_failure(ExtProcError::Timeout("message timeout"), &msg).await;
                    break 'transaction_loop;
                }
            }
        }
    }
}

impl ExternalProcessingWorker<kind::Observability> {
    fn new(config: Arc<ExternalProcessingWorkerConfig>, overridable_global_modes: Arc<OverridableGlobalModes>) -> Self {
        let request_processing = RequestProcessing::<kind::Observability>::from(&*config);
        let response_processing = ResponseProcessing::<kind::Observability>::from(&*config);

        let handshake = Some(ProtocolConfiguration {
            request_body_mode: config.processing_mode.request_body_mode as i32,
            response_body_mode: config.processing_mode.response_body_mode as i32,
            send_body_without_waiting_for_header_response: config.send_body_without_waiting_for_header_response,
        });
        let message_timeout = config.message_timeout;
        Self {
            config,
            bidi_stream: None,
            request_processing,
            response_processing,
            handshake,
            timeout_state: TimeoutState { active: false, duration: message_timeout, extended: false },
            overridable_modes: overridable_global_modes,
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn observability_loop(mut self, mut processing_request_channel: mpsc::Receiver<ProcessingTask>) {
        debug!(target: "ext_proc", "===== Observability BEGIN =====");
        defer! {
            debug!(target: "ext_proc", "===== Observability END =====");
        }

        let mut request_body_to_ext_proc_complete = false;
        let mut response_body_to_ext_proc_complete = false;

        'transaction_loop: loop {
            let streaming_enabled =
                self.request_processing.streaming_body_enabled || self.response_processing.streaming_body_enabled;
            let outbound_req_enabled =
                self.request_processing.streaming_body_enabled && !request_body_to_ext_proc_complete;
            let outbound_resp_enabled =
                self.response_processing.streaming_body_enabled && !response_body_to_ext_proc_complete;

            debug!(target: "ext_proc", "----- select! [ process_task:{} inbound:true outbound_req:{outbound_req_enabled} outbound_resp:{outbound_resp_enabled} timeout:{} ] -----",
                !streaming_enabled, self.timeout_state.active);

            tokio::select! {
                outbound_processing_request = processing_request_channel.recv(), if !streaming_enabled => {
                    debug!(target: "ext_proc", "processing {outbound_processing_request:?}...");
                    match outbound_processing_request {
                        Some(ProcessingTask{ data: ProcessingData::Request(headers, frame_bridge), reply_channel, http_version}) => {
                            let action = self.request_processing.process(headers, frame_bridge, reply_channel, http_version, &self.overridable_modes);
                            run_action!(self, self.request_processing, action, "process_request");
                        }
                        Some(ProcessingTask{ data: ProcessingData::Response(headers, frame_bridge), reply_channel, http_version}) => {
                            let action = self.response_processing.process(headers, frame_bridge, reply_channel, http_version, &self.overridable_modes);
                            run_action!(self, self.response_processing, action, "process_response");
                        }
                        _ => {
                            debug!(target: "ext_proc", ">> worker channel closed!");
                            break 'transaction_loop
                        },
                    }
                },

                outbound_request_body_frame = self.request_processing.frame_bridge.next(), if outbound_req_enabled => {
                    debug!(target: "ext_proc", "outbound request body frame: {outbound_request_body_frame:?}");
                    match outbound_request_body_frame {
                        Some(Ok(frame)) => {
                            debug!(target: "ext_proc", "outbound request body frame: buffering frame...");
                            if let Some(frame_to_send) = self.request_processing.frames_buffer.park(frame) {
                                if self.overridable_modes.request.should_process_body() {
                                    _ = self.request_processing.frame_bridge.inject_frame(Ok(clone_frame(&frame_to_send))).await;
                                    debug!(target: "ext_proc", "outbound request body frame: sending body chunk of request ({})",  if frame_to_send.is_data() { "DATA" } else { "TRAILERS" });
                                    let action = self.request_processing.handle_outgoing_body_chunk(frame_to_send, false);
                                    run_action!(self, self.request_processing, action, "handle_body_chunk (merged frame sent)");

                                } else {
                                    debug!(target: "ext_proc", "outbound request body frame: injecting the frame DATA into the body");
                                    _ = self.request_processing.frame_bridge.inject_frame(Ok(frame_to_send)).await;
                                }
                            }
                        },
                        Some(Err(_err)) => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming request body to external processing");
                            self.request_processing.status_error("error occurred when streaming request body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "request body stream completing...");

                            while let Some(last_frame) = self.request_processing.frames_buffer.take() {
                                if last_frame.is_data() { // DATA
                                  if self.overridable_modes.request.should_process_body() {
                                      debug!(target: "ext_proc", "sending the last data chunk of request");
                                      _ = self.request_processing.frame_bridge.inject_frame(Ok(clone_frame(&last_frame))).await;
                                      let end_of_stream = !self.request_processing.frames_buffer.has_trailers();
                                      let action = self.request_processing.handle_outgoing_body_chunk(last_frame, end_of_stream);
                                      run_action!(self, self.request_processing, action, "handle_body_chunk (last data frame sent)");
                                  } else {
                                      debug!(target: "ext_proc", "injecting the last data chunk of request into the body");
                                      _ = self.request_processing.frame_bridge.inject_frame(Ok(last_frame)).await;
                                  }
                                } else { // TRAILERS
                                    if self.overridable_modes.request.should_process_trailers() {
                                        debug!(target: "ext_proc", "sending trailers chunk of request");
                                        _ = self.request_processing.frame_bridge.inject_frame(Ok(clone_frame(&last_frame))).await;
                                        let action = self.request_processing.handle_outgoing_body_chunk(last_frame, true);
                                        run_action!(self, self.request_processing, action, "handle_body_chunk (trailer frame sent)");
                                    } else {
                                        debug!(target: "ext_proc", "injecting the trailers chunk of request into the body");
                                        _ = self.request_processing.frame_bridge.inject_frame(Ok(last_frame)).await;
                                    }
                                }
                            }

                            let action = Action::Return(ProcessingStatus::RequestReady(ReadyStatus::default()));
                            run_action!(self, self.request_processing, action, "request streaming completed");
                            self.request_processing.end_of_stream = true;
                        }
                    }
                },

                outbound_response_body_frame = self.response_processing.frame_bridge.next(), if outbound_resp_enabled => {
                    debug!(target: "ext_proc", "outbound response body frame: {outbound_response_body_frame:?}");
                    match outbound_response_body_frame {
                        Some(Ok(frame)) => {
                            debug!(target: "ext_proc", "outbound response body frame: buffering frame...");
                            if let Some(frame_to_send) = self.response_processing.frames_buffer.park(frame) {
                                if self.overridable_modes.response.should_process_body() {
                                    _ = self.response_processing.frame_bridge.inject_frame(Ok(clone_frame(&frame_to_send))).await;
                                    debug!(target: "ext_proc", "outbound response body frame: sending body chunk of response ({})",  if frame_to_send.is_data() { "DATA" } else { "TRAILERS" });
                                    let action = self.response_processing.handle_outgoing_body_chunk(frame_to_send, false);
                                    run_action!(self, self.response_processing, action, "handle_body_chunk (merged frame sent)");

                                } else {
                                    debug!(target: "ext_proc", "outbound response body frame: injecting the frame DATA into the body");
                                    _ = self.response_processing.frame_bridge.inject_frame(Ok(frame_to_send)).await;
                                }
                            }
                        },
                        Some(Err(_err)) => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming response body to external processing");
                            self.response_processing.status_error("error occurred when streaming response body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "response body stream completing...");

                            while let Some(last_frame) = self.response_processing.frames_buffer.take() {
                                if last_frame.is_data() { // DATA
                                  if self.overridable_modes.response.should_process_body() {
                                      debug!(target: "ext_proc", "sending the last data chunk of response");
                                      _ = self.response_processing.frame_bridge.inject_frame(Ok(clone_frame(&last_frame))).await;
                                      let end_of_stream = !self.response_processing.frames_buffer.has_trailers();
                                      let action = self.response_processing.handle_outgoing_body_chunk(last_frame, end_of_stream);
                                      run_action!(self, self.response_processing, action, "handle_body_chunk (last data frame sent)");
                                  } else {
                                      debug!(target: "ext_proc", "injecting the last data chunk of response into the body");
                                      _ = self.response_processing.frame_bridge.inject_frame(Ok(last_frame)).await;
                                  }
                                } else { // TRAILERS
                                    if self.overridable_modes.response.should_process_trailers() {
                                        debug!(target: "ext_proc", "sending trailers chunk of response");
                                        _ = self.response_processing.frame_bridge.inject_frame(Ok(clone_frame(&last_frame))).await;
                                        let action = self.response_processing.handle_outgoing_body_chunk(last_frame, true);
                                        run_action!(self, self.response_processing, action, "handle_body_chunk (trailer frame sent)");
                                    } else {
                                        debug!(target: "ext_proc", "injecting the trailers chunk of response into the body");
                                        _ = self.response_processing.frame_bridge.inject_frame(Ok(last_frame)).await;
                                    }
                                }
                            }

                            let action = Action::Return(ProcessingStatus::ResponseReady(ReadyStatus::default()));
                            run_action!(self, self.response_processing, action, "response streaming completed");
                            self.response_processing.end_of_stream = true;
                        }
                    }
                },

                _ = std::future::ready(()), if streaming_enabled && !outbound_req_enabled && !outbound_resp_enabled => {
                    debug!(target: "ext_proc", "no outbound request or response enabled (observability terminating...)");
                    break 'transaction_loop;
                }
            }
        }
    }
}

impl<S: kind::Mode + Default> ExternalProcessingWorker<S> {
    fn connect(grpc_service_specifier: &GrpcServiceSpecifier, first_request: ProcessingRequest) -> BidiStream {
        let (request_sender, mut request_receiver) = mpsc::channel::<ProcessingRequest>(4);
        let (response_sender, response_receiver) = mpsc::channel::<Result<ProcessingResponse, Status>>(4);
        let grpc_service_specifier = grpc_service_specifier.clone();

        tokio::spawn(async move {
            let request_stream = async_stream::stream! {
                yield first_request;
                while let Some(message) = request_receiver.recv().await {
                    yield message
                }
            };
            let stream_setup_result = match grpc_service_specifier {
                GrpcServiceSpecifier::Cluster(cluster_name) => {
                    let cluster_spec = ClusterSpecifier::Cluster(cluster_name.clone());
                    match clusters_manager::resolve_cluster(&cluster_spec) {
                        Some(cluster_id) => {
                            match clusters_manager::get_grpc_connection(cluster_id, RoutingContext::None) {
                                Ok(grpc_service) => {
                                    let mut client = ExternalProcessorClient::new(grpc_service);
                                    client.process(request_stream).await
                                },
                                Err(e) => Err(Status::unavailable(format!("Failed to get gRPC connection: {e}"))),
                            }
                        },
                        None => Err(Status::unavailable(format!(
                            "Failed to resolve cluster '{cluster_name}' for external processor"
                        ))),
                    }
                },
                GrpcServiceSpecifier::GoogleGrpc(google_grpc) => {
                    match ExternalProcessorClient::connect(google_grpc.target_uri.clone()).await {
                        Ok(mut client) => client.process(request_stream).await,
                        Err(e) => Err(Status::unavailable(format!("Failed to connect: {e}"))),
                    }
                },
            };
            match stream_setup_result {
                Ok(response_stream) => {
                    let mut stream = response_stream.into_inner();
                    while let Some(processing_response) = stream.message().await.transpose() {
                        if response_sender.send(processing_response).await.is_err() {
                            break;
                        }
                    }
                },
                Err(status) => {
                    let _ = response_sender.send(Err(status)).await;
                },
            }
        });

        BidiStream { external_sender: request_sender, inbound_responses: response_receiver }
    }

    fn get_bidi_stream(&mut self, pending_request: &mut Option<ProcessingRequest>) -> Result<&BidiStream, Error> {
        if let Some(ref stream) = self.bidi_stream {
            Ok(stream)
        } else {
            let first_request = pending_request.take().ok_or_else(|| {
                Error::from("Internal error: attempted to establish bidi stream with external processor without a ProcessingRequest")
            })?;
            let stream = Self::connect(&self.config.grpc_service_specifier, first_request);
            Ok(self.bidi_stream.insert(stream))
        }
    }

    async fn forward_to_external_processor(&mut self, mut request: ProcessingRequest) {
        request.protocol_config = self.handshake.take();
        let mut request_opt = Some(request);

        let stream = match self.get_bidi_stream(&mut request_opt) {
            Err(err) => {
                self.response_processing
                    .status_error(format!("External processor: {err:?}").as_str(), self.config.failure_mode_allow);
                self.request_processing
                    .status_error(format!("External processor: {err:?}").as_str(), self.config.failure_mode_allow);
                return;
            },
            Ok(stream) => stream,
        };
        let send_outcome = match request_opt {
            Some(request) => Some(stream.external_sender.send(request).await),
            None => None,
        };

        match send_outcome {
            Some(Err(err)) => {
                info!(target: "ext_proc", "External processor is unavailable: {err}");
                if let Some(reply_channel) = self.response_processing.reply_channel.take() {
                    let _ = reply_channel.send(
                        self.response_processing
                            .status_error("Lost connection to external processor", self.config.failure_mode_allow),
                    );
                }

                if let Some(reply_channel) = self.request_processing.reply_channel.take() {
                    let _ = reply_channel.send(
                        self.request_processing
                            .status_error("Lost connection to external processor", self.config.failure_mode_allow),
                    );
                }

                self.timeout_state.active = false;
            },
            _ => {
                // enable timeout, if not in observability mode
                self.timeout_state.active = !self.config.observability_mode;
            },
        }
    }

    #[allow(clippy::cast_sign_loss)]
    fn handle_timeout_extension(&mut self, extended_timeout: google::protobuf::Duration) -> bool {
        if self.timeout_state.extended {
            let msg = "External processor attempted multiple timeout extensions";
            if let Some(reply_channel) = self.response_processing.reply_channel.take() {
                let _ = reply_channel.send(self.response_processing.status_error(msg, self.config.failure_mode_allow));
            }

            if let Some(reply_channel) = self.request_processing.reply_channel.take() {
                let _ = reply_channel.send(self.request_processing.status_error(msg, self.config.failure_mode_allow));
            }

            return false;
        }
        let mut timeout_duration =
            Duration::from_secs(extended_timeout.seconds as u64) + Duration::from_nanos(extended_timeout.nanos as u64);
        if timeout_duration < Duration::from_millis(1) {
            info!(target:"ext_proc", "External processor: override_message_timeout must be >= 1ms");
            timeout_duration = self.config.message_timeout;
        }
        if let Some(max_timeout) = self.config.max_message_timeout {
            if timeout_duration > max_timeout {
                info!(target:"ext_proc", "External processor: attempted to override message timeout to value > max_message_timeout (defaulting to max_message_timeout)");
                timeout_duration = max_timeout;
            }
        }
        self.timeout_state.duration = timeout_duration;
        self.timeout_state.extended = true;
        true
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn build_direct_response(&mut self, response_attempt: &ImmediateResponse) -> Response<PolyBody> {
        let status = response_attempt
            .status
            .as_ref()
            .and_then(|s| http::StatusCode::from_u16(s.code as u16).ok())
            .unwrap_or(http::StatusCode::OK);
        let body_bytes = Bytes::copy_from_slice(&response_attempt.body);
        let body = Full::new(body_bytes);
        let mut response = Response::new(PolyBody::from(body));
        *response.status_mut() = status;
        if let Some(header_mutation) = &response_attempt.headers {
            let _ =
                apply_response_header_mutations(&mut response, header_mutation, self.config.mutation_rules.as_ref());
        }
        if let Some(grpc_status) = &response_attempt.grpc_status {
            if let Ok(status_value) = http::HeaderValue::from_str(&grpc_status.status.to_string()) {
                response.headers_mut().insert("grpc-status", status_value);
            }
        }
        response
    }
}
