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

#![allow(unused_macros)]

mod deferred_init;
mod log_writer;
pub mod logger;
mod pool;

use compact_str::CompactString;
use logger::AccessLogger;
use once_cell::sync::OnceCell;
use orion_configuration::config::network_filters::access_log::AccessLogConf;
use orion_format::FormattedMessage;
use parking_lot::Mutex;
use pool::LoggerPool;
use tracing_appender::rolling::Rotation;

use std::{fmt::Display, hash::Hash, sync::Arc};
use tokio::{
    sync::mpsc::{Permit, Sender},
    task::JoinSet,
};
use tracing::{error, info};

/// Represents the destination for an access logging event.
///
/// - `Listener`: Identifies a specific listener by name.
/// - `Admin`: Refers to the Envoy admin interface.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Listener(CompactString),
    Admin,
}

impl Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Listener(name) => write!(f, "Listener({name})"),
            Target::Admin => write!(f, "Admin"),
        }
    }
}

/// Represents messages sent to access loggers.
///
/// - `Configure`: Updates the logger configuration for a given target.
/// - `Message`: Sends one or more formatted log entries to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessLogMessage {
    Configure(Target, Vec<AccessLogConf>),
    Message(Target, Vec<FormattedMessage>),
}

#[derive(Debug, thiserror::Error)]
pub enum LoggerError {
    #[error("Failed to initialize logger: {0}")]
    InitializationError(String),
    #[error("Channel closed unexpectedly")]
    SenderError,
}

static SENDER_POOL: OnceCell<LoggerPool<AccessLogMessage>> = OnceCell::new();

type AccessLogToken = Permit<'static, AccessLogMessage>;

/// Asynchronously reserves a permit to send an `AccessLogMessage`, if possible.
///
/// Returns an `Arc<Mutex<Option<AccessLogToken>>>`, which contains `Some(permit)` if the sender is available
/// and the reservation succeeds, or `None` otherwise. The permit can be shared across tasks and
/// consumed later by one of them to log message.
///
#[inline]
pub async fn log_access_reserve_balanced() -> Arc<Mutex<Option<AccessLogToken>>> {
    let maybe_permit = if let Some(sender) = get_sender() { sender.reserve().await.ok() } else { None };
    Arc::new(Mutex::new(maybe_permit))
}

/// Asynchronously reserves a permit from the first access log sender, if available.
///
/// Returns an `Arc<Mutex<Option<AccessLogToken>>>` containing `Some(permit)` if the reservation succeeds,
/// or `None` if the sender is unavailable or the reservation fails. The permit can be shared across tasks
/// and consumed later by one of them to log a message.
#[inline]
pub async fn log_access_reserve_single() -> Arc<Mutex<Option<AccessLogToken>>> {
    let maybe_permit = if let Some(sender) = get_sender_at(0) { sender.reserve().await.ok() } else { None };
    Arc::new(Mutex::new(maybe_permit))
}

/// Sends an `AccessLogMessage` using a previously reserved permit, if available.
///
/// Consumes the permit inside the given `Arc<Mutex<Option<AccessLogToken>>>` and sends a message
/// containing the specified `target` and formatted messages.
#[allow(clippy::needless_pass_by_value)]
#[inline]
pub fn log_access(permit: Arc<Mutex<Option<AccessLogToken>>>, target: Target, vec: Vec<FormattedMessage>) {
    if let Some(permit) = permit.lock().take() {
        permit.send(AccessLogMessage::Message(target, vec));
    }
}

/// Initializes and starts a set of asynchronous access loggers.
///
/// This function creates `len` independent logging tasks, each with its own
/// bounded MPSC channel of capacity `buffer`. These loggers receive messages
/// through the globally shared `SENDER_POOL`, which is initialized here using
/// an unsafe swap.
///
/// Each logger runs concurrently in the background using a Tokio `JoinSet`,
/// and processes incoming messages asynchronously.
///
/// # Arguments
///
/// * `len` - The number of logger instances to spawn.
/// * `buffer` - The size of the message buffer for each logger's channel.
/// * `rotation` - The Rotation variant from tracing lib.
/// * `max_log_files` - The maximum number of files per each target.
///
/// # Returns
///
/// A `JoinSet<()>` containing all spawned logger tasks, which can be awaited
#[allow(clippy::needless_pass_by_value)]
pub fn start_access_loggers(
    num_instances: usize,
    buffer: usize,
    rotation: Rotation,
    max_log_files: usize,
) -> JoinSet<()> {
    let (mut senders, mut receivers) = (Vec::with_capacity(num_instances), Vec::with_capacity(num_instances));
    for _ in 0..num_instances {
        let (sender, receiver) = tokio::sync::mpsc::channel(buffer);
        senders.push(sender);
        receivers.push(receiver);
    }

    info!("Initializing access loggers...");

    if SENDER_POOL.set(LoggerPool(senders)).is_err() {
        error!("Unable to initialize logger pool!");
        return JoinSet::new(); // Return an empty JoinSet on error
    }

    let mut join_set = JoinSet::new();
    for (i, recv) in receivers.into_iter().enumerate() {
        let rotation = rotation.clone();
        join_set.spawn(async move {
            let mut logger = AccessLogger::new(i, rotation, max_log_files);
            logger.run(recv).await
        });
    }
    join_set
}

/// Returns true if the logger has been initialized with active senders.
///
/// Checks if the global sender pool contains any senders.
#[inline]
pub fn is_access_log_enabled() -> bool {
    SENDER_POOL.get().is_some_and(|pool| !pool.0.is_empty())
}

/// Returns a reference to an available logger sender, if any.
///
/// Accesses the global sender pool unsafely and retrieves one sender.
/// Returns `None` if no senders are available (if initialized with `init_logger_once`)
#[inline]
pub fn get_sender() -> Option<&'static Sender<AccessLogMessage>> {
    SENDER_POOL.get().and_then(|pool| pool.get())
}

#[inline]
pub fn get_sender_at(index: usize) -> Option<&'static Sender<AccessLogMessage>> {
    SENDER_POOL.get().and_then(|pool| pool.get_at(index))
}

/// Updates logger configuration for the given target.
///
/// Sends an `AccessLogMessage::Configure` with the new configuration to all active loggers.
/// Returns `Ok(())` if all sends succeed, otherwise returns a vector of errors.
///
/// Logs info if there are active loggers, and errors for each failed send.
pub async fn update_configuration(target: Target, init: Vec<AccessLogConf>) -> Result<(), LoggerError> {
    let pool =
        SENDER_POOL.get().ok_or_else(|| LoggerError::InitializationError("Logger pool not initialized".into()))?;
    for (i, senders) in pool.0.iter().enumerate() {
        if let Err(e) = senders.send(AccessLogMessage::Configure(target.clone(), init.clone())).await {
            error!("Failed to send logger configuration to sender {i}: {e}");
            return Err(LoggerError::SenderError);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::access_log::start_access_loggers;

    use super::*;
    use orion_format::{
        context::{DownstreamContext, DownstreamResponse, FinishContext, InitContext, UpstreamContext},
        types::ResponseFlags,
        LogFormatter, DEFAULT_ACCESS_LOG_FORMAT,
    };
    use tokio::{self, time::timeout};

    fn build_request() -> http::Request<()> {
        http::Request::builder().uri("https://www.rust-lang.org/").header("User-Agent", "awesome/1.0").body(()).unwrap()
    }

    fn build_response() -> http::Response<()> {
        let builder = http::Response::builder().status(http::StatusCode::OK);
        builder.body(()).unwrap()
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn test_access_logger() {
        let req = build_request();
        let resp = build_response();

        let formatter = LogFormatter::try_new(DEFAULT_ACCESS_LOG_FORMAT, false).unwrap();
        let mut fmt = formatter.local_clone();

        fmt.with_context(&InitContext { start_time: std::time::SystemTime::now() });
        fmt.with_context(&DownstreamContext { request: &req, trace_id: None, request_head_size: 0, server_name: None });
        fmt.with_context(&UpstreamContext {
            authority: Some(req.uri().authority().unwrap()),
            cluster_name: Some("test_cluster"),
            route_name: "test_route",
        });
        fmt.with_context(&DownstreamResponse { response: &resp, response_head_size: 0 });
        fmt.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM,
            upstream_failure: None,
            response_code_details: None,
            connection_termination_details: None,
        });

        let message = fmt.into_message();

        // initialize the logger pool with one channel for access log messages
        let handles = start_access_loggers(8, 100, Rotation::NEVER, 3);

        // send a new configuration for the logger(s)
        update_configuration(
            Target::Listener("test".into()),
            vec![AccessLogConf::File("test-access.log".into()), AccessLogConf::Stderr],
        )
        .await
        .unwrap();

        let permit = log_access_reserve_single().await;
        // log the formatted message to file and stdout...
        log_access(permit, Target::Listener("test".into()), vec![message.clone(), message.clone()]);

        _ = timeout(Duration::from_secs(2), handles.join_all()).await;
    }
}
