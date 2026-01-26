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
use http_body_util::{BodyExt, Empty, StreamBody};
use orion_configuration::config::network_filters::http_connection_manager::http_filters::ext_proc::{
    BodyProcessingMode, ExternalProcessor as ExternalProcessorConfig, GoogleGrpc, GrpcService, GrpcServiceSpecifier,
    HeaderProcessingMode, ProcessingMode, RouteCacheAction, TrailerProcessingMode,
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
            processing_request::Request as ProcessingRequestType,
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
use std::{collections::VecDeque, convert::Infallible, net::SocketAddr, str::FromStr, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};

use tokio::select;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

pub struct OutState {
    last_end_of_stream: Option<bool>,
}

#[derive(Clone)]
pub struct MockExternalProcessorState {
    responses: VecDeque<ProcessingResponse>,
    token: CancellationToken,
    sender: Option<Sender<OutState>>,
}

impl std::fmt::Debug for MockExternalProcessorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockExternalProcessorState")
            .field("responses", &self.responses)
            .field("token", &self.token)
            .finish()
    }
}

impl MockExternalProcessorState {
    pub fn new() -> Self {
        Self { responses: VecDeque::new(), token: CancellationToken::new(), sender: None }
    }

    pub fn add_response(mut self, response: ProcessingResponse) -> Self {
        self.responses.push_back(response);
        self
    }

    pub fn get_next_response(&mut self) -> Option<ProcessingResponse> {
        self.responses.pop_front()
    }

    pub fn with_sender(mut self, sender: Sender<OutState>) -> Self {
        self.sender = Some(sender);
        self
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
        let mut last_end_of_stream = None;

        tokio::spawn(async move {
            loop {
                select! {
                     result_msg = inbound.message() => {
                        let request = match result_msg {
                                            Ok(Some(req)) => req,
                                            Ok(None) => break, // Stream ended naturally
                                            Err(_) => break,   // Error in stream
                                      };

                        let end_of_stream = request.request.map(|req| {
                            match req {
                               ProcessingRequestType::RequestHeaders(hdrs) | ProcessingRequestType::ResponseHeaders(hdrs) => {
                                   Some(hdrs.end_of_stream)
                               }
                               ProcessingRequestType::RequestBody(http_body) | ProcessingRequestType::ResponseBody(http_body) => {
                                   Some(http_body.end_of_stream)
                               },
                               ProcessingRequestType::RequestTrailers(_) | ProcessingRequestType::ResponseTrailers(_) => None,
                            }
                        }).flatten();

                        if end_of_stream.is_some() {
                            last_end_of_stream = end_of_stream;
                        }

                        if let Some(response) = state.get_next_response() {
                            if let Some(delay) = delay {
                                tokio::time::sleep(delay).await;
                            }
                            if tx.send(Ok(response)).await.is_err() {
                                break;
                            }
                        }
                     }
                     _ = state.token.cancelled() => {
                         break;
                     }
                }
            }

            if let Some(ref sender) = state.sender {
                _ = sender.send(OutState { last_end_of_stream }).await;
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
    body: Vec<&'static str>,
    trailers: Vec<Option<(&'static str, &'static str)>>,
    _marker: std::marker::PhantomData<M>,
}

impl<M: MsgKind> Mock<M> {
    fn new(
        headers: Vec<Option<(&'static str, &'static str)>>,
        body: Vec<&'static str>,
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

    fn apply_body_mutation(&self, body_mutation: Option<&Mutation>, idx: usize) -> Bytes {
        let body_replacement: Option<Bytes> = match body_mutation {
            Some(Mutation::Body(bytes)) => Some(bytes.clone().into()),
            Some(Mutation::ClearBody(true)) => Some(Bytes::new()),
            Some(Mutation::ClearBody(false)) | None => None,
            Some(Mutation::StreamedResponse(chunk)) => Some(chunk.clone().body.into()),
        };

        if let Some(chunk) = self.body.get(idx) {
            let b = *chunk;
            body_replacement.or(Some(b.into())).unwrap()
        } else {
            body_replacement.unwrap()
        }
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

async fn build_request_from_mock(mock_request: &Mock<RequestMsg>) -> Request<BodyWithMetrics<PolyBody>> {
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

    let body = create_collected_body_with_trailers(
        mock_request.body.clone(),
        if trailers_map.is_empty() { None } else { Some(trailers_map) },
    )
    .await;

    req.body(BodyWithMetrics::new(BodyKind::Request, body, |_, _, _| {})).unwrap()
}

async fn build_response_from_mock(mock_response: &Mock<ResponseMsg>) -> Response<PolyBody> {
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

    let body = create_collected_body_with_trailers(
        mock_response.body.clone(),
        if trailers_map.is_empty() { None } else { Some(trailers_map) },
    )
    .await;

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
        message_timeout: Some(Duration::from_secs(5)),
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
                    header: Some(EnvoyHeaderValue { key: key.to_owned(), value: value.to_owned(), raw_value: vec![] }),
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
        BodyMutation {
            mutation: Some(Mutation::StreamedResponse(StreamedBodyResponse {
                body,
                end_of_stream,
                end_of_stream_without_message: false,
                grpc_message_compressed: false,
            })),
        }
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
            .map(|(key, value)| EnvoyHeaderValue { key: key.to_owned(), value: value.to_owned(), raw_value: vec![] })
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
        ..Default::default()
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

    ProcessingResponse { response, ..Default::default() }
}

pub async fn create_collected_body_with_trailers(frames: Vec<&str>, trailers: Option<http::HeaderMap>) -> PolyBody {
    let mut chunks: Vec<Result<http_body::Frame<Bytes>, Infallible>> = Vec::new();

    for frame in frames {
        let chunk_bytes = Bytes::from(frame.to_owned());
        chunks.push(Ok(http_body::Frame::data(chunk_bytes)));
    }

    if let Some(t_map) = trailers {
        chunks.push(Ok(http_body::Frame::trailers(t_map)));
    }

    // Collected body does not implement is_end_stream method, which always returns false by default (even for empty bodies).
    // For this reason, we create the Collected body only when the vector of chunks is not empty.
    // Empty instead implements is_end_stream correctly which always returns true.

    if chunks.is_empty() {
        PolyBody::from(Empty::new())
    } else {
        let body_stream = futures_util::stream::iter(chunks);
        PolyBody::from(BodyExt::collect(StreamBody::new(body_stream)).await.unwrap())
    }
}

pub async fn to_body_data_chunks(mut body: Collected<Bytes>) -> Vec<Bytes> {
    let mut chunks = Vec::new();
    while let Some(frame_result) = body.frame().await {
        let Ok(frame) = frame_result;
        if let Some(data) = frame.data_ref() {
            chunks.push(data.clone());
        }
    }

    chunks
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

    ProcessingResponse { response, ..Default::default() }
}

fn create_trailers_response<M: MsgKind>(trailers: Vec<Option<(&str, &str)>>) -> ProcessingResponse {
    let trailer_mutation = transform(trailers).and_then(|trls| create_trailer_mutation(trls));

    let trailers_response = TrailersResponse { header_mutation: trailer_mutation };
    let response = if M::IS_RESPONSE {
        Some(ProcessingResponseType::ResponseTrailers(trailers_response))
    } else {
        Some(ProcessingResponseType::RequestTrailers(trailers_response))
    };
    ProcessingResponse { response, ..Default::default() }
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
static REQUEST_BODIES: [&[&str]; 2] = [&[], &["original body"]];
static REQUEST_TRAILERS: [Option<(&str, &str)>; 2] = [None, Some(("x-test-trailer", "original-trailer-value"))];

fn generate_mock_messages<M: MsgKind>() -> Vec<Mock<M>> {
    REQUEST_HEADERS
        .iter()
        .flat_map(|&headers| {
            REQUEST_BODIES.iter().flat_map(move |&body| {
                REQUEST_TRAILERS.iter().map(move |&trailers| Mock::<M> {
                    headers: vec![headers],
                    body: body.into(),
                    trailers: vec![trailers],
                    _marker: std::marker::PhantomData,
                })
            })
        })
        .collect()
}

static HEADER_MODIFICATIONS: [Option<(&str, &str)>; 2] = [None, Some(("x-test-header", "ext-proc-header-value"))];
static BODY_MODIFICATIONS: [Option<&str>; 2] = [None, Some("external processor body")];
static TRAILER_MODIFICATIONS: [Option<(&str, &str)>; 2] = [None, Some(("x-test-trailer", "ext-proc-trailer-value"))];

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
    let has_body = !mock_msg.body.is_empty();
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
            ProcessingResponseType::StreamedImmediateResponse(_) => return false,
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn assert_result<M>(
    test_case_num: i32,
    mock: &Mock<M>,
    headers: &http::HeaderMap,
    body: Vec<Bytes>,
    trailers: &http::HeaderMap,
    mock_state: &MockExternalProcessorState,
    processing_mode: &ProcessingMode,
) where
    M: std::fmt::Debug + MsgKind + ModeSelector,
{
    // ProcessingMode predicates
    let has_headers = transform(mock.headers.clone()).is_some();
    let has_body = !mock.body.is_empty();
    let has_trailers = transform(mock.trailers.clone()).is_some();

    let should_send_headers =
        has_headers && !matches!(<M as ModeSelector>::header_mode(processing_mode), HeaderProcessingMode::Skip);
    let should_send_body =
        has_body && !matches!(<M as ModeSelector>::body_mode(processing_mode), BodyProcessingMode::None);
    let should_send_trailers =
        has_trailers && matches!(<M as ModeSelector>::trailer_mode(processing_mode), TrailerProcessingMode::Send);

    // Compute expected values to assert over modified request
    let mut expected_headers = mock.orig_headers_map();
    let mut expected_body: Vec<Bytes> = mock.body.clone().into_iter().map(Into::into).collect::<Vec<_>>();
    let mut expected_trailers = mock.orig_trailers_map();
    let mut idx = 0;
    for response_opt in &mock_state.responses {
        if let Some(response) = response_opt.response.as_ref() {
            match response {
                ProcessingResponseType::RequestHeaders(HeadersResponse { response: Some(common_response) })
                | ProcessingResponseType::ResponseHeaders(HeadersResponse { response: Some(common_response) }) => {
                    let mutation = should_send_headers.then_some(common_response.header_mutation.as_ref()).flatten();
                    expected_headers = mock.apply_header_mutation(mutation);
                },
                ProcessingResponseType::RequestBody(BodyResponse {
                    response: Some(CommonResponse { body_mutation: Some(BodyMutation { mutation }), .. }),
                })
                | ProcessingResponseType::ResponseBody(BodyResponse {
                    response: Some(CommonResponse { body_mutation: Some(BodyMutation { mutation }), .. }),
                }) => {
                    let mutation = should_send_body.then_some(mutation.as_ref()).flatten();
                    expected_body[idx] = mock.apply_body_mutation(mutation, idx);
                    idx += 1;
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
        expected_body,
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
                    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
                    config.observability_mode = false;
                    config.failure_mode_allow = false;
                    let mut ext_proc = ExternalProcessor::from(config);
                    let mut request = build_request_from_mock(mock_request).await;
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

                    let request_body = to_body_data_chunks(collected).await;

                    assert_result(
                        test_case_num,
                        mock_request,
                        &request_headers,
                        request_body,
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
                    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode.clone());
                    config.observability_mode = false;
                    config.failure_mode_allow = false;
                    let mut ext_proc = ExternalProcessor::from(config);
                    let mut response = build_response_from_mock(mock_response).await;
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

                    let response_body = to_body_data_chunks(collected).await;

                    assert_result(
                        test_case_num,
                        mock_response,
                        &response_headers,
                        response_body,
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
                let mut request = build_request_from_mock(mock_request).await;
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

                let request_body = to_body_data_chunks(collected).await;

                assert_result(
                    test_case_num,
                    mock_request,
                    &request_headers,
                    request_body,
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
                let mut response = build_response_from_mock(mock_response).await;
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

                let response_body = to_body_data_chunks(collected).await;

                assert_result(
                    test_case_num,
                    mock_response,
                    &response_headers,
                    response_body,
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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None))
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
        body: vec!["body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec!["original body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec!["original body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.method(), Method::GET);
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
        body: vec!["buffered body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
    config.send_body_without_waiting_for_header_response = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![],
        body: vec!["buffered body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec!["streaming body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());
}

#[tokio::test]
#[test_log::test]
async fn test_request_full_duplex_streaming_mode_header_only() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Send,
        request_body_mode: BodyProcessingMode::FullDuplexStreamed,
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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_full_duplex_streaming_mode_with_body() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Send,
        request_body_mode: BodyProcessingMode::FullDuplexStreamed,
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
        body: vec!["this is the body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_full_duplex_streaming_mode_with_body_and_trailers() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .add_response(create_trailers_response::<RequestMsg>(vec![]))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Send,
        request_body_mode: BodyProcessingMode::FullDuplexStreamed,
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
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(false), "Expected end of stream false");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_buffered_mode_header_only() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_buffered_mode_with_body() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

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
        body: vec!["this is the body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_buffered_mode_with_body_and_trailers() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .add_response(create_trailers_response::<RequestMsg>(vec![]))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

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
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(false), "Expected end of stream false");
    }
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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
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
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["streaming body data"], vec![])).await;

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
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
async fn test_request_multichunk_body_with_mutation() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_body_response::<RequestMsg>(
            vec![],
            Some("CHUNK1".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(
            vec![],
            Some("CHUNK2".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(
            vec![],
            Some("CHUNK3".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));

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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut request =
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["chunk1", "chunk2", "chunk3"], vec![])).await;
    let result = ext_proc.apply_request(&mut request).await;
    assert_matches!(result, FilterDecision::Continue);

    let body_chunks = to_body_data_chunks(std::mem::take(&mut request.body_mut().inner).collect().await.unwrap()).await;

    assert_eq!(body_chunks, ["CHUNK1", "CHUNK2", "CHUNK3"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>());
}

#[tokio::test]
#[test_log::test]
async fn test_request_multichunk_body_timeout_failure_mode_allow_true() {
    let mock_state = MockExternalProcessorState::new().add_response(create_body_response::<RequestMsg>(
        vec![],
        Some("modified_chunk".into()),
        vec![],
        ResponseStatus::Continue as i32,
        None,
    ));
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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut request =
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["chunk1", "chunk2", "chunk3"], vec![])).await;
    let result = ext_proc.apply_request(&mut request).await;
    assert_matches!(result, FilterDecision::Continue);

    let body_chunks = to_body_data_chunks(std::mem::take(&mut request.body_mut().inner).collect().await.unwrap()).await;
    assert_eq!(
        body_chunks,
        ["modified_chunk", "chunk2", "chunk3"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>()
    );
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
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
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
        vec!["streaming body data"],
        vec![],
    ))
    .await;
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
        vec!["streaming body data"],
        vec![],
    ))
    .await;
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
    let mock_state = MockExternalProcessorState::new().add_response(header_response).add_response(
        create_body_response::<RequestMsg>(
            vec![],
            Some("ext-proc body".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ),
    );
    let (server_addr, _) = start_mock_server(mock_state).await;

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.allow_mode_override = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec![],
        trailers: vec![Some(("x-original-trailer", "original trailer value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

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
    let mock_state = MockExternalProcessorState::new().add_response(header_response).add_response(
        create_body_response::<ResponseMsg>(
            vec![],
            Some("ext-proc body".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ),
    );
    let (server_addr, _) = start_mock_server(mock_state).await;

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.allow_mode_override = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec![],
        trailers: vec![Some(("x-original-trailer", "original trailer value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let request_result = ext_proc.apply_request(&mut request).await;
    assert_matches!(request_result, FilterDecision::Continue);
    assert_eq!(request.headers().get("content-type").unwrap(), "application/json");

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original response body"],
        trailers: vec![Some(("x-custom-trailer", "original trailer value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

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

#[tokio::test]
#[test_log::test]
async fn test_request_body_buffered_too_large() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<RequestMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));

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

    let very_large_body = "a".repeat(10 * 1024 * 1024); // 10 MB body
    let boxed_str: Box<str> = very_large_body.clone().into_boxed_str();

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![],
        body: vec![Box::leak(boxed_str)],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::DirectResponse(_));
    match result {
        FilterDecision::DirectResponse(response) => {
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        },
        _ => assert!(false, "Unexpected filter decision"),
    }
}

#[tokio::test]
async fn test_request_multichunk_merged_body_buffered_mode() {
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
        body: vec!["streaming", "body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-stream-processed").unwrap(), "true");
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());
}

use futures::task::noop_waker;
use http_body::Body as HttpBody;
use std::pin::Pin;
use std::task::Context;

async fn assert_body_frames<B: HttpBody + Unpin>(
    mut body: B,
    expected_data: &[Bytes],
    expected_trailers: Option<http::HeaderMap>,
) -> Result<(), String>
where
    B: HttpBody<Data = Bytes>,
    B::Error: Sized + Unpin + std::fmt::Debug,
{
    // Setup for manual polling
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    let mut pinned_body = Pin::new(&mut body);
    let mut data_index = 0;
    let mut trailers_seen = None;

    loop {
        // Simulates the call to poll_frame from the asynchronous runtime
        let poll = pinned_body.as_mut().poll_frame(&mut cx);

        match poll {
            // Case 1: Frame is ready
            std::task::Poll::Ready(maybe_frame) => {
                match maybe_frame {
                    // Body is fully consumed: End the loop
                    None => {
                        break;
                    },

                    Some(Ok(frame)) => {
                        if frame.is_data() {
                            // Control 1: DATA Frame
                            if trailers_seen.is_some() {
                                return Err(format!("Error: DATA received after TRAILERS. Data Index: {}", data_index));
                            }

                            // Optional: Verification of DATA content
                            let data = frame.into_data().unwrap();

                            if data_index < expected_data.len() {
                                if data != expected_data[data_index] {
                                    return Err(format!(
                                        "Error: DATA content mismatch for chunk {}, frame received: {:?}, expected: {:?}",
                                        data_index, data, expected_data[data_index]
                                    ));
                                }
                            } else {
                                // Received more DATA than expected (malformed)
                                return Err("Error: More DATA chunks than expected.".to_string());
                            }

                            data_index += 1;
                        } else if frame.is_trailers() {
                            // Control 2: TRAILERS Frame
                            if trailers_seen.is_some() {
                                return Err("Error: Received a second Frame::trailers.".to_string());
                            }
                            if data_index < expected_data.len() {
                                // Trailer frame received before all expected data chunks were seen
                                return Err("Error: TRAILERS received prematurely before all DATA.".to_string());
                            }

                            trailers_seen = Some(frame.into_trailers().unwrap());
                        } else {
                            // Unexpected frame type
                            return Err(format!("DEBUG: Received unexpected Frame: {:?}", frame));
                        }
                    },

                    Some(Err(e)) => {
                        // Error occurred while reading the body
                        return Err(format!("Body Error: {:?}", e));
                    },
                }
            },

            // Case 2: Not ready, Body is pending (waiting for data)
            std::task::Poll::Pending => {
                // Even if we are in a test, ext_proc uses a ChannelBody
                // that might delay delivery of data chunks, so we could hit
                // a Poll::Pending. If that happens, yield to allow the body
                // to make progress.
                tokio::task::yield_now().await;
            },
        }
    }

    if trailers_seen != expected_trailers {
        return Err(format!("Trailers {trailers_seen:?} did not match expected state. Expected {expected_trailers:?}"));
    }

    // Final check: ensure all expected data was processed
    if data_index == expected_data.len() {
        Ok(())
    } else {
        Err("Body was not consumed completely.".to_string())
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_body_and_trailer_out_of_order() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<RequestMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None))
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None))
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));

    let (server_addr, _) = start_mock_server_with_delay(mock_state, Duration::from_millis(500)).await;
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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["chunk1", "chunk2", "chunk3"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;
    assert_matches!(result, FilterDecision::Continue);

    let body = std::mem::take(&mut request.body_mut().inner);

    let expected_trailers = http::HeaderMap::from_iter(vec![(
        http::HeaderName::from_static("x-custom-trailer"),
        http::HeaderValue::from_static("original-value"),
    )]);
    let expected_data = &["chunk1".into(), "chunk2".into(), "chunk3".into()];

    assert_matches!(assert_body_frames(body, expected_data, Some(expected_trailers)).await, Ok(()));
}

#[tokio::test]
#[test_log::test]
async fn test_request_multichunk_body_no_truncate_body() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_body_response::<RequestMsg>(
            vec![],
            Some("CHUNK1".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(
            vec![],
            Some("CHUNK2".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(
            vec![],
            Some("CHUNK3".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));

    let (server_addr, _) = start_mock_server_with_delay(mock_state, Duration::from_millis(500)).await;
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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut request =
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["chunk1", "chunk2", "chunk3"], vec![])).await;
    let result = ext_proc.apply_request(&mut request).await;
    assert_matches!(result, FilterDecision::Continue);

    let body_chunks = to_body_data_chunks(std::mem::take(&mut request.body_mut().inner).collect().await.unwrap()).await;

    assert_eq!(body_chunks, ["CHUNK1", "CHUNK2", "CHUNK3"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>());
}

#[tokio::test]
#[test_log::test]
async fn test_response_header_mutation() {
    let mock_state = MockExternalProcessorState::new().add_response(create_headers_response::<ResponseMsg>(
        vec![Some(("x-processed", "true")), Some(("x-custom-header", "custom-value"))],
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
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-processed").unwrap(), "true");
    assert_eq!(response.headers().get("x-custom-header").unwrap(), "custom-value");
    assert_eq!(response.headers().get("content-type").unwrap(), "application/json");
}

#[tokio::test]
#[test_log::test]
async fn test_response_trailer_mutation() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None))
        .add_response(create_trailers_response::<ResponseMsg>(vec![
            Some(("x-processed", "true")),
            Some(("x-custom-trailer", "modified-value")),
        ]));
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Send,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;
    let (parts, body) = response.into_parts();
    let collected = body.collect().await.unwrap();
    let trailers = collected.trailers().cloned();

    assert!(trailers.is_some());
    let trailers = trailers.unwrap();
    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(parts.headers.get("content-type").unwrap(), "application/json");
    assert_eq!(collected.to_bytes(), "body");
    assert_eq!(trailers.get("x-processed").unwrap(), "true");
    assert_eq!(trailers.get("x-custom-trailer").unwrap(), "modified-value");
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_buffered_continue_and_replace_on_headers_response() {
    let new_body = "modified body content";
    let mock_state = MockExternalProcessorState::new().add_response(create_headers_response::<ResponseMsg>(
        vec![Some(("y-custom-header", "true"))],
        Some(new_body.as_bytes().into()),
        vec![],
        ResponseStatus::ContinueAndReplace as i32,
        None,
    ));
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["original body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("y-custom-header").unwrap(), "true");
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, new_body.as_bytes());
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_buffered_continue_and_replace_on_body_response() {
    let new_body = "modified body content";
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some(new_body.as_bytes().into()),
            vec![],
            ResponseStatus::ContinueAndReplace as i32,
            None,
        ));
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["original body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, new_body.as_bytes());
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_buffered_mode() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("body data from external processor".as_bytes().into()),
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
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["buffered body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_buffered_mode_send_body_without_waiting_for_header_response() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("body data from external processor".as_bytes().into()),
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
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    config.send_body_without_waiting_for_header_response = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["buffered body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());
}

#[tokio::test]
#[test_log::test]
async fn test_request_body_streaming_mode_observability() {
    let mock_state = MockExternalProcessorState::new();
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
    config.observability_mode = true;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request =
        build_request_from_mock(&Mock::<RequestMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "streaming body data".as_bytes());
}

#[tokio::test]
#[test_log::test]
async fn test_immediate_response_response_header() {
    let mock_state = MockExternalProcessorState::new().add_response(create_immediate_response(
        vec![Some(("x-immediate-header", "immediate value"))],
        Some("immediate body".as_bytes().into()),
        302,
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
    config.disable_immediate_response = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response =
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["streaming body data"], vec![])).await;
    let result = ext_proc.apply_response(&mut response).await;
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
async fn test_immediate_response_response_header_disable_immediate_response() {
    let mock_state = MockExternalProcessorState::new().add_response(create_immediate_response(
        vec![Some(("x-immediate-header", "immediate value"))],
        Some("immediate body".as_bytes().into()),
        302,
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
    config.failure_mode_allow = false;
    config.disable_immediate_response = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg>::new(
        vec![Some(("x-test-header", "original value"))],
        vec!["streaming body data"],
        vec![],
    ))
    .await;
    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::DirectResponse(dr) => {
        assert_eq!(dr.status(), StatusCode::from_u16(500).unwrap());
    });
}

#[tokio::test]
#[test_log::test]
async fn test_immediate_response_response_header_disable_immediate_response_failure_allow_mode() {
    let mock_state = MockExternalProcessorState::new().add_response(create_immediate_response(
        vec![Some(("x-immediate-header", "immediate value"))],
        Some("immediate body".as_bytes().into()),
        302,
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
    config.failure_mode_allow = true;
    config.disable_immediate_response = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg>::new(
        vec![Some(("x-test-header", "original value"))],
        vec!["streaming body data"],
        vec![],
    ))
    .await;
    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    let (parts, body) = response.into_parts();
    assert_eq!(parts.headers.get("x-test-header").unwrap(), "original value");
    let body_bytes = body.collect().await;
    assert!(body_bytes.is_ok());
    if let Ok(bytes) = body_bytes {
        assert_eq!(bytes.to_bytes(), "streaming body data".as_bytes());
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_buffered_too_large() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));

    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let very_large_body = "a".repeat(10 * 1024 * 1024); // 10 MB body
    let boxed_str: Box<str> = very_large_body.clone().into_boxed_str();

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec![Box::leak(boxed_str)],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::DirectResponse(_));
    match result {
        FilterDecision::DirectResponse(direct_response) => {
            assert_eq!(direct_response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        },
        _ => assert!(false, "Unexpected filter decision"),
    }
}

#[tokio::test]
async fn test_response_multichunk_merged_body_buffered_mode() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("body data from external processor".as_bytes().into()),
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
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["streaming", "body data"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());
}

#[tokio::test]
async fn test_response_multichunk_not_merged_body_streaming_mode() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![Some(("x-stream-processed", "true")), Some(("y-custom-header", "true"))],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("body data".as_bytes().into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("external processor".as_bytes().into()),
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
        response_body_mode: BodyProcessingMode::Streamed,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["streaming", "body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;
    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    let body_chunks = to_body_data_chunks(response.body_mut().collect().await.unwrap()).await;
    assert_eq!(body_chunks, ["body data", "external processor"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>());
}

#[tokio::test]
#[test_log::test]
async fn test_response_multichunk_body_with_mutation() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("CHUNK1".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("CHUNK2".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("CHUNK3".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));

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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut response =
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["chunk1", "chunk2", "chunk3"], vec![])).await;
    let result = ext_proc.apply_response(&mut response).await;
    assert_matches!(result, FilterDecision::Continue);

    let body_chunks = to_body_data_chunks(response.body_mut().collect().await.unwrap()).await;

    assert_eq!(body_chunks, ["CHUNK1", "CHUNK2", "CHUNK3"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>());
}

#[tokio::test]
#[test_log::test]
async fn test_response_multichunk_body_timeout_failure_mode_allow_true() {
    let mock_state = MockExternalProcessorState::new().add_response(create_body_response::<ResponseMsg>(
        vec![],
        Some("modified_chunk".into()),
        vec![],
        ResponseStatus::Continue as i32,
        None,
    ));
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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut response =
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["chunk1", "chunk2", "chunk3"], vec![])).await;
    let result = ext_proc.apply_response(&mut response).await;
    assert_matches!(result, FilterDecision::Continue);

    let body_chunks = to_body_data_chunks(response.body_mut().collect().await.unwrap()).await;
    assert_eq!(
        body_chunks,
        ["modified_chunk", "chunk2", "chunk3"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>()
    );
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_and_trailer_out_of_order() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));

    let (server_addr, _) = start_mock_server_with_delay(mock_state, Duration::from_millis(500)).await;
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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["chunk1", "chunk2", "chunk3"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;
    assert_matches!(result, FilterDecision::Continue);

    let (_, body) = response.into_parts();

    let expected_trailers = http::HeaderMap::from_iter(vec![(
        http::HeaderName::from_static("x-custom-trailer"),
        http::HeaderValue::from_static("original-value"),
    )]);
    let expected_data = &["chunk1".into(), "chunk2".into(), "chunk3".into()];

    assert_matches!(assert_body_frames(body, expected_data, Some(expected_trailers)).await, Ok(()));
}

#[tokio::test]
#[test_log::test]
async fn test_response_multichunk_body_no_truncate_body() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("CHUNK1".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("CHUNK2".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(
            vec![],
            Some("CHUNK3".into()),
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ));

    let (server_addr, _) = start_mock_server_with_delay(mock_state, Duration::from_millis(500)).await;
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

    let mut ext_proc = ExternalProcessor::from((config, None));

    let mut response =
        build_response_from_mock(&Mock::<ResponseMsg>::new(vec![], vec!["chunk1", "chunk2", "chunk3"], vec![])).await;
    let result = ext_proc.apply_response(&mut response).await;
    assert_matches!(result, FilterDecision::Continue);

    let body_chunks = to_body_data_chunks(response.body_mut().collect().await.unwrap()).await;

    assert_eq!(body_chunks, ["CHUNK1", "CHUNK2", "CHUNK3"].into_iter().map(|s| s.into()).collect::<Vec<Bytes>>());
}

#[tokio::test]
#[test_log::test]
async fn test_request_trailer_timeout() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<RequestMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));
    // No trailer response provided, will timeout
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
        body: vec!["body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;
    // Trailer processing happens in the body stream, so the initial result is Continue
    // but the body collection will fail due to the trailer timeout
    assert_matches!(result, FilterDecision::Continue);

    let body_result = std::mem::take(&mut request.body_mut().inner).collect().await;
    assert!(body_result.is_err());
}

#[tokio::test]
#[test_log::test]
async fn test_request_trailer_timeout_failure_mode_allow_true() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<RequestMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<RequestMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));
    // No trailer response provided, will timeout but failure_mode_allow is true
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
    config.failure_mode_allow = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;
    assert_matches!(result, FilterDecision::Continue);

    let body = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap();
    let trailers = body.trailers().cloned();
    assert!(trailers.is_some());
    let trailers = trailers.unwrap();
    assert_eq!(trailers.get("x-custom-trailer").unwrap(), "original-value");
}

#[tokio::test]
#[test_log::test]
async fn test_response_trailer_timeout() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));
    // No trailer response provided, will timeout
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Send,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;
    // Trailer processing happens in the body stream, so the initial result is Continue
    // but the body collection will fail due to the trailer timeout
    assert_matches!(result, FilterDecision::Continue);

    let (_, body) = response.into_parts();
    let body_result = body.collect().await;
    assert!(body_result.is_err());
}

#[tokio::test]
#[test_log::test]
async fn test_response_trailer_timeout_failure_mode_allow_true() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response::<ResponseMsg>(vec![], None, vec![], ResponseStatus::Continue as i32, None));
    // No trailer response provided, will timeout but failure_mode_allow is true
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Send,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;
    assert_matches!(result, FilterDecision::Continue);

    let (_, body) = response.into_parts();
    let collected = body.collect().await.unwrap();
    let trailers = collected.trailers().cloned();
    assert!(trailers.is_some());
    let trailers = trailers.unwrap();
    assert_eq!(trailers.get("x-custom-trailer").unwrap(), "original-value");
}

fn create_clear_body_mutation() -> BodyMutation {
    BodyMutation { mutation: Some(Mutation::ClearBody(true)) }
}

fn create_body_response_with_clear_body<M: MsgKind>(
    headers: Vec<Option<(&str, &str)>>,
    status: i32,
) -> ProcessingResponse {
    let header_mutation = transform(headers).and_then(|hdrs| create_header_mutation(hdrs));

    let body_response = BodyResponse {
        response: Some(CommonResponse {
            status,
            header_mutation,
            body_mutation: Some(create_clear_body_mutation()),
            trailers: None,
            clear_route_cache: false,
        }),
    };

    let response = if M::IS_RESPONSE {
        Some(ProcessingResponseType::ResponseBody(body_response))
    } else {
        Some(ProcessingResponseType::RequestBody(body_response))
    };

    ProcessingResponse { response, ..Default::default() }
}

#[tokio::test]
#[test_log::test]
async fn test_request_body_clear_body_mutation() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<RequestMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response_with_clear_body::<RequestMsg>(vec![], ResponseStatus::Continue as i32));
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
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original body content that should be cleared"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, Bytes::new());
}

#[tokio::test]
#[test_log::test]
async fn test_response_body_clear_body_mutation() {
    let mock_state = MockExternalProcessorState::new()
        .add_response(create_headers_response::<ResponseMsg>(
            vec![],
            None,
            vec![],
            ResponseStatus::Continue as i32,
            None,
        ))
        .add_response(create_body_response_with_clear_body::<ResponseMsg>(vec![], ResponseStatus::Continue as i32));
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original body content that should be cleared"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, Bytes::new());
}

fn create_headers_response_with_clear_body<M: MsgKind>(
    headers: Vec<Option<(&str, &str)>>,
    status: i32,
) -> ProcessingResponse {
    let header_mutation = transform(headers).and_then(|hdrs| create_header_mutation(hdrs));

    let header_response = HeadersResponse {
        response: Some(CommonResponse {
            status,
            header_mutation,
            body_mutation: Some(create_clear_body_mutation()),
            trailers: None,
            clear_route_cache: false,
        }),
    };

    let response = if M::IS_RESPONSE {
        Some(ProcessingResponseType::ResponseHeaders(header_response))
    } else {
        Some(ProcessingResponseType::RequestHeaders(header_response))
    };

    ProcessingResponse { response, ..Default::default() }
}

#[tokio::test]
#[test_log::test]
async fn test_request_header_clear_body_mutation_continue_and_replace() {
    let mock_state =
        MockExternalProcessorState::new().add_response(create_headers_response_with_clear_body::<RequestMsg>(
            vec![Some(("x-body-cleared", "true"))],
            ResponseStatus::ContinueAndReplace as i32,
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
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original body content that should be cleared"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(request.headers().get("x-body-cleared").unwrap(), "true");
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, Bytes::new());
}

#[tokio::test]
#[test_log::test]
async fn test_response_header_clear_body_mutation_continue_and_replace() {
    let mock_state =
        MockExternalProcessorState::new().add_response(create_headers_response_with_clear_body::<ResponseMsg>(
            vec![Some(("x-body-cleared", "true"))],
            ResponseStatus::ContinueAndReplace as i32,
        ));
    let (server_addr, _) = start_mock_server(mock_state).await;
    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![Some(("content-type", "application/json"))],
        body: vec!["original body content that should be cleared"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-body-cleared").unwrap(), "true");
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, Bytes::new());
}

#[tokio::test]
#[test_log::test]
async fn test_response_full_duplex_streaming_mode_header_only() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::FullDuplexStreamed,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_full_duplex_streaming_mode_with_body() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::FullDuplexStreamed,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_full_duplex_streaming_mode_with_body_and_trailers() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .add_response(create_trailers_response::<ResponseMsg>(vec![]))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::FullDuplexStreamed,
        response_trailer_mode: TrailerProcessingMode::Send,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(false), "Expected end of stream false");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_buffered_mode_header_only() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_buffered_mode_with_body() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_buffered_mode_with_body_and_trailers() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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
        ))
        .add_response(create_trailers_response::<ResponseMsg>(vec![]))
        .with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::Buffered,
        response_trailer_mode: TrailerProcessingMode::Send,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = false;
    config.failure_mode_allow = false;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    assert_eq!(response.headers().get("x-stream-processed").unwrap(), "true");

    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "body data from external processor".as_bytes());

    mock_state.token.cancel();

    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(false), "Expected end of stream false");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_full_duplex_streaming_mode_header_only_observability() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mock_state = MockExternalProcessorState::new().with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Send,
        request_body_mode: BodyProcessingMode::FullDuplexStreamed,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Skip,
        response_body_mode: BodyProcessingMode::None,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![],
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    // in observability mode we need to sleep for a bit before sending the
    // cancellation token to allow the mock external processor to process all
    // chunks
    tokio::time::sleep(Duration::from_millis(500)).await;
    mock_state.token.cancel();
    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_full_duplex_streaming_mode_with_body_observability() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mock_state = MockExternalProcessorState::new().with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Send,
        request_body_mode: BodyProcessingMode::FullDuplexStreamed,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Skip,
        response_body_mode: BodyProcessingMode::None,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "this is the body".as_bytes());
    // in observability mode we need to sleep for a bit before sending the
    // cancellation token to allow the mock external processor to process all
    // chunks
    tokio::time::sleep(Duration::from_millis(500)).await;
    mock_state.token.cancel();
    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_request_full_duplex_streaming_mode_with_body_and_trailers_observability() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mock_state = MockExternalProcessorState::new().with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Send,
        request_body_mode: BodyProcessingMode::FullDuplexStreamed,
        request_trailer_mode: TrailerProcessingMode::Send,
        response_header_mode: HeaderProcessingMode::Skip,
        response_body_mode: BodyProcessingMode::None,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut request = build_request_from_mock(&Mock::<RequestMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_request(&mut request).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = std::mem::take(&mut request.body_mut().inner).collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "this is the body".as_bytes());
    // in observability mode we need to sleep for a bit before sending the
    // cancellation token to allow the mock external processor to process all
    // chunks
    tokio::time::sleep(Duration::from_millis(500)).await;
    mock_state.token.cancel();
    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(false), "Expected end of stream false");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_full_duplex_streaming_mode_header_only_observability() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mock_state = MockExternalProcessorState::new().with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::FullDuplexStreamed,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec![],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    // in observability mode we need to sleep for a bit before sending the
    // cancellation token to allow the mock external processor to process all
    // chunks
    tokio::time::sleep(Duration::from_millis(500)).await;
    mock_state.token.cancel();
    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_full_duplex_streaming_mode_with_body_observability() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mock_state = MockExternalProcessorState::new().with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::FullDuplexStreamed,
        response_trailer_mode: TrailerProcessingMode::Skip,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;

    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "this is the body".as_bytes());
    // in observability mode we need to sleep for a bit before sending the
    // cancellation token to allow the mock external processor to process all
    // chunks
    tokio::time::sleep(Duration::from_millis(500)).await;
    mock_state.token.cancel();
    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(true), "Expected end of stream true");
    }
}

#[tokio::test]
#[test_log::test]
async fn test_response_full_duplex_streaming_mode_with_body_and_trailers_observability() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mock_state = MockExternalProcessorState::new().with_sender(tx);

    let (server_addr, _) = start_mock_server(mock_state.clone()).await;

    let processing_mode = ProcessingMode {
        request_header_mode: HeaderProcessingMode::Skip,
        request_body_mode: BodyProcessingMode::None,
        request_trailer_mode: TrailerProcessingMode::Skip,
        response_header_mode: HeaderProcessingMode::Send,
        response_body_mode: BodyProcessingMode::FullDuplexStreamed,
        response_trailer_mode: TrailerProcessingMode::Send,
    };

    let mut config = create_default_config_for_ext_proc_filter(server_addr, processing_mode);
    config.observability_mode = true;
    let mut ext_proc = ExternalProcessor::from(config);

    let mut response = build_response_from_mock(&Mock::<ResponseMsg> {
        headers: vec![],
        body: vec!["this is the body"],
        trailers: vec![Some(("x-custom-trailer", "original-value"))],
        _marker: std::marker::PhantomData,
    })
    .await;

    let result = ext_proc.apply_response(&mut response).await;
    // in observability mode we need to sleep for a bit before sending the
    // cancellation token to allow the mock external processor to process all
    // chunks
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_matches!(result, FilterDecision::Continue);
    let body_bytes = response.body_mut().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, "this is the body".as_bytes());

    mock_state.token.cancel();
    if let Some(state) = rx.recv().await {
        assert_matches!(state.last_end_of_stream, Some(false), "Expected end of stream false");
    }
}
