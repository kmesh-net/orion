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
mod worker_config;

use crate::body::channel_body::{ChannelBody, FrameBridge};

use crate::event_error::EventKind;
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
    body::{body_with_metrics::BodyWithMetrics, response_flags::ResponseFlags},
    clusters::clusters_manager::{self, RoutingContext},
    listeners::{http_connection_manager::FilterDecision, synthetic_http_response::SyntheticHttpResponse},
    Error, PolyBody,
};
use bytes::Bytes;
use futures::{future::Either, StreamExt};
use http::header::CONTENT_LENGTH;
use http::{Request, Response};
use http_body::Frame;
use http_body_util::combinators::WithTrailers;
use http_body_util::BodyExt;
use http_body_util::Collected;
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
    tonic::{codec::Streaming, Status},
};
use orion_format::types::ResponseFlags as FmtResponseFlags;
use pingora_timeout::fast_timeout;
use scopeguard::defer;
use std::convert::Infallible;
use std::future::Future;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, warn};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ExtProcError {
    #[error("Timeout Error: {0}")]
    Timeout(&'static str),
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
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
        debug!(target: "ext_proc", "From<ExternalProcessorConfig, ExtProcPerRoute> for ExternalProcessor");
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
    ($self:ident, $processor:expr, $action:expr, $ctx:literal) => {
        match $action {
            Action::Send(outbound) => {
                debug!(target: "ext_proc", "{ctx} @{typ}: action -> forward {outbound:?}",
                    ctx = $ctx,
                    typ = stringify!($processor),
                    outbound = outbound);

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
    fn collect_to_single_chunk(
        original: Collected<Bytes>,
    ) -> WithTrailers<Full<Bytes>, impl Future<Output = Option<Result<http::HeaderMap, Infallible>>>> {
        let trailers = original.trailers().cloned();
        let aggregated_bytes = original.to_bytes();
        let new_body = Full::new(aggregated_bytes);
        let trailer_future = async move { trailers.map(Ok::<_, Infallible>) };
        new_body.with_trailers(trailer_future)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn apply_request(&mut self, request: &mut Request<BodyWithMetrics<PolyBody>>) -> FilterDecision {
        let modes = &self.overridable_modes.request;
        let process_headers = modes.should_process_headers();
        let process_body = modes.should_process_body();
        let process_trailers = modes.should_process_trailers();

        if !process_headers && !process_body && !process_trailers {
            return FilterDecision::Continue;
        }

        let mut ext_proc_headers = None;

        if process_headers {
            debug!(target: "ext_proc", "request processing headers");
            ext_proc_headers = Some(self.filter_header_map(request.headers()));
        }

        let body: PolyBody = std::mem::take(&mut request.body_mut().inner);

        let ext_proc_frame_bridge = match (modes.body_mode(), modes.trailer_mode()) {
            (OverridableBodyMode::None, trailers_mode) => {
                // event though body processing is None and trailers processing is Skip, we have to
                // create the bridge, to allow ext_proc mutate the body with ContinueAndReplace action.
                debug!(target: "ext_proc", "request processing body(None) and trailers:{trailers_mode:?} => {body:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                request.body_mut().inner = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Streamed | OverridableBodyMode::FullDuplexStreamed, trailers_mode) => {
                debug!(target: "ext_proc", "request processing body(Streamed) and trailers:{trailers_mode:?} => {body:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                request.body_mut().inner = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Buffered | OverridableBodyMode::BufferedPartial, trailers_mode) => {
                debug!(target: "ext_proc", "request processing body(Buffered) with trailers:{trailers_mode:?} => {body:?}");
                let Ok(collected) = body.collect().await else {
                    return self.on_filter_error(
                        "Failed to collect request body for external processing",
                        None,
                        request.version(),
                    );
                };

                let buffered = Self::collect_to_single_chunk(collected);

                let (new_body, bridge) = ChannelBody::new(buffered);
                request.body_mut().inner = PolyBody::from(new_body);
                bridge
            },
        };

        debug!(target: "ext_proc", "request headers: {ext_proc_headers:?}");
        debug!(target: "ext_proc", "request body: {ext_proc_frame_bridge:?}");

        let processing_data = ProcessingData::Request(ext_proc_headers, ext_proc_frame_bridge);

        let ver = request.version();
        let Ok(response_rx) = self.send_processing_data(processing_data, ver).await else {
            return self.on_filter_error("Failed to schedule sending request data to external processor", None, ver);
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
                warn!(target: "ext_proc", "apply_request: unexpected ResponseReady...");
                return self.on_filter_error(
                    "Unexpected ResponseReady status received during request processing",
                    None,
                    request.version(),
                );
            },
            Err(e) => {
                self.on_filter_error(format!("External processor: {e:?}").as_str(), Some(e.into()), request.version())
            },
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

        let body: PolyBody = std::mem::take(response.body_mut());

        let ext_proc_frame_bridge = match (modes.body_mode(), modes.trailer_mode()) {
            (OverridableBodyMode::None, trailers_mode) => {
                // event though body processing is None and trailers processing is Skip, we have to
                // create the bridge, to allow ext_proc mutate the body with ContinueAndReplace action.
                debug!(target: "ext_proc", "response processing body(None) and trailers:{trailers_mode:?} => {body:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                *response.body_mut() = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Streamed | OverridableBodyMode::FullDuplexStreamed, trailers_mode) => {
                debug!(target: "ext_proc", "response processing body(Streamed) and trailers:{trailers_mode:?} => {body:?}");
                let (new_body, bridge) = ChannelBody::new(body);
                *response.body_mut() = PolyBody::from(new_body);
                bridge
            },
            (OverridableBodyMode::Buffered | OverridableBodyMode::BufferedPartial, trailers_mode) => {
                debug!(target: "ext_proc", "response processing body(Buffered) with trailers:{trailers_mode:?} => {body:?}");
                let Ok(collected) = body.collect().await else {
                    return self.on_filter_error(
                        "Failed to collect response body for external processing",
                        None,
                        response.version(),
                    );
                };

                debug!(target: "ext_proc", "response body collected: {collected:?}");
                let buffered = Self::collect_to_single_chunk(collected);
                let (new_body, bridge) = ChannelBody::new(buffered);
                *response.body_mut() = PolyBody::from(new_body);
                bridge
            },
        };

        debug!(target: "ext_proc", "response headers: {ext_proc_headers:?}");
        debug!(target: "ext_proc", "response body: {ext_proc_frame_bridge:?}");

        let processing_data = ProcessingData::Response(ext_proc_headers, ext_proc_frame_bridge);

        let ver = response.version();
        let Ok(response_rx) = self.send_processing_data(processing_data, ver).await else {
            return self.on_filter_error("Failed to schedule sending response data to external processor", None, ver);
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
                warn!(target: "ext_proc", "apply_response: unexpected RequestReady...");
                return self.on_filter_error(
                    "Unexpected RequestReady status received during response processing",
                    None,
                    response.version(),
                );
            },
            Err(e) => self.on_filter_error(
                format!("External processor response processing: {e:?}").as_str(),
                Some(e.into()),
                response.version(),
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

    fn on_filter_error(&mut self, msg: &str, error: Option<Error>, http_version: http::Version) -> FilterDecision {
        if let Some(err) = error {
            warn!(target: "ext_proc","{msg}: {err}");
        } else {
            warn!(target: "ext_proc", "{msg}");
        }
        if self.worker_config.failure_mode_allow {
            FilterDecision::Continue
        } else {
            FilterDecision::DirectResponse(
                SyntheticHttpResponse::internal_error_with_msg(
                    msg,
                    EventKind::FilterChainNotFound.into(),
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
    inbound_responses: Streaming<ProcessingResponse>,
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
                        Either::Left(stream.inbound_responses.message())
                    } else {
                        Either::Right(std::future::pending::<Result<Option<ProcessingResponse>, Status>>())
                    } => {
                    debug!(target: "ext_proc", "<= inbound processing response: {inbound_processing_response:?}");

                    match inbound_processing_response {
                        Ok(Some(ProcessingResponse { override_message_timeout: Some(extended_timeout), ..})) => {
                            debug!(target: "ext_proc", "<- timeout extension received: {extended_timeout:?}");
                            if !self.handle_timeout_extension(extended_timeout) {
                                debug!(target: "ext_proc", "invalid timeout extension - closing stream");
                                break 'transaction_loop;
                            }
                        },
                        Ok(Some(ProcessingResponse { response: Some(ProcessingResponseType::ImmediateResponse(response_attempt)), ..})) => {
                            debug!(target: "ext_proc", "<- ImmediateResponse received");
                            if self.config.disable_immediate_response {
                                warn!(target: "ext_proc", "External processor attempted to send immediate response which is disabled by config");

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
                        Ok(Some(ProcessingResponse { mode_override, response: Some(ProcessingResponseType::RequestHeaders(headers_response)), ..})) => {
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
                        Ok(Some(ProcessingResponse { response: Some(ProcessingResponseType::RequestBody(body_response)), ..})) => {
                            debug!(target: "ext_proc", "<- RequestBody response received");

                            let action = self
                                .request_processing
                                .handle_body_response(body_response, Some(&self.config.route_cache_action), &mut self.timeout_state.active).await;

                            run_action!(self, self.request_processing, action, "body_response");

                        },
                        Ok(Some(ProcessingResponse { response: Some(ProcessingResponseType::RequestTrailers(trailers_response)), ..})) => {
                            debug!(target: "ext_proc", "<- RequestTrailers response received");
                            let action = self.request_processing.handle_trailers_response(trailers_response, &mut self.timeout_state.active).await;
                            run_action!(self, self.request_processing, action, "handle_trailers_response");
                        },
                        Ok(Some(ProcessingResponse { mode_override, response: Some(ProcessingResponseType::ResponseHeaders(headers_response)), ..})) => {
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
                        Ok(Some(ProcessingResponse { response: Some(ProcessingResponseType::ResponseBody(body_response)), ..})) => {
                            debug!(target: "ext_proc", "<- ResponseBody response received");
                            let empty_response = body_response.response.is_none();
                            let action = self.response_processing.handle_body_response(body_response, None, &mut self.timeout_state.active).await;
                            run_action!(self, self.response_processing, action, "handle_body_response");

                            if empty_response {
                                debug!(target: "ext_proc", "response body response contained no response - closing stream");
                                break 'transaction_loop;
                            }
                        },
                        Ok(Some(ProcessingResponse { response: Some(ProcessingResponseType::ResponseTrailers(trailers_response)), ..})) => {
                            debug!(target: "ext_proc", "<- ResponseTrailers response received");
                            let action = self.response_processing.handle_trailers_response(trailers_response, &mut self.timeout_state.active).await;
                            run_action!(self, self.response_processing, action, "handle_trailers_response");
                        },
                        Ok(Some(r)) => {
                            debug!(target: "ext_proc", "<- unsupported response received: {r:?}");

                            let status = Action::Return(self.request_processing.status_internal_error("unsupported response type received from external processor"));
                            run_action!(self, self.request_processing, status, "unsupported_response");
                            let status = Action::Return(self.response_processing.status_internal_error("unsupported response type received from external processor"));
                            run_action!(self, self.response_processing, status, "unsupported_response");

                            break 'transaction_loop;
                        }
                        Err(e) => {
                            let msg = format!("gRPC error received from external processor: {}", e.message());
                            warn!(target: "ext_proc", msg);

                            let status = Action::Return(self.request_processing.status_error(&msg, self.config.failure_mode_allow));
                            run_action!(self, self.request_processing, status, "grpc_error");
                            let status = Action::Return(self.response_processing.status_error(&msg, self.config.failure_mode_allow));
                            run_action!(self, self.response_processing, status, "grpc_error");

                            break 'transaction_loop;
                        },
                        _ => {
                            debug!(target: "ext_proc", "stream closed by the external processor");
                            break 'transaction_loop;
                        }
                    }
                },

                outbound_request_body_frame =
                    async { self.request_processing.frame_bridge.next().await }, if outbound_req_enabled => {

                    debug!(target: "ext_proc", "outbound request body frame: {outbound_request_body_frame:?}");
                    match outbound_request_body_frame {
                        Some(Ok(current_frame)) => {
                            // 1. send previously parked frame...
                            //
                            if let Some(prev_frame) = self.request_processing.parked_frame.take() {
                                debug!(target: "ext_proc", "sending body chunk of request ({})",  if prev_frame.is_data() { "DATA" } else { "TRAILERS" });
                                let action = self.request_processing.handle_outgoing_body_chunk(clone_frame(&prev_frame), false);
                                run_action!(self, self.request_processing, action, "handle_body_chunk (parked)");
                                // save a copy of the frame to inject into the body bridge later
                                self.request_processing.inflight_frames.push(prev_frame);
                            }
                            // 2. park this frame for delayed transmission or inject it directly into the body bridge...
                            //
                            self.request_processing.park_or_inject_frame(current_frame, &self.overridable_modes).await;
                        },
                        Some(Err(_err)) => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming request body to external processing");
                            self.request_processing.status_error("error occurred when streaming request body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "request body stream ended!");
                            if let Some(current_frame) = self.request_processing.parked_frame.take() {
                                debug!(target: "ext_proc", "sending last body chunk of request!");
                                let action = self.request_processing.handle_outgoing_body_chunk(clone_frame(&current_frame), true);
                                run_action!(self, self.request_processing, action, "handle_body_chunk (end)");
                                self.request_processing.inflight_frames.push(current_frame);
                            }

                            self.request_processing.end_of_stream = true;

                            if self.request_processing.inflight_frames.is_empty() {
                                debug!(target: "ext_proc", "frame bridge closed (request body)!");
                                self.request_processing.frame_bridge_close(&mut self.timeout_state.active);
                            }
                        }
                    }
                },

                outbound_response_body_frame =
                    async { self.response_processing.frame_bridge.next().await }, if outbound_resp_enabled => {

                    debug!(target: "ext_proc", "outbound response body frame: {outbound_response_body_frame:?}");
                    match outbound_response_body_frame {
                        Some(Ok(current_frame)) => {

                            // 1. send previously parked frame...
                            //
                            if let Some(prev_frame) = self.response_processing.parked_frame.take() {
                                debug!(target: "ext_proc", "sending body chunk of response ({})",  if prev_frame.is_data() { "DATA" } else { "TRAILERS" });
                                let action = self.response_processing.handle_outgoing_body_chunk(clone_frame(&prev_frame), false);
                                run_action!(self, self.response_processing, action, "handle_body_chunk (parked)");
                                // save a copy of the frame to inject into the body bridge later
                                self.response_processing.inflight_frames.push(prev_frame);
                            }

                            // 2. park this frame for delayed transmission or inject it directly into the body bridge...
                            //
                            self.response_processing.park_or_inject_frame(current_frame, &self.overridable_modes).await;

                        },
                        Some(Err(_err)) => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming response body to external processing");
                            self.response_processing.status_error("error occurred when streaming response body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "response body stream ended!");
                            if let Some(current_frame) = self.response_processing.parked_frame.take() {
                                debug!(target: "ext_proc", "sending last body chunk of response!");
                                let action = self.response_processing.handle_outgoing_body_chunk(clone_frame(&current_frame), true);
                                run_action!(self, self.response_processing, action, "handle_body_chunk (end)");
                                self.response_processing.inflight_frames.push(current_frame);
                            }

                            self.response_processing.end_of_stream = true;

                            if self.response_processing.inflight_frames.is_empty() {
                                debug!(target: "ext_proc", "frame bridge closed (response body)!");
                                self.response_processing.frame_bridge_close(&mut self.timeout_state.active);
                            }
                        }
                    }
                },

                () = fast_timeout::fast_sleep(self.timeout_state.duration), if self.timeout_state.active => {
                    debug!(target: "ext_proc", "loop: message timeout!");

                    if self.config.failure_mode_allow {
                        warn!(target: "ext_proc", "external processing message timeout - continue (failure_mode_allow is true)");

                        self.request_processing.inject_inflight_frames_and_complete().await;
                        self.request_processing.frame_bridge_close(&mut self.timeout_state.active);
                        self.response_processing.inject_inflight_frames_and_complete().await;
                        self.response_processing.frame_bridge_close(&mut self.timeout_state.active);

                    } else {
                        warn!(target: "ext_proc", "external processing message timeout - abort (failure_mode_allow is false)");

                        _ = self.request_processing.frame_bridge.inject_frame(Err(Box::new(ExtProcError::Timeout("request timeout")))).await;
                        self.request_processing.frame_bridge_close(&mut self.timeout_state.active);
                        _ = self.response_processing.frame_bridge.inject_frame(Err(Box::new(ExtProcError::Timeout("request timeout")))).await;
                        self.response_processing.frame_bridge_close(&mut self.timeout_state.active);
                    }

                    let status = Action::Return(self.request_processing.status_timeout(self.config.failure_mode_allow));
                    run_action!(self, self.request_processing, status, "message_timeout");
                    let status = Action::Return(self.response_processing.status_timeout(self.config.failure_mode_allow));
                    run_action!(self, self.response_processing, status, "message_timeout");
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

                outbound_request_body_frame =
                    async { self.request_processing.frame_bridge.next().await }, if outbound_req_enabled => {

                    debug!(target: "ext_proc", "outbound request body frame: {outbound_request_body_frame:?}");
                    match outbound_request_body_frame {
                        Some(Ok(frame)) => {
                            // send the current frame and inject back into the body bridge...
                            debug!(target: "ext_proc", "sending body chunk of request ({})",  if frame.is_data() { "DATA" } else { "TRAILERS" });
                            let action = self.request_processing.handle_outgoing_body_chunk(clone_frame(&frame), false);
                            run_action!(self, self.request_processing, action, "handle_body_chunk");
                            _ = self.request_processing.frame_bridge.inject_frame(Ok(frame)).await;
                            let action = Action::Return(ProcessingStatus::RequestReady(ReadyStatus::default()));
                            run_action!(self, self.request_processing, action, "handle_body_chunk (returning after first chunk)");
                        },
                        Some(Err(_err)) => {
                            request_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming request body to external processing");
                            self.request_processing.status_error("error occurred when streaming request body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            debug!(target: "ext_proc", "request body stream ended!");
                            request_body_to_ext_proc_complete = true;
                            self.request_processing.end_of_stream = true;
                            self.request_processing.frame_bridge_close(&mut self.timeout_state.active);
                            let action = Action::Return(ProcessingStatus::RequestReady(ReadyStatus::default()));
                            run_action!(self, self.request_processing, action, "handle_body_chunk (end)");
                        }
                    }
                },

                outbound_response_body_frame =
                    async { self.response_processing.frame_bridge.next().await }, if outbound_resp_enabled => {

                    debug!(target: "ext_proc", "outbound response body frame: {outbound_response_body_frame:?}");
                    match outbound_response_body_frame {
                        Some(Ok(frame)) => {
                            // send the current frame and inject back into the body bridge...
                            debug!(target: "ext_proc", "sending body chunk of request ({})",  if frame.is_data() { "DATA" } else { "TRAILERS" });
                            let action = self.response_processing.handle_outgoing_body_chunk(clone_frame(&frame), false);
                            run_action!(self, self.response_processing, action, "handle_body_chunk");
                            _ = self.response_processing.frame_bridge.inject_frame(Ok(frame)).await;
                            let action = Action::Return(ProcessingStatus::ResponseReady(ReadyStatus::default()));
                            run_action!(self, self.response_processing, action, "handle_body_chunk (returning after first chunk)");
                        },
                        Some(Err(_err)) => {
                            response_body_to_ext_proc_complete = true;
                            debug!(target: "ext_proc", "error occurred when streaming response body to external processing");
                            self.response_processing.status_error("error occurred when streaming response body to external processing", self.config.failure_mode_allow);
                        },
                        None => {
                            debug!(target: "ext_proc", "request body stream ended!");
                            response_body_to_ext_proc_complete = true;
                            self.response_processing.end_of_stream = true;
                            self.response_processing.frame_bridge_close(&mut self.timeout_state.active);
                            let action = Action::Return(ProcessingStatus::ResponseReady(ReadyStatus::default()));
                            run_action!(self, self.response_processing, action, "handle_body_chunk (end)");
                        }
                    }
                }
            }
        }
    }
}

impl<S: kind::Mode + Default> ExternalProcessingWorker<S> {
    async fn connect(
        grpc_service_specifier: &GrpcServiceSpecifier,
        first_request: ProcessingRequest,
    ) -> Result<BidiStream, Error> {
        // Create a channel to send requests to the gRPC stream.
        let (request_sender, mut request_receiver) = mpsc::channel::<ProcessingRequest>(4);
        let request_stream = async_stream::stream! {
            yield first_request;
            while let Some(message) = request_receiver.recv().await {
                yield message
            }
        };
        let response_stream = match grpc_service_specifier {
            GrpcServiceSpecifier::Cluster(cluster_name) => {
                let cluster_spec = ClusterSpecifier::Cluster(cluster_name.clone());
                let cluster_id = clusters_manager::resolve_cluster(&cluster_spec).ok_or_else(|| {
                    Error::from(format!("Failed to resolve cluster '{cluster_name}' for external processor"))
                })?;
                let grpc_service = clusters_manager::get_grpc_connection(cluster_id, RoutingContext::None)?;
                let mut client = ExternalProcessorClient::new(grpc_service);
                client
                    .process(request_stream)
                    .await
                    .map_err(|e| Error::from(format!("Failed to establish external processor stream: {e}")))?
                    .into_inner()
            },
            GrpcServiceSpecifier::GoogleGrpc(google_grpc) => {
                let mut client =
                    ExternalProcessorClient::connect(google_grpc.target_uri.clone()).await.map_err(|e| {
                        Error::from(format!("Failed to connect to external processor (GoogleGrpc endpoint): {e}"))
                    })?;
                client
                    .process(request_stream)
                    .await
                    .map_err(|e| Error::from(format!("Failed to establish external processor stream: {e}")))?
                    .into_inner()
            },
        };

        Ok::<_, Error>(BidiStream { external_sender: request_sender, inbound_responses: response_stream })
    }

    async fn get_bidi_stream(&mut self, pending_request: &mut Option<ProcessingRequest>) -> Result<&BidiStream, Error> {
        if let Some(ref stream) = self.bidi_stream {
            Ok(stream)
        } else {
            let first_request = pending_request.take().ok_or_else(|| {
                Error::from("Internal error: attempted to establish bidi stream with external processor without a ProcessingRequest")
            })?;
            let stream = Self::connect(&self.config.grpc_service_specifier, first_request).await?;
            Ok(self.bidi_stream.insert(stream))
        }
    }

    async fn forward_to_external_processor(&mut self, mut request: ProcessingRequest) {
        request.protocol_config = self.handshake.take();
        let mut request_opt = Some(request);

        let stream = match self.get_bidi_stream(&mut request_opt).await {
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
                warn!(target: "ext_proc", "External processor is unavailable: {err}");
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
            warn!(target:"ext_proc", "External processor: override_message_timeout must be >= 1ms");
            timeout_duration = self.config.message_timeout;
        }
        if let Some(max_timeout) = self.config.max_message_timeout {
            if timeout_duration > max_timeout {
                warn!(target:"ext_proc", "External processor: attempted to override message timeout to value > max_message_timeout (defaulting to max_message_timeout)");
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
        let mut response = Response::new(crate::PolyBody::from(body));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        body::{body_with_metrics::BodyWithMetrics, response_flags::BodyKind},
        listeners::http_connection_manager::ext_proc::{
            kind::{MsgKind, RequestMsg, ResponseMsg},
            mutation::apply_header_mutations,
            r#override::ModeSelector,
        },
    };
    use http::{Method, StatusCode, Version};
    use http_body_util::{BodyExt, Empty, Full};
    use orion_configuration::config::network_filters::http_connection_manager::http_filters::ext_proc::{
        BodyProcessingMode, ExternalProcessor as ExternalProcessorConfig, GoogleGrpc, GrpcService,
        GrpcServiceSpecifier, HeaderProcessingMode, ProcessingMode, RouteCacheAction, TrailerProcessingMode,
    };
    use orion_data_plane_api::envoy_data_plane_api::envoy::extensions::filters::http::ext_proc::v3::{
        processing_mode, ProcessingMode as EnvoyProcessingMode,
    };
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::core::v3::{
                header_value_option::HeaderAppendAction, HeaderValue as EnvoyHeaderValue, HeaderValueOption,
            },
            r#type::v3::HttpStatus as EnvoyHttpStatus,
            service::ext_proc::v3::{
                body_mutation::Mutation,
                common_response::ResponseStatus,
                external_processor_server::{ExternalProcessor as ExternalProcessorService, ExternalProcessorServer},
                processing_response::Response as ProcessingResponseType,
                BodyMutation, BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, ProcessingRequest,
                ProcessingResponse, StreamedBodyResponse, TrailersResponse,
            },
        },
        tonic::{
            async_trait,
            transport::{Error as TonicError, Server},
            Request as TonicRequest, Response as TonicResponse, Status, Streaming,
        },
    };
    use std::{collections::VecDeque, future::ready, net::SocketAddr, str::FromStr, time::Duration};
    use tokio::{net::TcpListener, task::JoinHandle};

    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};

    #[derive(Debug, Clone)]
    pub struct MockExternalProcessorState {
        responses: VecDeque<ProcessingResponse>,
    }

    impl MockExternalProcessorState {
        pub fn new() -> Self {
            Self { responses: VecDeque::new() }
        }
        pub fn add_response(mut self, response: ProcessingResponse) -> Self {
            self.responses.push_back(response);
            self
        }
        pub fn get_next_response(&mut self) -> Option<ProcessingResponse> {
            self.responses.pop_front()
        }
    }

    #[derive(Debug)]
    pub struct MockExternalProcessor {
        state: MockExternalProcessorState,
        delay: Option<Duration>,
    }

    impl MockExternalProcessor {
        pub fn new(state: MockExternalProcessorState, delay: Option<Duration>) -> Self {
            Self { state, delay }
        }
    }

    #[async_trait]
    impl ExternalProcessorService for MockExternalProcessor {
        type ProcessStream = ReceiverStream<Result<ProcessingResponse, Status>>;

        async fn process(
            &self,
            request: TonicRequest<Streaming<ProcessingRequest>>,
        ) -> Result<TonicResponse<Self::ProcessStream>, Status> {
            let mut inbound = request.into_inner();
            let mut state = self.state.clone();
            let delay = self.delay.clone();
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                while let Some(_req) = inbound.message().await.unwrap_or(None) {
                    if let Some(response) = state.get_next_response() {
                        if let Some(delay) = delay {
                            tokio::time::sleep(delay).await;
                        }
                        if tx.send(Ok(response)).await.is_err() {
                            break;
                        }
                    }
                }
            });
            let output_stream = ReceiverStream::new(rx);
            Ok(TonicResponse::new(output_stream))
        }
    }

    async fn start_mock_server(state: MockExternalProcessorState) -> (SocketAddr, JoinHandle<Result<(), TonicError>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket_addr = listener.local_addr().unwrap();
        let incoming = TcpListenerStream::new(listener);
        let mock_service = MockExternalProcessor::new(state, None);
        let server =
            Server::builder().add_service(ExternalProcessorServer::new(mock_service)).serve_with_incoming(incoming);
        let server_handle = tokio::spawn(server);
        (socket_addr, server_handle)
    }

    async fn start_mock_server_with_delay(
        state: MockExternalProcessorState,
        delay: Duration,
    ) -> (SocketAddr, JoinHandle<Result<(), TonicError>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket_addr = listener.local_addr().unwrap();
        let incoming = TcpListenerStream::new(listener);
        let mock_service = MockExternalProcessor::new(state, Some(delay));
        let server =
            Server::builder().add_service(ExternalProcessorServer::new(mock_service)).serve_with_incoming(incoming);
        let server_handle = tokio::spawn(server);
        (socket_addr, server_handle)
    }

    #[derive(Debug, Default)]
    struct Mock<M: MsgKind> {
        headers: Vec<Option<(&'static str, &'static str)>>,
        body: Option<&'static str>,
        trailers: Vec<Option<(&'static str, &'static str)>>,
        _marker: std::marker::PhantomData<M>,
    }

    impl<M: MsgKind> Mock<M> {
        fn new(
            headers: Vec<Option<(&'static str, &'static str)>>,
            body: Option<&'static str>,
            trailers: Vec<Option<(&'static str, &'static str)>>,
        ) -> Self {
            Self { headers, body, trailers, _marker: std::marker::PhantomData }
        }

        fn orig_headers_map(&self) -> http::HeaderMap {
            self.apply_header_mutation(None)
        }

        fn orig_trailers_map(&self) -> http::HeaderMap {
            self.apply_trailer_mutation(None)
        }

        fn apply_header_mutation(&self, header_mutation: Option<&HeaderMutation>) -> http::HeaderMap {
            let mut header_map = http::HeaderMap::with_capacity(self.headers.len());
            for (key, value) in self.headers.iter().flatten() {
                header_map.insert(http::HeaderName::from_static(key), http::HeaderValue::from_static(value));
            }
            if let Some(mutation) = header_mutation.as_ref() {
                apply_header_mutations(&mut header_map, mutation, None).unwrap();
            }
            header_map
        }

        fn apply_body_mutation(&self, body_mutation: Option<&Mutation>) -> Option<Bytes> {
            let body_replacement: Option<Bytes> = match body_mutation {
                Some(Mutation::Body(bytes)) => Some(bytes.clone().into()),
                Some(Mutation::ClearBody(true)) => Some(Bytes::new()),
                Some(Mutation::ClearBody(false)) | None => None,
                Some(Mutation::StreamedResponse(chunk)) => Some(chunk.clone().body.into()),
            };
            body_replacement.or(self.body.map(Into::into))
        }

        fn apply_trailer_mutation(&self, trailer_mutation: Option<&HeaderMutation>) -> http::HeaderMap {
            let mut trailer_map = http::HeaderMap::with_capacity(self.trailers.len());
            for (key, value) in self.trailers.iter().flatten() {
                trailer_map.insert(http::HeaderName::from_static(key), http::HeaderValue::from_static(value));
            }
            if let Some(mutation) = trailer_mutation.as_ref() {
                apply_header_mutations(&mut trailer_map, mutation, None).unwrap();
            }
            trailer_map
        }
    }

    fn build_request_from_mock(mock_request: &Mock<RequestMsg>) -> Request<BodyWithMetrics<PolyBody>> {
        let mut req = Request::builder().method(Method::GET).uri("http://example.com/test").version(Version::HTTP_11);

        if let Some(headers) = transform(mock_request.headers.clone()) {
            for (name, value) in headers {
                req = req.header(name, value);
            }
        }

        let mut trailers_map = http::HeaderMap::with_capacity(mock_request.trailers.len());
        if let Some(trailers) = transform(mock_request.trailers.clone()) {
            trailers_map = if trailers.is_empty() {
                http::HeaderMap::default()
            } else {
                let mut map = http::header::HeaderMap::new();
                for (name, value) in trailers {
                    map.append(
                        http::header::HeaderName::from_str(name).unwrap(),
                        http::header::HeaderValue::from_str(value).unwrap().clone(),
                    );
                }
                map
            };
        }

        let body = match mock_request.body {
            None if !trailers_map.is_empty() => PolyBody::from(
                Empty::<bytes::Bytes>::new().with_trailers(ready(Some(Ok::<_, Infallible>(trailers_map)))),
            ),
            Some(b) if !trailers_map.is_empty() => {
                PolyBody::from(Full::new(bytes::Bytes::from(b.to_owned())).with_trailers(ready(Some(Ok::<
                    _,
                    Infallible,
                >(
                    trailers_map
                )))))
            },
            None => PolyBody::from(Empty::<bytes::Bytes>::new()),
            Some(b) => PolyBody::from(Full::new(bytes::Bytes::from(b.to_owned()))),
        };

        req.body(BodyWithMetrics::new(BodyKind::Request, body, |_, _, _| {})).unwrap()
    }

    fn build_response_from_mock(mock_response: &Mock<ResponseMsg>) -> Response<PolyBody> {
        let mut resp = Response::builder().version(Version::HTTP_11);

        if let Some(headers) = transform(mock_response.headers.clone()) {
            for (name, value) in headers {
                resp = resp.header(name, value);
            }
        }

        let mut trailers_map = http::HeaderMap::with_capacity(mock_response.trailers.len());
        if let Some(trailers) = transform(mock_response.trailers.clone()) {
            trailers_map = if trailers.is_empty() {
                http::HeaderMap::default()
            } else {
                let mut map = http::header::HeaderMap::new();
                for (name, value) in trailers {
                    map.append(
                        http::header::HeaderName::from_str(name).unwrap(),
                        http::header::HeaderValue::from_str(value).unwrap().clone(),
                    );
                }
                map
            };
        }

        let body = match mock_response.body {
            None if !trailers_map.is_empty() => PolyBody::from(
                Empty::<bytes::Bytes>::new().with_trailers(ready(Some(Ok::<_, Infallible>(trailers_map)))),
            ),
            Some(b) if !trailers_map.is_empty() => {
                PolyBody::from(Full::new(bytes::Bytes::from(b.to_owned())).with_trailers(ready(Some(Ok::<
                    _,
                    Infallible,
                >(
                    trailers_map
                )))))
            },
            None => PolyBody::from(Empty::<bytes::Bytes>::new()),
            Some(b) => PolyBody::from(Full::new(bytes::Bytes::from(b.to_owned()))),
        };

        resp.body(body).unwrap()
    }

    fn create_default_config_for_ext_proc_filter(
        server_addr: SocketAddr,
        processing_mode: ProcessingMode,
    ) -> ExternalProcessorConfig {
        ExternalProcessorConfig {
            grpc_service: GrpcService {
                service_specifier: GrpcServiceSpecifier::GoogleGrpc(GoogleGrpc {
                    target_uri: format!("http://{server_addr}"),
                }),
                timeout: Some(Duration::from_secs(1)),
            },
            processing_mode: Some(processing_mode),
            observability_mode: false,
            failure_mode_allow: false,
            disable_immediate_response: false,
            forward_rules: None,
            mutation_rules: None,
            message_timeout: Some(Duration::from_millis(200)),
            max_message_timeout: None,
            allowed_override_modes: vec![],
            allow_mode_override: false,
            route_cache_action: RouteCacheAction::Default,
            send_body_without_waiting_for_header_response: false,
            deferred_close_timeout: None,
        }
    }

    #[inline]
    fn create_header_mutation(headers: Vec<(&str, &str)>) -> Option<HeaderMutation> {
        if headers.is_empty() {
            None
        } else {
            Some(HeaderMutation {
                set_headers: headers
                    .into_iter()
                    .map(|(key, value)| HeaderValueOption {
                        header: Some(EnvoyHeaderValue {
                            key: key.to_owned(),
                            value: value.to_owned(),
                            raw_value: vec![],
                        }),
                        #[allow(deprecated)]
                        append: None,
                        append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                        keep_empty_value: false,
                    })
                    .collect(),
                remove_headers: vec![],
            })
        }
    }

    #[inline]
    fn create_body_mutation(body: Vec<u8>, end_of_stream: Option<bool>) -> BodyMutation {
        if let Some(end_of_stream) = end_of_stream {
            BodyMutation { mutation: Some(Mutation::StreamedResponse(StreamedBodyResponse { body, end_of_stream })) }
        } else {
            BodyMutation { mutation: Some(Mutation::Body(body)) }
        }
    }

    #[inline]
    fn create_trailer_mutation(trailers: Vec<(&str, &str)>) -> Option<HeaderMutation> {
        create_header_mutation(trailers)
    }

    #[inline]
    fn convert_trailers_to_envoy_header_map(trailers: Vec<(&str, &str)>) -> HeaderMap {
        HeaderMap {
            headers: trailers
                .into_iter()
                .map(|(key, value)| EnvoyHeaderValue {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    raw_value: vec![],
                })
                .collect(),
        }
    }

    fn create_immediate_response(
        headers: Vec<Option<(&str, &str)>>,
        body_data: Option<Vec<u8>>,
        status: i32,
    ) -> ProcessingResponse {
        let response_headers = transform(headers).and_then(|hdrs| create_header_mutation(hdrs));
        let response_body = body_data.unwrap_or_default();

        let immediate_response = ImmediateResponse {
            status: Some(EnvoyHttpStatus { code: status }),
            headers: response_headers,
            body: response_body,
            grpc_status: None,
            details: "immediate_response".to_string(),
        };

        ProcessingResponse {
            response: Some(ProcessingResponseType::ImmediateResponse(immediate_response)),
            mode_override: None,
            dynamic_metadata: None,
            override_message_timeout: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_headers_response<M: MsgKind>(
        headers: Vec<Option<(&str, &str)>>,
        body_data: Option<Vec<u8>>,
        trailers: Vec<Option<(&str, &str)>>,
        status: i32,
        end_of_stream: Option<bool>,
    ) -> ProcessingResponse {
        let header_mutation = transform(headers).and_then(|hdrs| create_header_mutation(hdrs));
        let body_mutation = body_data.map(|body| create_body_mutation(body, end_of_stream));
        let mut trailers_new = None;
        if status == ResponseStatus::ContinueAndReplace as i32 {
            if let Some(trailers) = transform(trailers) {
                trailers_new = Some(convert_trailers_to_envoy_header_map(trailers));
            }
        }

        let header_response = HeadersResponse {
            response: Some(CommonResponse {
                status,
                header_mutation,
                body_mutation,
                trailers: trailers_new,
                clear_route_cache: false,
            }),
        };

        let response = if M::IS_RESPONSE {
            Some(ProcessingResponseType::ResponseHeaders(header_response))
        } else {
            Some(ProcessingResponseType::RequestHeaders(header_response))
        };

        ProcessingResponse { response, mode_override: None, dynamic_metadata: None, override_message_timeout: None }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_body_response<M: MsgKind>(
        headers: Vec<Option<(&str, &str)>>,
        body_data: Option<Vec<u8>>,
        trailers: Vec<Option<(&str, &str)>>,
        status: i32,
        end_of_stream: Option<bool>,
    ) -> ProcessingResponse {
        let header_mutation = transform(headers).and_then(|hdrs| create_header_mutation(hdrs));
        let body_mutation = body_data.map(|body| create_body_mutation(body, end_of_stream));
        let mut trailers_new = None;
        if status == ResponseStatus::ContinueAndReplace as i32 {
            if let Some(trailers) = transform(trailers) {
                trailers_new = Some(convert_trailers_to_envoy_header_map(trailers));
            }
        }

        let body_response = BodyResponse {
            response: Some(CommonResponse {
                status,
                header_mutation,
                body_mutation,
                trailers: trailers_new,
                clear_route_cache: false,
            }),
        };

        let response = if M::IS_RESPONSE {
            Some(ProcessingResponseType::ResponseBody(body_response))
        } else {
            Some(ProcessingResponseType::RequestBody(body_response))
        };

        ProcessingResponse { response, mode_override: None, dynamic_metadata: None, override_message_timeout: None }
    }

    fn create_trailers_response<M: MsgKind>(trailers: Vec<Option<(&str, &str)>>) -> ProcessingResponse {
        let trailer_mutation = transform(trailers).and_then(|trls| create_trailer_mutation(trls));

        let trailers_response = TrailersResponse { header_mutation: trailer_mutation };
        let response = if M::IS_RESPONSE {
            Some(ProcessingResponseType::ResponseTrailers(trailers_response))
        } else {
            Some(ProcessingResponseType::RequestTrailers(trailers_response))
        };
        ProcessingResponse { response, mode_override: None, dynamic_metadata: None, override_message_timeout: None }
    }

    static HEADER_PROCESSING_MODE: [HeaderProcessingMode; 3] =
        [HeaderProcessingMode::Default, HeaderProcessingMode::Send, HeaderProcessingMode::Skip];
    static BODY_PROCESSING_MODE: [BodyProcessingMode; 5] = [
        BodyProcessingMode::None,
        BodyProcessingMode::Streamed,
        BodyProcessingMode::Buffered,
        BodyProcessingMode::BufferedPartial,
        BodyProcessingMode::FullDuplexStreamed,
    ];
    static TRAILER_PROCESSING_MODE: [TrailerProcessingMode; 3] =
        [TrailerProcessingMode::Default, TrailerProcessingMode::Skip, TrailerProcessingMode::Send];

    fn generate_processing_mode_configurations<M: MsgKind>() -> Vec<ProcessingMode> {
        if M::IS_RESPONSE {
            HEADER_PROCESSING_MODE
                .iter()
                .flat_map(|&header_mode| {
                    BODY_PROCESSING_MODE.iter().flat_map(move |&body_mode| {
                        TRAILER_PROCESSING_MODE.iter().map(move |&trailer_mode| ProcessingMode {
                            request_header_mode: HeaderProcessingMode::Skip,
                            request_body_mode: BodyProcessingMode::None,
                            request_trailer_mode: TrailerProcessingMode::Skip,
                            response_header_mode: header_mode,
                            response_body_mode: body_mode,
                            response_trailer_mode: trailer_mode,
                        })
                    })
                })
                .collect()
        } else {
            HEADER_PROCESSING_MODE
                .iter()
                .flat_map(|&header_mode| {
                    BODY_PROCESSING_MODE.iter().flat_map(move |&body_mode| {
                        TRAILER_PROCESSING_MODE.iter().map(move |&trailer_mode| ProcessingMode {
                            request_header_mode: header_mode,
                            request_body_mode: body_mode,
                            request_trailer_mode: trailer_mode,
                            response_header_mode: HeaderProcessingMode::Skip,
                            response_body_mode: BodyProcessingMode::None,
                            response_trailer_mode: TrailerProcessingMode::Skip,
                        })
                    })
                })
                .collect()
        }
    }

    // we always include extra headers in the generated request
    static REQUEST_HEADERS: [Option<(&str, &str)>; 2] = [None, Some(("x-test-header", "original-header-value"))];
    static REQUEST_BODIES: [Option<&str>; 2] = [None, Some("original body")];
    static REQUEST_TRAILERS: [Option<(&str, &str)>; 2] = [None, Some(("x-test-trailer", "original-trailer-value"))];

    fn generate_mock_messages<M: MsgKind>() -> Vec<Mock<M>> {
        REQUEST_HEADERS
            .iter()
            .flat_map(|&headers| {
                REQUEST_BODIES.iter().flat_map(move |&body| {
                    REQUEST_TRAILERS.iter().map(move |&trailers| Mock::<M> {
                        headers: vec![headers],
                        body,
                        trailers: vec![trailers],
                        _marker: std::marker::PhantomData,
                    })
                })
            })
            .collect()
    }

    static HEADER_MODIFICATIONS: [Option<(&str, &str)>; 2] = [None, Some(("x-test-header", "ext-proc-header-value"))];
    static BODY_MODIFICATIONS: [Option<&str>; 2] = [None, Some("external processor body")];
    static TRAILER_MODIFICATIONS: [Option<(&str, &str)>; 2] =
        [None, Some(("x-test-trailer", "ext-proc-trailer-value"))];

    fn transform<T>(vec: Vec<Option<T>>) -> Option<Vec<T>> {
        let filtered: Vec<T> = vec.into_iter().flatten().collect();
        (!filtered.is_empty()).then_some(filtered)
    }

    fn generate_header_processing_response<M: MsgKind>(status: ResponseStatus) -> Vec<ProcessingResponse> {
        HEADER_MODIFICATIONS
            .iter()
            .flat_map(|&headers| {
                BODY_MODIFICATIONS.iter().flat_map(move |&body| {
                    TRAILER_MODIFICATIONS.iter().map(move |&trailers| {
                        create_headers_response::<M>(
                            vec![headers],
                            body.map(Into::into),
                            // trailers modifications are NYI in body response in Envoy but Orion supports them
                            vec![trailers],
                            status as i32,
                            None,
                        )
                    })
                })
            })
            .collect()
    }

    fn generate_body_processing_response<M: MsgKind>(status: ResponseStatus) -> Vec<ProcessingResponse> {
        HEADER_MODIFICATIONS
            .iter()
            .flat_map(|&headers| {
                BODY_MODIFICATIONS.iter().flat_map(move |&body| {
                    TRAILER_MODIFICATIONS.iter().map(move |&trailers| {
                        create_body_response::<M>(
                            vec![headers],
                            body.map(Into::into),
                            // trailers modifications are NYI in body response in Envoy but Orion supports them
                            vec![trailers],
                            status as i32,
                            None,
                        )
                    })
                })
            })
            .collect()
    }

    fn generate_trailer_processing_response<M: MsgKind>() -> Vec<ProcessingResponse> {
        TRAILER_MODIFICATIONS.iter().map(move |&trailers| create_trailers_response::<M>(vec![trailers])).collect()
    }

    fn generate_mock_external_processors_states<M: MsgKind>(status: ResponseStatus) -> Vec<MockExternalProcessorState> {
        let mut mock_ext_proc_state: Vec<MockExternalProcessorState> = vec![];
        for header_response in generate_header_processing_response::<M>(status) {
            for body_response in generate_body_processing_response::<M>(status) {
                for trailer_response in generate_trailer_processing_response::<M>() {
                    mock_ext_proc_state.push(
                        MockExternalProcessorState::new()
                            .add_response(header_response.clone())
                            .add_response(body_response.clone())
                            .add_response(trailer_response),
                    );
                }
            }
        }
        mock_ext_proc_state
    }

    fn validate_mock_server_configuration<M: MsgKind + ModeSelector>(
        mock_msg: &Mock<M>,
        processing_mode: &ProcessingMode,
        mock_ext_proc_state: &MockExternalProcessorState,
    ) -> bool {
        let has_headers = transform(mock_msg.headers.clone()).is_some();
        let has_body = mock_msg.body.is_some();
        let has_trailers = transform(mock_msg.trailers.clone()).is_some();

        let should_send_headers =
            has_headers && !matches!(<M as ModeSelector>::header_mode(processing_mode), HeaderProcessingMode::Skip);
        let should_send_body =
            has_body && !matches!(<M as ModeSelector>::body_mode(processing_mode), BodyProcessingMode::None);
        let should_send_trailers =
            has_trailers && matches!(<M as ModeSelector>::trailer_mode(processing_mode), TrailerProcessingMode::Send);

        for response in &mock_ext_proc_state.responses {
            match response.response.as_ref().unwrap() {
                ProcessingResponseType::RequestHeaders(_) | ProcessingResponseType::ResponseHeaders(_) => {
                    if !should_send_headers {
                        return false;
                    }
                },
                ProcessingResponseType::RequestBody(_) | ProcessingResponseType::ResponseBody(_) => {
                    if !should_send_body {
                        return false;
                    }
                },
                ProcessingResponseType::RequestTrailers(_) | ProcessingResponseType::ResponseTrailers(_) => {
                    if !should_send_trailers {
                        return false;
                    }
                },
                ProcessingResponseType::ImmediateResponse(_) => return false,
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn assert_result<M>(
        test_case_num: i32,
        mock: &Mock<M>,
        headers: &http::HeaderMap,
        body: Option<&Bytes>,
        trailers: &http::HeaderMap,
        mock_state: &MockExternalProcessorState,
        processing_mode: &ProcessingMode,
    ) where
        M: std::fmt::Debug + MsgKind + ModeSelector,
    {
        // ProcessingMode predicates
        let has_headers = transform(mock.headers.clone()).is_some();
        let has_body = mock.body.is_some();
        let has_trailers = transform(mock.trailers.clone()).is_some();

        let should_send_headers =
            has_headers && !matches!(<M as ModeSelector>::header_mode(processing_mode), HeaderProcessingMode::Skip);
        let should_send_body =
            has_body && !matches!(<M as ModeSelector>::body_mode(processing_mode), BodyProcessingMode::None);
        let should_send_trailers =
            has_trailers && matches!(<M as ModeSelector>::trailer_mode(processing_mode), TrailerProcessingMode::Send);

        // Compute expected values to assert over modified request
        let mut expected_headers = mock.orig_headers_map();
        let mut expected_body = mock.body.map(Into::into);
        let mut expected_trailers = mock.orig_trailers_map();
        for response_opt in &mock_state.responses {
            if let Some(response) = response_opt.response.as_ref() {
                match response {
                    ProcessingResponseType::RequestHeaders(HeadersResponse { response: Some(common_response) })
                    | ProcessingResponseType::ResponseHeaders(HeadersResponse { response: Some(common_response) }) => {
                        let mutation =
                            should_send_headers.then_some(common_response.header_mutation.as_ref()).flatten();
                        expected_headers = mock.apply_header_mutation(mutation);
                    },
                    ProcessingResponseType::RequestBody(BodyResponse {
                        response: Some(CommonResponse { body_mutation: Some(BodyMutation { mutation }), .. }),
                    })
                    | ProcessingResponseType::ResponseBody(BodyResponse {
                        response: Some(CommonResponse { body_mutation: Some(BodyMutation { mutation }), .. }),
                    }) => {
                        let mutation = should_send_body.then_some(mutation.as_ref()).flatten();
                        expected_body = mock.apply_body_mutation(mutation);
                    },
                    ProcessingResponseType::RequestTrailers(TrailersResponse { header_mutation })
                    | ProcessingResponseType::ResponseTrailers(TrailersResponse { header_mutation }) => {
                        let mutation = should_send_trailers.then_some(header_mutation.as_ref()).flatten();
                        expected_trailers = mock.apply_trailer_mutation(mutation);
                    },
                    _ => (),
                }
            }
        }

        assert_eq!(
            *headers,
            expected_headers,
            "test_case #: {}, asserting headers, original {} {:#?}, ext_proc server conf: {:#?}, ext_proc_filter processing mode: {:#?}",
            test_case_num,
            <M as MsgKind>::NAME,
            mock,
            mock_state,
            processing_mode
        );
        assert_eq!(
            body,
            expected_body.as_ref(),
            "test_case #: {}, asserting body, original {} {:#?}, ext_proc server conf: {:#?}, ext_proc_filter processing mode: {:#?}",
            test_case_num,
            <M as MsgKind>::NAME,
            mock,
            mock_state,
            processing_mode
        );
        assert_eq!(
            *trailers,
            expected_trailers,
            "test_case #: {}, asserting trailers, original {} {:#?}, ext_proc server conf: {:#?}, ext_proc_filter processing mode: {:#?}",
            test_case_num,
            <M as MsgKind>::NAME,
            mock,
            mock_state,
            processing_mode
        );
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_combinatorial_modes_continue() {
        let status = ResponseStatus::Continue;
        let mut test_case_num = 0;

        let mock_states = generate_mock_external_processors_states::<RequestMsg>(status);
        let processing_modes = generate_processing_mode_configurations::<RequestMsg>();
        let mock_requests = generate_mock_messages::<RequestMsg>();

        for mock_state in &mock_states {
            for processing_mode in &processing_modes {
                for mock_request in &mock_requests {
                    if validate_mock_server_configuration(mock_request, processing_mode, mock_state) {
                        debug!(target: "ext_proc_tests", "Test case #: {test_case_num}");
                        let (server_addr, server_handle) = start_mock_server(mock_state.clone()).await;
                        let mut config =
                            create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
                        config.observability_mode = false;
                        config.failure_mode_allow = false;
                        let mut ext_proc = ExternalProcessor::from(config);
                        let mut request = build_request_from_mock(mock_request);
                        let result = ext_proc.apply_request(&mut request).await;
                        server_handle.abort();
                        assert_matches!(result, FilterDecision::Continue);

                        let (parts, body) = request.into_parts();
                        let request_headers = parts.headers;
                        let collected = body.collect().await.unwrap();
                        let request_trailers = if let Some(trailers) = collected.trailers() {
                            trailers.clone()
                        } else {
                            http::HeaderMap::default()
                        };
                        let request_body = match collected.to_bytes() {
                            b if b.is_empty() => None,
                            b => Some(b),
                        };

                        assert_result(
                            test_case_num,
                            mock_request,
                            &request_headers,
                            request_body.as_ref(),
                            &request_trailers,
                            mock_state,
                            processing_mode,
                        );
                        test_case_num += 1;
                    }
                }
            }
        }
        println!("Total test cases executed: {test_case_num}");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_combinatorial_modes_continue() {
        let status = ResponseStatus::Continue;
        let mut test_case_num = 0;

        let mock_states = generate_mock_external_processors_states::<ResponseMsg>(status);
        let processing_modes = generate_processing_mode_configurations::<ResponseMsg>();
        let mock_responses = generate_mock_messages::<ResponseMsg>();

        for mock_state in &mock_states {
            for processing_mode in &processing_modes {
                for mock_response in &mock_responses {
                    if validate_mock_server_configuration(mock_response, processing_mode, mock_state) {
                        debug!(target: "ext_proc_tests", "Test case #: {test_case_num}");
                        let (server_addr, server_handle) = start_mock_server(mock_state.clone()).await;
                        let mut config =
                            create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
                        config.observability_mode = false;
                        config.failure_mode_allow = false;
                        let mut ext_proc = ExternalProcessor::from(config);
                        let mut response = build_response_from_mock(mock_response);
                        let result = ext_proc.apply_response(&mut response).await;
                        server_handle.abort();
                        assert_matches!(result, FilterDecision::Continue);

                        let (parts, body) = response.into_parts();
                        let response_headers = parts.headers;
                        let collected = body.collect().await.unwrap();
                        let response_trailers = if let Some(trailers) = collected.trailers() {
                            trailers.clone()
                        } else {
                            http::HeaderMap::default()
                        };
                        let response_body = match collected.to_bytes() {
                            b if b.is_empty() => None,
                            b => Some(b),
                        };

                        assert_result(
                            test_case_num,
                            mock_response,
                            &response_headers,
                            response_body.as_ref(),
                            &response_trailers,
                            mock_state,
                            processing_mode,
                        );
                        test_case_num += 1;
                    }
                }
            }
        }
        println!("Total test cases executed: {test_case_num}");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_combinatorial_modes_observability() {
        let mut test_case_num = 0;

        let mock_state = MockExternalProcessorState::new();
        let processing_modes = generate_processing_mode_configurations::<RequestMsg>();
        let mock_requests = generate_mock_messages::<RequestMsg>();

        for processing_mode in &processing_modes {
            for mock_request in &mock_requests {
                if validate_mock_server_configuration(mock_request, processing_mode, &mock_state) {
                    debug!(target: "ext_proc_tests", "Test case #: {test_case_num}");
                    let (server_addr, server_handle) = start_mock_server(mock_state.clone()).await;
                    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
                    config.observability_mode = true;
                    config.failure_mode_allow = false;
                    let mut ext_proc = ExternalProcessor::from(config);
                    let mut request = build_request_from_mock(mock_request);
                    let result = ext_proc.apply_request(&mut request).await;
                    server_handle.abort();
                    assert_matches!(result, FilterDecision::Continue);

                    let (parts, body) = request.into_parts();
                    let request_headers = parts.headers;
                    let collected = body.collect().await.unwrap();
                    let request_trailers = if let Some(trailers) = collected.trailers() {
                        trailers.clone()
                    } else {
                        http::HeaderMap::default()
                    };
                    let request_body = match collected.to_bytes() {
                        b if b.is_empty() => None,
                        b => Some(b),
                    };

                    assert_result(
                        test_case_num,
                        mock_request,
                        &request_headers,
                        request_body.as_ref(),
                        &request_trailers,
                        &mock_state,
                        processing_mode,
                    );
                    test_case_num += 1;
                }
            }
        }
        println!("Total test cases executed: {test_case_num}");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_combinatorial_modes_observability() {
        let mut test_case_num = 0;

        let mock_state = MockExternalProcessorState::new();
        let processing_modes = generate_processing_mode_configurations::<ResponseMsg>();
        let mock_responses = generate_mock_messages::<ResponseMsg>();

        for processing_mode in &processing_modes {
            for mock_response in &mock_responses {
                if validate_mock_server_configuration(mock_response, processing_mode, &mock_state) {
                    debug!(target: "ext_proc_tests", "Test case #: {test_case_num}");
                    let (server_addr, server_handle) = start_mock_server(mock_state.clone()).await;
                    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
                    config.observability_mode = true;
                    config.failure_mode_allow = false;
                    let mut ext_proc = ExternalProcessor::from(config);
                    let mut response = build_response_from_mock(mock_response);
                    let result = ext_proc.apply_response(&mut response).await;
                    server_handle.abort();
                    assert_matches!(result, FilterDecision::Continue);

                    let (parts, body) = response.into_parts();
                    let response_headers = parts.headers;
                    let collected = body.collect().await.unwrap();
                    let response_trailers = if let Some(trailers) = collected.trailers() {
                        trailers.clone()
                    } else {
                        http::HeaderMap::default()
                    };
                    let response_body = match collected.to_bytes() {
                        b if b.is_empty() => None,
                        b => Some(b),
                    };

                    assert_result(
                        test_case_num,
                        mock_response,
                        &response_headers,
                        response_body.as_ref(),
                        &response_trailers,
                        &mock_state,
                        processing_mode,
                    );
                    test_case_num += 1;
                }
            }
        }
        println!("Total test cases executed: {test_case_num}");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_mutation() {
        let mock_state = MockExternalProcessorState::new().add_response(create_headers_response::<RequestMsg>(
            vec![Some(("x-processed", "true")), Some(("x-custom-header", "custom-value"))],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: None,
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.headers().get("x-processed").unwrap(), "true");
        assert_eq!(request.headers().get("x-custom-header").unwrap(), "custom-value");
        assert_eq!(request.headers().get("content-type").unwrap(), "application/json");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_mutation_pseudo_headers() {
        let mock_state = MockExternalProcessorState::new().add_response(create_headers_response::<RequestMsg>(
            vec![
                Some((":method", "POST")),
                Some((":path", "/ext-proc-html-path")),
                Some((":scheme", "https")),
                Some((":authority", "ext-proc.com")),
            ],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: None,
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;
        let (parts, _) = request.into_parts();

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(parts.method.as_str(), "POST");
        assert_eq!(parts.uri.authority().unwrap().as_str(), "ext-proc.com");
        assert_eq!(parts.uri.scheme().unwrap().as_str(), "https");
        assert_eq!(parts.uri.path_and_query().unwrap().as_str(), "/ext-proc-html-path");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_trailer_mutation() {
        let mock_state = MockExternalProcessorState::new()
            .add_response(create_headers_response::<RequestMsg>(
                vec![],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_body_response::<RequestMsg>(
                vec![],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_trailers_response::<RequestMsg>(vec![
                Some(("x-processed", "true")),
                Some(("x-custom-trailer", "modified-value")),
            ]));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::Buffered,
            request_trailer_mode: TrailerProcessingMode::Send,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: Some("body"),
            trailers: vec![Some(("x-custom-trailer", "original-value"))],
            _marker: std::marker::PhantomData,
        });

        let result = ext_proc.apply_request(&mut request).await;
        let body = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap();
        let trailers = body.trailers().cloned();

        assert!(trailers.is_some());
        let trailers = trailers.unwrap();
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.headers().get("content-type").unwrap(), "application/json");
        assert_eq!(body.to_bytes(), "body");
        assert_eq!(trailers.get("x-processed").unwrap(), "true");
        assert_eq!(trailers.get("x-custom-trailer").unwrap(), "modified-value");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_body_buffered_continue_and_replace_on_headers_response() {
        let new_body = "modified body content";
        let mock_state = MockExternalProcessorState::new().add_response(create_headers_response::<RequestMsg>(
            vec![Some(("y-custom-header", "true"))],
            Some(new_body.as_bytes().into()),
            vec![],
            ResponseStatus::ContinueAndReplace as i32,
            None,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::Buffered,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![],
            body: Some("original body"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.method(), Method::GET);
        assert_eq!(request.headers().get("y-custom-header").unwrap(), "true");
        let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, new_body.as_bytes());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_body_buffered_continue_and_replace_on_body_response() {
        let new_body = "modified body content";
        let mock_state = MockExternalProcessorState::new()
            .add_response(create_headers_response::<RequestMsg>(
                vec![],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_body_response::<RequestMsg>(
                // TODO support headers modifications on body responses
                //vec![("y-custom-header", "true")],
                vec![],
                Some(new_body.as_bytes().into()),
                vec![],
                ResponseStatus::ContinueAndReplace as i32,
                None,
            ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::Buffered,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![],
            body: Some("original body"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.method(), Method::GET);
        //assert_eq!(request.headers().get("y-custom-header").unwrap(), "true");
        let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, new_body.as_bytes());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_body_buffered_mode() {
        let mock_state = MockExternalProcessorState::new()
            .add_response(create_headers_response::<RequestMsg>(
                vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_body_response::<RequestMsg>(
                vec![],
                Some("body data from external processor".as_bytes().into()),
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ));
        // starting mock with delay to compare output with the test that sets
        // send_body_without_waiting_for_header_response to true
        let (server_addr, _) = start_mock_server_with_delay(mock_state, Duration::from_millis(1)).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::Buffered,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![],
            body: Some("buffered body data"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");
        let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, "body data from external processor".as_bytes());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_body_buffered_mode_send_body_without_waiting_for_header_response() {
        let mock_state = MockExternalProcessorState::new()
            .add_response(create_headers_response::<RequestMsg>(
                vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_body_response::<RequestMsg>(
                vec![],
                Some("body data from external processor".as_bytes().into()),
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ));
        let (server_addr, _) = start_mock_server_with_delay(mock_state, Duration::from_millis(1)).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::Buffered,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        config.send_body_without_waiting_for_header_response = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![],
            body: Some("buffered body data"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");
        let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, "body data from external processor".as_bytes());
    }

    #[tokio::test]
    async fn test_request_body_streaming_mode() {
        let mock_state = MockExternalProcessorState::new()
            .add_response(create_headers_response::<RequestMsg>(
                vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                // even though we will stream the body, no body modification is
                // performed in the headers response so no need to set the flag
                None,
            ))
            .add_response(create_body_response::<RequestMsg>(
                vec![],
                Some("body data from external processor".as_bytes().into()),
                vec![],
                ResponseStatus::Continue as i32,
                // the response for the streaming body is still just a normal Body
                // no need to set end_of_stream, which is only for FULL_DUPLEX_STREAMED
                None,
            ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::Streamed,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![],
            body: Some("streaming body data"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");
        let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, "body data from external processor".as_bytes());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_header_mutation_pseudo_headers() {
        let mock_state = MockExternalProcessorState::new().add_response(create_headers_response::<ResponseMsg>(
            vec![Some((":status", "404"))],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: None,
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });
        let result = ext_proc.apply_response(&mut response).await;
        let (parts, _) = response.into_parts();

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(parts.status.as_str(), "404");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_body_streaming_mode() {
        let mock_state = MockExternalProcessorState::new()
            .add_response(create_headers_response::<ResponseMsg>(
                vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                // even though we will stream the body, no body modification is
                // performed in the headers response so no need to set the flag
                None,
            ))
            .add_response(create_body_response::<ResponseMsg>(
                vec![],
                Some("body data from external processor".as_bytes().into()),
                vec![],
                ResponseStatus::Continue as i32,
                // the response for the streaming body is still just a normal Body
                // no need to set end_of_stream, which is only for FULL_DUPLEX_STREAMED
                None,
            ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::Streamed,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response =
            build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_response(&mut response).await;

        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");
        let body_bytes = &mut response.body_mut().collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, "body data from external processor".as_bytes());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_body_streaming_mode_observability() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::Streamed,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = true;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response =
            build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_response(&mut response).await;

        assert_matches!(result, FilterDecision::Continue);
        let body_bytes = &mut response.body_mut().collect().await.unwrap().to_bytes();
        assert_eq!(body_bytes, "streaming body data".as_bytes());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_timeout() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request =
            build_request_from_mock(&Mock::<RequestMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::DirectResponse(dr) => {
            assert_eq!(dr.status(), http::StatusCode::GATEWAY_TIMEOUT);
        });
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_timeout_failure_mode_allow_true() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request =
            build_request_from_mock(&Mock::<RequestMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::Continue);

        let (_, body) = request.into_parts();
        let body_bytes = body.inner.collect().await;

        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "streaming body data".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_body_timeout_failure_mode_allow_false() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::Streamed,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request =
            build_request_from_mock(&Mock::<RequestMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::Continue);

        let body_bytes = &mut request.body_mut().collect().await;
        assert!(body_bytes.is_err());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_body_timeout_failure_mode_allow_true() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::Streamed,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request =
            build_request_from_mock(&Mock::<RequestMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::Continue);

        let (_, body) = request.into_parts();
        let body_bytes = body.inner.collect().await;

        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "streaming body data".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_header_timeout() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response =
            build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::DirectResponse(dr) => {
            assert_eq!(dr.status(), http::StatusCode::GATEWAY_TIMEOUT);
        });
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_header_timeout_failure_mode_allow_true() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response =
            build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::Continue);

        let (_, body) = response.into_parts();
        let body_bytes = body.collect().await;

        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "streaming body data".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_body_timeout_failure_mode_allow_false() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::Streamed,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response =
            build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::Continue);

        let body_bytes = &mut response.body_mut().collect().await;
        assert!(body_bytes.is_err());
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_body_timeout_failure_mode_allow_true() {
        let mock_state = MockExternalProcessorState::new();
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::Streamed,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.observability_mode = false;
        config.failure_mode_allow = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response =
            build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::Continue);

        let (_, body) = response.into_parts();
        let body_bytes = body.collect().await;

        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "streaming body data".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_immediate_response_request_header() {
        let mock_state = MockExternalProcessorState::new().add_response(create_immediate_response(
            vec![Some(("x-immediate-header", "immediate value"))],
            Some("immediate body".as_bytes().into()),
            302,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.disable_immediate_response = false;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request =
            build_request_from_mock(&Mock::<RequestMsg>::new(vec![], Some("streaming body data"), vec![]));
        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::DirectResponse(dr) => {
            assert_eq!(dr.status(), StatusCode::from_u16(302).unwrap());
            assert_eq!(dr.headers().get("x-immediate-header").unwrap(), "immediate value");
            let (_, body) = dr.into_parts();
            let body_bytes = body.collect().await.unwrap().to_bytes();
            assert_eq!(body_bytes, "immediate body".as_bytes());
        });
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_immediate_response_request_header_disable_immediate_response() {
        let mock_state = MockExternalProcessorState::new().add_response(create_immediate_response(
            vec![Some(("x-immediate-header", "immediate value"))],
            Some("immediate body".as_bytes().into()),
            302,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.failure_mode_allow = false;
        config.disable_immediate_response = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg>::new(
            vec![Some(("x-test-header", "original value"))],
            Some("streaming body data"),
            vec![],
        ));
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::DirectResponse(dr) => {
            assert_eq!(dr.status(), StatusCode::from_u16(500).unwrap());
        });
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_immediate_response_request_header_disable_immediate_response_failure_allow_mode() {
        let mock_state = MockExternalProcessorState::new().add_response(create_immediate_response(
            vec![Some(("x-immediate-header", "immediate value"))],
            Some("immediate body".as_bytes().into()),
            302,
        ));
        let (server_addr, _) = start_mock_server(mock_state).await;
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.failure_mode_allow = true;
        config.disable_immediate_response = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg>::new(
            vec![Some(("x-test-header", "original value"))],
            Some("streaming body data"),
            vec![],
        ));
        let result = ext_proc.apply_request(&mut request).await;

        assert_matches!(result, FilterDecision::Continue);
        let (parts, body) = request.into_parts();
        assert_eq!(parts.headers.get("x-test-header").unwrap(), "original value");
        let body_bytes = body.collect().await;
        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "streaming body data".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_override_request_body() {
        // Original processing mode
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        // ext-proc override
        let mut header_response =
            create_headers_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None);
        header_response.mode_override = Some(EnvoyProcessingMode {
            request_header_mode: processing_mode::HeaderSendMode::Send as i32,
            request_body_mode: processing_mode::BodySendMode::Buffered as i32,
            request_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
            response_header_mode: processing_mode::HeaderSendMode::Skip as i32,
            response_body_mode: processing_mode::BodySendMode::None as i32,
            response_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
        });
        let mock_state =
            MockExternalProcessorState::new().add_response(header_response).add_response(create_body_response::<
                RequestMsg,
            >(
                vec![],
                Some("ext-proc body".into()),
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ));
        let (server_addr, _) = start_mock_server(mock_state).await;

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.allow_mode_override = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: Some("original body"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });

        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::Continue);

        assert_eq!(request.headers().get("content-type").unwrap(), "application/json");

        let (_, body) = request.into_parts();
        let body_bytes = body.collect().await;
        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "ext-proc body".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_override_request_trailers() {
        // Original processing mode
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        // ext-proc override
        let mut header_response =
            create_headers_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None);
        header_response.mode_override = Some(EnvoyProcessingMode {
            request_header_mode: processing_mode::HeaderSendMode::Send as i32,
            request_body_mode: processing_mode::BodySendMode::None as i32,
            request_trailer_mode: processing_mode::HeaderSendMode::Send as i32,
            response_header_mode: processing_mode::HeaderSendMode::Skip as i32,
            response_body_mode: processing_mode::BodySendMode::None as i32,
            response_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
        });
        let mock_state =
            MockExternalProcessorState::new().add_response(header_response).add_response(create_trailers_response::<
                RequestMsg,
            >(vec![Some((
                "x-ext-proc-trailer",
                "ext-proc trailer value",
            ))]));
        let (server_addr, _) = start_mock_server(mock_state).await;

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.allow_mode_override = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: None,
            trailers: vec![Some(("x-original-trailer", "original trailer value"))],
            _marker: std::marker::PhantomData,
        });

        let result = ext_proc.apply_request(&mut request).await;
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(request.headers().get("content-type").unwrap(), "application/json");

        let body = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap();
        let trailers = body.trailers().cloned();

        assert!(trailers.is_some());
        let trailers = trailers.unwrap();
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(trailers.get("x-ext-proc-trailer").unwrap(), "ext-proc trailer value");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_header_override_response_body() {
        // Original processing mode
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        // ext-proc override
        let mut header_response =
            create_headers_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None);
        header_response.mode_override = Some(EnvoyProcessingMode {
            request_header_mode: processing_mode::HeaderSendMode::Skip as i32,
            request_body_mode: processing_mode::BodySendMode::None as i32,
            request_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
            response_header_mode: processing_mode::HeaderSendMode::Send as i32,
            response_body_mode: processing_mode::BodySendMode::Buffered as i32,
            response_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
        });
        let mock_state =
            MockExternalProcessorState::new().add_response(header_response).add_response(create_body_response::<
                ResponseMsg,
            >(
                vec![],
                Some("ext-proc body".into()),
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ));
        let (server_addr, _) = start_mock_server(mock_state).await;

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.allow_mode_override = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: Some("original body"),
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });

        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::Continue);

        assert_eq!(response.headers().get("content-type").unwrap(), "application/json");
        let (_, body) = response.into_parts();
        let body_bytes = body.collect().await;
        assert!(body_bytes.is_ok());
        if let Ok(bytes) = body_bytes {
            assert_eq!(bytes.to_bytes(), "ext-proc body".as_bytes());
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_response_header_override_response_trailers() {
        // Original processing mode
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Skip,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Send,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        // ext-proc override
        let mut header_response =
            create_headers_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None);
        header_response.mode_override = Some(EnvoyProcessingMode {
            request_header_mode: processing_mode::HeaderSendMode::Skip as i32,
            request_body_mode: processing_mode::BodySendMode::None as i32,
            request_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
            response_header_mode: processing_mode::HeaderSendMode::Send as i32,
            response_body_mode: processing_mode::BodySendMode::None as i32,
            response_trailer_mode: processing_mode::HeaderSendMode::Send as i32,
        });
        let mock_state =
            MockExternalProcessorState::new().add_response(header_response).add_response(create_trailers_response::<
                ResponseMsg,
            >(vec![Some((
                "x-ext-proc-trailer",
                "ext-proc trailer value",
            ))]));
        let (server_addr, _) = start_mock_server(mock_state).await;

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.allow_mode_override = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: None,
            trailers: vec![Some(("x-original-trailer", "original trailer value"))],
            _marker: std::marker::PhantomData,
        });

        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(response.headers().get("content-type").unwrap(), "application/json");

        let (_, body) = response.into_parts();
        let collected_body = body.collect().await.unwrap();
        let trailers = collected_body.trailers().cloned();
        assert!(trailers.is_some());
        let trailers = trailers.unwrap();
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(trailers.get("x-ext-proc-trailer").unwrap(), "ext-proc trailer value");
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_request_header_override_response_header() {
        // Original processing mode
        let processing_mode = ProcessingMode {
            request_header_mode: HeaderProcessingMode::Send,
            request_body_mode: BodyProcessingMode::None,
            request_trailer_mode: TrailerProcessingMode::Skip,
            response_header_mode: HeaderProcessingMode::Skip,
            response_body_mode: BodyProcessingMode::None,
            response_trailer_mode: TrailerProcessingMode::Skip,
        };

        // ext-proc override
        let mut request_header_response =
            create_headers_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None);
        request_header_response.mode_override = Some(EnvoyProcessingMode {
            request_header_mode: processing_mode::HeaderSendMode::Send as i32,
            request_body_mode: processing_mode::BodySendMode::None as i32,
            request_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
            response_header_mode: processing_mode::HeaderSendMode::Send as i32,
            response_body_mode: processing_mode::BodySendMode::Buffered as i32,
            response_trailer_mode: processing_mode::HeaderSendMode::Send as i32,
        });
        let mock_state = MockExternalProcessorState::new()
            .add_response(request_header_response)
            .add_response(create_headers_response::<ResponseMsg>(
                vec![Some(("x-custom-header", "ext-proc header value"))],
                None,
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_body_response::<ResponseMsg>(
                vec![],
                Some("ext-proc body value".into()),
                vec![],
                ResponseStatus::Continue as i32,
                None,
            ))
            .add_response(create_trailers_response::<ResponseMsg>(vec![Some((
                "x-custom-trailer",
                "ext-proc trailer value",
            ))]));
        let (server_addr, _) = start_mock_server(mock_state).await;

        let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
        config.allow_mode_override = true;
        let mut ext_proc = ExternalProcessor::from(config);

        let mut request = build_request_from_mock(&Mock::<RequestMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: None,
            trailers: vec![],
            _marker: std::marker::PhantomData,
        });

        let request_result = ext_proc.apply_request(&mut request).await;
        assert_matches!(request_result, FilterDecision::Continue);
        assert_eq!(request.headers().get("content-type").unwrap(), "application/json");

        let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
            headers: vec![Some(("content-type", "application/json"))],
            body: Some("original response body"),
            trailers: vec![Some(("x-custom-trailer", "original trailer value"))],
            _marker: std::marker::PhantomData,
        });

        let result = ext_proc.apply_response(&mut response).await;
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(response.headers().get("content-type").unwrap(), "application/json");
        assert_eq!(response.headers().get("x-custom-header").unwrap(), "ext-proc header value");

        let (_, body) = response.into_parts();
        let collected_body = body.collect().await.unwrap();
        let trailers = collected_body.trailers().cloned();
        assert!(trailers.is_some());
        assert_eq!(collected_body.to_bytes(), "ext-proc body value");
        let trailers = trailers.unwrap();
        assert_matches!(result, FilterDecision::Continue);
        assert_eq!(trailers.get("x-custom-trailer").unwrap(), "ext-proc trailer value");
    }
}
