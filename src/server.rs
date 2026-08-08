use std::{
    env,
    future::pending,
    io::{self, IsTerminal, Read},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use tokio::{
    signal,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time,
};

use crate::{config, nats, routes, telemetry, youtube};

const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 10_000;

/// Initialize configuration, observability, fail-soft dependencies, and HTTP.
pub(crate) async fn run() -> anyhow::Result<()> {
    let cfg = config::Config::from_env()?;
    telemetry::init(&cfg.service_name)?;
    let _telemetry = TelemetryGuard;

    serve(cfg).await?;
    tracing::info!("shutdown complete");
    Ok(())
}

async fn serve(cfg: config::Config) -> anyhow::Result<()> {
    let nats = nats::connect(&cfg.nats_url).await;
    if let Some(client) = nats.clone() {
        nats::spawn_event_subscriber(client);
    }

    let youtube = cfg
        .youtube
        .as_ref()
        .map(youtube::YoutubeGasClient::new)
        .transpose()?;

    tracing::info!(
        youtube_configured = youtube.is_some(),
        admin_auth_configured = cfg.admin_api_key.is_some(),
        "control-plane configuration loaded"
    );

    let app = routes::router(routes::AppState {
        nats,
        youtube,
        admin_api_key: cfg.admin_api_key.map(Arc::<str>::from),
    });

    let address = bind_address(cfg.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, service = %cfg.service_name, "act-api-server listening");

    let (graceful_tx, graceful_rx) = oneshot::channel();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = graceful_rx.await;
    });
    let server_handle = tokio::spawn(async move { server.await });

    supervise_server(server_handle, graceful_tx, shutdown_config()).await
}

fn bind_address(port: u16) -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], port))
}

/// Flush buffered OTLP spans on every return path after telemetry initializes.
struct TelemetryGuard;

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        telemetry::shutdown();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownPhase {
    Running,
    Draining,
    Forcing,
    Complete,
}

impl ShutdownPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Draining => "draining",
            Self::Forcing => "forcing",
            Self::Complete => "complete",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownTrigger {
    Sigint,
    Sigterm,
    StdinEof,
    Timeout,
    GracefulComplete,
}

impl ShutdownTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Sigint => "sigint",
            Self::Sigterm => "sigterm",
            Self::StdinEof => "stdin_eof",
            Self::Timeout => "timeout",
            Self::GracefulComplete => "graceful_complete",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownAction {
    Ignore,
    BeginGraceful,
    Force,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShutdownState {
    phase: ShutdownPhase,
    stdin_is_tty: bool,
    first_trigger: Option<ShutdownTrigger>,
}

impl ShutdownState {
    const fn new(stdin_is_tty: bool) -> Self {
        Self {
            phase: ShutdownPhase::Running,
            stdin_is_tty,
            first_trigger: None,
        }
    }

    fn apply(self, trigger: ShutdownTrigger) -> ShutdownTransition {
        match self.phase {
            ShutdownPhase::Running => match trigger {
                ShutdownTrigger::Sigint | ShutdownTrigger::Sigterm => ShutdownTransition {
                    state: Self {
                        phase: ShutdownPhase::Draining,
                        first_trigger: Some(trigger),
                        ..self
                    },
                    action: ShutdownAction::BeginGraceful,
                    show_force_hint: self.stdin_is_tty
                        && matches!(trigger, ShutdownTrigger::Sigint),
                },
                _ => ShutdownTransition::ignored(self),
            },
            ShutdownPhase::Draining => match trigger {
                ShutdownTrigger::GracefulComplete => ShutdownTransition {
                    state: Self {
                        phase: ShutdownPhase::Complete,
                        ..self
                    },
                    action: ShutdownAction::Complete,
                    show_force_hint: false,
                },
                ShutdownTrigger::Sigint
                | ShutdownTrigger::Sigterm
                | ShutdownTrigger::Timeout => ShutdownTransition {
                    state: Self {
                        phase: ShutdownPhase::Forcing,
                        ..self
                    },
                    action: ShutdownAction::Force,
                    show_force_hint: false,
                },
                ShutdownTrigger::StdinEof
                    if self.stdin_is_tty
                        && matches!(self.first_trigger, Some(ShutdownTrigger::Sigint)) =>
                {
                    ShutdownTransition {
                        state: Self {
                            phase: ShutdownPhase::Forcing,
                            ..self
                        },
                        action: ShutdownAction::Force,
                        show_force_hint: false,
                    }
                }
                _ => ShutdownTransition::ignored(self),
            },
            ShutdownPhase::Forcing | ShutdownPhase::Complete => {
                ShutdownTransition::ignored(self)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShutdownTransition {
    state: ShutdownState,
    action: ShutdownAction,
    show_force_hint: bool,
}

impl ShutdownTransition {
    const fn ignored(state: ShutdownState) -> Self {
        Self {
            state,
            action: ShutdownAction::Ignore,
            show_force_hint: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ShutdownConfig {
    grace_period: Duration,
    stdin_is_tty: bool,
}

fn shutdown_config() -> ShutdownConfig {
    let grace_ms = match env::var("SHUTDOWN_GRACE_MS") {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(value) if value > 0 => value,
            _ => {
                tracing::warn!(
                    event = "shutdown_config_invalid",
                    value = %raw,
                    fallback_ms = DEFAULT_SHUTDOWN_GRACE_MS,
                    "SHUTDOWN_GRACE_MS must be a positive integer"
                );
                DEFAULT_SHUTDOWN_GRACE_MS
            }
        },
        Err(env::VarError::NotPresent) => DEFAULT_SHUTDOWN_GRACE_MS,
        Err(error) => {
            tracing::warn!(
                event = "shutdown_config_invalid",
                %error,
                fallback_ms = DEFAULT_SHUTDOWN_GRACE_MS,
                "SHUTDOWN_GRACE_MS is not valid Unicode"
            );
            DEFAULT_SHUTDOWN_GRACE_MS
        }
    };

    ShutdownConfig {
        grace_period: Duration::from_millis(grace_ms),
        stdin_is_tty: io::stdin().is_terminal(),
    }
}

async fn supervise_server(
    mut server_handle: JoinHandle<io::Result<()>>,
    graceful_tx: oneshot::Sender<()>,
    config: ShutdownConfig,
) -> anyhow::Result<()> {
    let mut signals = ShutdownSignals::new()?;

    let first_trigger = tokio::select! {
        result = &mut server_handle => {
            flatten_server_result(result)?;
            tracing::info!(
                event = "shutdown_complete",
                phase = ShutdownPhase::Complete.as_str(),
                trigger = "server_complete",
                stdin_is_tty = config.stdin_is_tty,
                grace_ms = grace_ms(config.grace_period),
                forced = false,
                "server completed without a shutdown signal"
            );
            return Ok(());
        }
        trigger = signals.recv() => trigger?,
    };

    let first = ShutdownState::new(config.stdin_is_tty).apply(first_trigger);
    debug_assert_eq!(first.action, ShutdownAction::BeginGraceful);
    let mut state = first.state;

    tracing::info!(
        event = "shutdown_requested",
        phase = state.phase.as_str(),
        trigger = first_trigger.as_str(),
        stdin_is_tty = state.stdin_is_tty,
        grace_ms = grace_ms(config.grace_period),
        forced = false,
        "graceful shutdown requested; listener is closing and active requests are draining"
    );

    if graceful_tx.send(()).is_err() {
        tracing::warn!(
            event = "shutdown_notification_late",
            phase = state.phase.as_str(),
            trigger = first_trigger.as_str(),
            stdin_is_tty = state.stdin_is_tty,
            grace_ms = grace_ms(config.grace_period),
            forced = false,
            "server completed before the graceful-shutdown notification was delivered"
        );
    }

    let mut eof_receiver = if first.show_force_hint {
        tracing::info!(
            event = "shutdown_force_available",
            phase = state.phase.as_str(),
            trigger = first_trigger.as_str(),
            stdin_is_tty = true,
            grace_ms = grace_ms(config.grace_period),
            forced = false,
            "press Ctrl-C again or Ctrl-D to force shutdown"
        );
        spawn_stdin_eof_watcher()
    } else {
        None
    };

    let deadline = time::sleep(config.grace_period);
    tokio::pin!(deadline);

    let force_trigger = tokio::select! {
        result = &mut server_handle => {
            flatten_server_result(result)?;
            state = state.apply(ShutdownTrigger::GracefulComplete).state;
            tracing::info!(
                event = "shutdown_complete",
                phase = state.phase.as_str(),
                trigger = first_trigger.as_str(),
                stdin_is_tty = state.stdin_is_tty,
                grace_ms = grace_ms(config.grace_period),
                forced = false,
                "graceful shutdown complete"
            );
            return Ok(());
        }
        trigger = signals.recv() => trigger?,
        _ = &mut deadline => ShutdownTrigger::Timeout,
        () = receive_eof(&mut eof_receiver) => ShutdownTrigger::StdinEof,
    };

    let forced = state.apply(force_trigger);
    anyhow::ensure!(
        forced.action == ShutdownAction::Force,
        "invalid shutdown transition from {:?} via {:?}",
        state.phase,
        force_trigger
    );
    state = forced.state;

    tracing::warn!(
        event = "shutdown_forced",
        phase = state.phase.as_str(),
        trigger = force_trigger.as_str(),
        stdin_is_tty = state.stdin_is_tty,
        grace_ms = grace_ms(config.grace_period),
        forced = true,
        "forceful shutdown requested; active connections will be dropped"
    );

    server_handle.abort();
    match server_handle.await {
        Err(error) if error.is_cancelled() => {}
        Err(error) => return Err(error.into()),
        Ok(Err(error)) => return Err(error.into()),
        Ok(Ok(())) => {}
    }

    tracing::info!(
        event = "shutdown_complete",
        phase = ShutdownPhase::Complete.as_str(),
        trigger = force_trigger.as_str(),
        stdin_is_tty = state.stdin_is_tty,
        grace_ms = grace_ms(config.grace_period),
        forced = true,
        "forceful shutdown complete"
    );
    Ok(())
}

fn grace_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn spawn_stdin_eof_watcher() -> Option<mpsc::UnboundedReceiver<()>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let thread = std::thread::Builder::new()
        .name("shutdown-stdin-eof".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            let mut stdin = stdin.lock();
            let mut byte = [0_u8; 1];

            loop {
                match stdin.read(&mut byte) {
                    Ok(0) => {
                        let _ = sender.send(());
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            event = "shutdown_stdin_watch_failed",
                            %error,
                            "failed while waiting for terminal EOF; a second Ctrl-C still forces shutdown"
                        );
                        return;
                    }
                }
            }
        });

    match thread {
        Ok(_) => Some(receiver),
        Err(error) => {
            tracing::warn!(
                event = "shutdown_stdin_watch_failed",
                %error,
                "could not start terminal EOF watcher; a second Ctrl-C still forces shutdown"
            );
            None
        }
    }
}

async fn receive_eof(receiver: &mut Option<mpsc::UnboundedReceiver<()>>) {
    match receiver {
        Some(receiver) => match receiver.recv().await {
            Some(()) => {}
            None => pending::<()>().await,
        },
        None => pending::<()>().await,
    }
}

fn flatten_server_result(
    result: Result<io::Result<()>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.into()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
struct ShutdownSignals {
    sigint: signal::unix::Signal,
    sigterm: signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        use signal::unix::{signal, SignalKind};

        Ok(Self {
            sigint: signal(SignalKind::interrupt())?,
            sigterm: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) -> io::Result<ShutdownTrigger> {
        tokio::select! {
            signal = self.sigint.recv() => signal
                .map(|_| ShutdownTrigger::Sigint)
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "SIGINT stream closed")),
            signal = self.sigterm.recv() => signal
                .map(|_| ShutdownTrigger::Sigterm)
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "SIGTERM stream closed")),
        }
    }
}

#[cfg(not(unix))]
struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) -> io::Result<ShutdownTrigger> {
        signal::ctrl_c().await?;
        Ok(ShutdownTrigger::Sigint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_address_uses_all_ipv4_interfaces_and_configured_port() {
        assert_eq!(bind_address(8080), "0.0.0.0:8080".parse().unwrap());
        assert_eq!(bind_address(9124), "0.0.0.0:9124".parse().unwrap());
    }

    #[test]
    fn tty_second_sigint_forces() {
        let first = ShutdownState::new(true).apply(ShutdownTrigger::Sigint);
        assert_eq!(first.action, ShutdownAction::BeginGraceful);
        assert!(first.show_force_hint);

        let second = first.state.apply(ShutdownTrigger::Sigint);
        assert_eq!(second.action, ShutdownAction::Force);
        assert_eq!(second.state.phase, ShutdownPhase::Forcing);
    }

    #[test]
    fn tty_eof_only_forces_after_sigint() {
        let before = ShutdownState::new(true).apply(ShutdownTrigger::StdinEof);
        assert_eq!(before.action, ShutdownAction::Ignore);

        let first = ShutdownState::new(true).apply(ShutdownTrigger::Sigint);
        let eof = first.state.apply(ShutdownTrigger::StdinEof);
        assert_eq!(eof.action, ShutdownAction::Force);
    }

    #[test]
    fn non_tty_one_sigint_begins_graceful_without_force_hint() {
        let first = ShutdownState::new(false).apply(ShutdownTrigger::Sigint);
        assert_eq!(first.action, ShutdownAction::BeginGraceful);
        assert!(!first.show_force_hint);
    }

    #[test]
    fn sigterm_always_begins_graceful_without_force_hint() {
        for stdin_is_tty in [false, true] {
            let first = ShutdownState::new(stdin_is_tty).apply(ShutdownTrigger::Sigterm);
            assert_eq!(first.action, ShutdownAction::BeginGraceful);
            assert!(!first.show_force_hint);
        }
    }

    #[test]
    fn graceful_completion_reaches_complete_phase() {
        let first = ShutdownState::new(false).apply(ShutdownTrigger::Sigterm);
        let complete = first.state.apply(ShutdownTrigger::GracefulComplete);
        assert_eq!(complete.action, ShutdownAction::Complete);
        assert_eq!(complete.state.phase, ShutdownPhase::Complete);
    }
}
