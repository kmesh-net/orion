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

use std::thread::{self, JoinHandle};
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Signal types that can trigger shutdown
#[derive(Debug, Clone, Copy)]
pub enum ShutdownSignal {
    /// CTRL+C (SIGINT) signal
    Interrupt,
    /// SIGTERM signal (Unix only)
    #[cfg(unix)]
    Terminate,
    /// Manual shutdown request
    Manual,
}

impl std::fmt::Display for ShutdownSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShutdownSignal::Interrupt => write!(f, "SIGINT (CTRL+C)"),
            #[cfg(unix)]
            ShutdownSignal::Terminate => write!(f, "SIGTERM"),
            ShutdownSignal::Manual => write!(f, "Manual"),
        }
    }
}

/// Spawns a signal handler thread that listens for shutdown signals and notifies
/// all subscribers via a broadcast channel.
///
/// On Unix platforms, this listens for both SIGINT (CTRL+C) and SIGTERM.
/// On Windows, this only listens for CTRL+C.
///
/// Returns a tuple of:
/// - `broadcast::Sender<ShutdownSignal>`: Used to create receivers for shutdown notifications
/// - `JoinHandle<()>`: Handle to the signal handler thread
pub fn spawn_signal_handler() -> (broadcast::Sender<ShutdownSignal>, JoinHandle<()>) {
    let (shutdown_tx, _) = broadcast::channel::<ShutdownSignal>(16);
    let signal_shutdown_tx = shutdown_tx.clone();

    let signal_handle = thread::Builder::new()
        .name("signal_handler".to_owned())
        .spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    warn!("Failed to create signal handler runtime: {}", e);
                    return;
                },
            };

            rt.block_on(async {
                if let Err(e) = listen_for_signals(signal_shutdown_tx).await {
                    warn!("Signal handler error: {}", e);
                }
            });
        })
        .expect("Failed to spawn signal handler thread");

    (shutdown_tx, signal_handle)
}

/// Cross-platform signal listening implementation
async fn listen_for_signals(
    shutdown_tx: broadcast::Sender<ShutdownSignal>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(unix)]
    {
        listen_for_signals_unix(shutdown_tx).await
    }
    #[cfg(not(unix))]
    {
        listen_for_signals_windows(shutdown_tx).await
    }
}

/// Unix-specific signal handling (SIGINT and SIGTERM)
#[cfg(unix)]
async fn listen_for_signals_unix(
    shutdown_tx: broadcast::Sender<ShutdownSignal>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = sigint.recv() => {
            info!("Received {} signal, initiating shutdown...", ShutdownSignal::Interrupt);
            if let Err(e) = shutdown_tx.send(ShutdownSignal::Interrupt) {
                warn!("Failed to send shutdown signal: {}", e);
            }
        }
        _ = sigterm.recv() => {
            info!("Received {} signal, initiating shutdown...", ShutdownSignal::Terminate);
            if let Err(e) = shutdown_tx.send(ShutdownSignal::Terminate) {
                warn!("Failed to send shutdown signal: {}", e);
            }
        }
    }

    Ok(())
}

/// Windows-specific signal handling (CTRL+C only)
#[cfg(not(unix))]
async fn listen_for_signals_windows(
    shutdown_tx: broadcast::Sender<ShutdownSignal>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Err(e) = tokio::signal::ctrl_c().await {
        return Err(format!("Failed to listen for CTRL+C: {}", e).into());
    }

    info!("Received {} signal, initiating shutdown...", ShutdownSignal::Interrupt);
    if let Err(e) = shutdown_tx.send(ShutdownSignal::Interrupt) {
        warn!("Failed to send shutdown signal: {}", e);
    }

    Ok(())
}

/// Creates a shutdown receiver from the broadcast sender
pub fn create_shutdown_receiver(
    shutdown_tx: &broadcast::Sender<ShutdownSignal>,
) -> broadcast::Receiver<ShutdownSignal> {
    shutdown_tx.subscribe()
}

/// Utility function to manually trigger shutdown (useful for testing or graceful shutdown)
pub fn trigger_manual_shutdown(
    shutdown_tx: &broadcast::Sender<ShutdownSignal>,
) -> Result<(), broadcast::error::SendError<ShutdownSignal>> {
    info!("Triggering manual shutdown...");
    shutdown_tx.send(ShutdownSignal::Manual).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_manual_shutdown() {
        let (shutdown_tx, _handle) = spawn_signal_handler();
        let mut shutdown_rx = create_shutdown_receiver(&shutdown_tx);

        // Trigger manual shutdown
        trigger_manual_shutdown(&shutdown_tx).expect("Failed to trigger manual shutdown");

        // Should receive the shutdown signal
        let signal = tokio::time::timeout(Duration::from_millis(100), shutdown_rx.recv())
            .await
            .expect("Timeout waiting for shutdown signal")
            .expect("Failed to receive shutdown signal");

        matches!(signal, ShutdownSignal::Manual);
    }

    #[test]
    fn test_signal_display() {
        assert_eq!(format!("{}", ShutdownSignal::Interrupt), "SIGINT (CTRL+C)");
        assert_eq!(format!("{}", ShutdownSignal::Manual), "Manual");

        #[cfg(unix)]
        assert_eq!(format!("{}", ShutdownSignal::Terminate), "SIGTERM");
    }
}
