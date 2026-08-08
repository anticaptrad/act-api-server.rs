use std::{
    env,
    future::pending,
    io::{self, IsTerminal, Read},
    time::Duration,
};

use tokio::{
    signal,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time,
};

const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Running,
    Draining,
    Forcing,
    Complete,
}

impl Phase {
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
enum Trigger {
    Sigint,
    Sigterm,
    StdinEof,
    Timeout,
    GracefulComplete,
}

impl Trigger {
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
enum Action {
    Ignore,
    BeginGraceful,
    Force,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct State {
    phase: Phase,
    stdin_is_tty: bool,
    first_trigger: Option<Trigger>,
}

impl State {
    const fn new(stdin_is_tty: bool) -> Self {
        Self {
            phase: Phase::Running,
            stdin_is_tty,
            first_trigger: None,
        }
    }

    fn apply(self, trigger: Trigger) -> Transition {
        match self.phase {
            Phase::Running => match trigger {
                Trigger::Sigint | Trigger::Sigterm => Transition {
                    state: Self {
                        phase: Phase::Draining,
                        first_trigger: Some(trigger),
                        ..self
                    },
                    action: Action::BeginGraceful,
                    show_force_hint: self.stdin_is_tty && matches!(trigger, Trigger::Sigint),
                },
                _ => Transition::ignored(self),
            },
            Phase::Draining => match trigger {
                Trigger::GracefulComplete => Transition {
                    state: Self {
                        phase: Phase::Complete,
                        ..self
                    },
                    action: Action::Complete,
                    show_force_hint: false,
                },
                Trigger::Sigint | Trigger::Sigterm | Trigger::Timeout => Transition {
                    state: Self {
                        phase: Phase::Forcing,
                        ..self
                    },
                    action: Action::Force,
                    show_force_hint: false,
                },
                Trigger::StdinEof
                    if self.stdin_is_tty && matches!(self.first_trigger, Some(Trigger::Sigint)) =>
                {
                    Transition {
                        state: Self {
                            phase: Phase::Forcing,
                            ..self
                        },
                        action: Action::Force,
                        show_force_hint: false,
                    }
                }
                _ => Transition::ignored(self),
            },
            Phase::Forcing | Phase::Complete => Transition::ignored(self),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Transition {
    state: State,
    action: Action,
    show_force_hint: bool,
}

impl Transition {
    const fn ignored(state: State) -> Self {
        Self {
            state,
            action: Action::Ignore,
            show_force_hint: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Config {
    grace_period: Duration,
    stdin_is_tty: bool,
}

impl Config {
    pub(super) fn from_env() -> Self {
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

        Self {
            grace_period: Duration::from_millis(grace_ms),
            stdin_is_tty: io::stdin().is_terminal(),
        }
    }
}

pub(super) async fn supervise(
    mut server_handle: JoinHandle<io::Result<()>>,
    graceful_tx: oneshot::Sender<()>,
    config: Config,
) -> anyhow::Result<()> {
    let mut signals = Signals::new()?;

    let first_trigger = tokio::select! {
        result = &mut server_handle => {
            flatten_server_result(result)?;
            tracing::info!(
                event = "shutdown_complete",
                phase = Phase::Complete.as_str(),
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

    let first = State::new(config.stdin_is_tty).apply(first_trigger);
    debug_assert_eq!(first.action, Action::BeginGraceful);
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
            state = state.apply(Trigger::GracefulComplete).state;
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
        _ = &mut deadline => Trigger::Timeout,
        () = receive_eof(&mut eof_receiver) => Trigger::StdinEof,
    };

    let forced = state.apply(force_trigger);
    anyhow::ensure!(
        forced.action == Action::Force,
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
        phase = Phase::Complete.as_str(),
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
struct Signals {
    sigint: signal::unix::Signal,
    sigterm: signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> io::Result<Self> {
        use signal::unix::{SignalKind, signal};

        Ok(Self {
            sigint: signal(SignalKind::interrupt())?,
            sigterm: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) -> io::Result<Trigger> {
        tokio::select! {
            signal = self.sigint.recv() => signal
                .map(|_| Trigger::Sigint)
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "SIGINT stream closed")),
            signal = self.sigterm.recv() => signal
                .map(|_| Trigger::Sigterm)
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "SIGTERM stream closed")),
        }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) -> io::Result<Trigger> {
        signal::ctrl_c().await?;
        Ok(Trigger::Sigint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_second_sigint_forces() {
        let first = State::new(true).apply(Trigger::Sigint);
        assert_eq!(first.action, Action::BeginGraceful);
        assert!(first.show_force_hint);

        let second = first.state.apply(Trigger::Sigint);
        assert_eq!(second.action, Action::Force);
        assert_eq!(second.state.phase, Phase::Forcing);
    }

    #[test]
    fn tty_eof_only_forces_after_sigint() {
        let before = State::new(true).apply(Trigger::StdinEof);
        assert_eq!(before.action, Action::Ignore);

        let first = State::new(true).apply(Trigger::Sigint);
        let eof = first.state.apply(Trigger::StdinEof);
        assert_eq!(eof.action, Action::Force);
    }

    #[test]
    fn non_tty_one_sigint_begins_graceful_without_force_hint() {
        let first = State::new(false).apply(Trigger::Sigint);
        assert_eq!(first.action, Action::BeginGraceful);
        assert!(!first.show_force_hint);
    }

    #[test]
    fn sigterm_always_begins_graceful_without_force_hint() {
        for stdin_is_tty in [false, true] {
            let first = State::new(stdin_is_tty).apply(Trigger::Sigterm);
            assert_eq!(first.action, Action::BeginGraceful);
            assert!(!first.show_force_hint);
        }
    }

    #[test]
    fn graceful_completion_reaches_complete_phase() {
        let first = State::new(false).apply(Trigger::Sigterm);
        let complete = first.state.apply(Trigger::GracefulComplete);
        assert_eq!(complete.action, Action::Complete);
        assert_eq!(complete.state.phase, Phase::Complete);
    }
}
