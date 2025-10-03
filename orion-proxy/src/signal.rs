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

use tracing::{info, warn};

/// Signal types that can trigger shutdown
#[derive(Debug, Clone, Copy)]
pub enum ShutdownSignal {
    /// CTRL+C (SIGINT) signal
    Interrupt,
    /// SIGTERM signal (Unix only)
    #[cfg(unix)]
    Terminate,
}

impl std::fmt::Display for ShutdownSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShutdownSignal::Interrupt => write!(f, "SIGINT (CTRL+C)"),
            #[cfg(unix)]
            ShutdownSignal::Terminate => write!(f, "SIGTERM"),
        }
    }
}

/// `wait_signal` listens for shutdown signals
///
/// On Unix platforms, this listens for both SIGINT (CTRL+C) and SIGTERM.
/// On Windows, this only listens for CTRL+C.
pub async fn wait_signal() {
    if let Err(e) = listen_for_signals().await {
        warn!("Signal handler error: {}", e);
    }
}

/// Unix-specific signal handling (SIGINT and SIGTERM)
#[cfg(unix)]
async fn listen_for_signals() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = sigint.recv() => {
            info!("Received {} signal, initiating shutdown...", ShutdownSignal::Interrupt);
        }
        _ = sigterm.recv() => {
            info!("Received {} signal, initiating shutdown...", ShutdownSignal::Terminate);
        }
    }

    Ok(())
}

/// Windows-specific signal handling (CTRL+C only)
#[cfg(not(unix))]
async fn listen_for_signals() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Err(e) = tokio::signal::ctrl_c().await {
        return Err(format!("Failed to listen for CTRL+C: {}", e).into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_timeout::fast_timeout::fast_timeout;
    use std::time::Duration;

    // These tests send real POSIX signals; mark ignored so they are only run explicitly:
    // cargo test --package orion-proxy --lib -- --ignored test_wait_signal_sigint
    #[cfg(unix)]
    #[ignore]
    #[tokio::test]
    async fn test_wait_signal_sigint() {
        let handle = tokio::spawn(async {
            wait_signal().await;
        });
        // Give the signal handler a brief moment to install
        tokio::time::sleep(Duration::from_millis(20)).await;
        unsafe {
            libc::kill(libc::getpid(), libc::SIGINT);
        }
        fast_timeout(Duration::from_secs(1), handle)
            .await
            .expect("timeout waiting for wait_signal to return")
            .expect("wait_signal task panicked");
    }

    #[cfg(unix)]
    #[ignore]
    #[tokio::test]
    async fn test_wait_signal_sigterm() {
        let handle = tokio::spawn(async {
            wait_signal().await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }
        fast_timeout(Duration::from_secs(1), handle)
            .await
            .expect("timeout waiting for wait_signal to return")
            .expect("wait_signal task panicked");
    }

    #[test]
    fn test_display_variants() {
        assert_eq!(format!("{}", ShutdownSignal::Interrupt), "SIGINT (CTRL+C)");
        #[cfg(unix)]
        assert_eq!(format!("{}", ShutdownSignal::Terminate), "SIGTERM");
    }
}
