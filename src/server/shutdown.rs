use std::{
    env,
    future::pending,
    io::{self, IsTerminal, Read},
    time::{Duration, Instant},
};

use axum_server::Handle;
use tokio::{signal, sync::mpsc, task::JoinHandle, time};

const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 10_000;
const FORCE_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

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

    const fn is_signal(self) -> bool {
        matches!(self, Self::Sigint | Self::Sigterm)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Outcome {
    forced: bool,
    trigger: Trigger,
}

pub(super) async fn supervise(
    server_handle: JoinHandle<io::Result<()>>,
    server_control: Handle,
    config: Config,
) -> anyhow::Result<()> {
    let mut signals = Signals::new()?;
    let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
    let signal_task = tokio::spawn(async move {
        loop {
            match signals.recv().await {
                Ok(trigger) => {
                    if trigger_tx.send(Ok(trigger)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = trigger_tx.send(Err(error));
                    return;
                }
            }
        }
    });

    let result = supervise_with_triggers(server_handle, server_control, config, trigger_rx).await;
    signal_task.abort();
    let _ = signal_task.await;
    result.map(|_| ())
}

async fn supervise_with_triggers(
    mut server_handle: JoinHandle<io::Result<()>>,
    server_control: Handle,
    config: Config,
    mut triggers: mpsc::UnboundedReceiver<io::Result<Trigger>>,
) -> anyhow::Result<Outcome> {
    let started = Instant::now();
    let first_trigger = tokio::select! {
        result = &mut server_handle => {
            flatten_server_result(result)?;
            tracing::info!(
                event = "shutdown_complete",
                phase = Phase::Complete.as_str(),
                trigger = "server_complete",
                stdin_is_tty = config.stdin_is_tty,
                grace_ms = grace_ms(config.grace_period),
                signal_count = 0_u32,
                active_connections = server_control.connection_count(),
                elapsed_ms = elapsed_ms(started),
                forced = false,
                "server completed without a shutdown signal"
            );
            return Ok(Outcome {
                forced: false,
                trigger: Trigger::GracefulComplete,
            });
        }
        trigger = receive_trigger(&mut triggers) => trigger?,
    };

    let first = State::new(config.stdin_is_tty).apply(first_trigger);
    anyhow::ensure!(
        first.action == Action::BeginGraceful,
        "invalid first shutdown trigger: {first_trigger:?}"
    );
    let mut state = first.state;
    let mut signal_count = u32::from(first_trigger.is_signal());

    // The Handle owns both the listener and all accepted connection tasks. None
    // means the supervisor, rather than the server crate, owns the deadline and
    // can still honor a second interactive signal or Ctrl-D.
    server_control.graceful_shutdown(None);

    tracing::info!(
        event = "shutdown_requested",
        phase = state.phase.as_str(),
        trigger = first_trigger.as_str(),
        stdin_is_tty = state.stdin_is_tty,
        grace_ms = grace_ms(config.grace_period),
        signal_count,
        active_connections = server_control.connection_count(),
        elapsed_ms = elapsed_ms(started),
        forced = false,
        "graceful shutdown requested; listener is closing and active requests are draining"
    );

    let mut eof_receiver = if first.show_force_hint {
        tracing::info!(
            event = "shutdown_force_available",
            phase = state.phase.as_str(),
            trigger = first_trigger.as_str(),
            stdin_is_tty = true,
            grace_ms = grace_ms(config.grace_period),
            signal_count,
            active_connections = server_control.connection_count(),
            elapsed_ms = elapsed_ms(started),
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
                signal_count,
                active_connections = server_control.connection_count(),
                elapsed_ms = elapsed_ms(started),
                forced = false,
                "graceful shutdown complete"
            );
            return Ok(Outcome {
                forced: false,
                trigger: first_trigger,
            });
        }
        trigger = receive_trigger(&mut triggers) => trigger?,
        _ = &mut deadline => Trigger::Timeout,
        () = receive_eof(&mut eof_receiver) => Trigger::StdinEof,
    };

    if force_trigger.is_signal() {
        signal_count = signal_count.saturating_add(1);
    }
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
        first_trigger = first_trigger.as_str(),
        stdin_is_tty = state.stdin_is_tty,
        grace_ms = grace_ms(config.grace_period),
        signal_count,
        active_connections = server_control.connection_count(),
        elapsed_ms = elapsed_ms(started),
        forced = true,
        "forceful shutdown requested; active connections will be dropped"
    );

    // Unlike aborting Axum's outer future, Handle::shutdown reaches every
    // accepted connection task managed by axum-server.
    server_control.shutdown();
    match time::timeout(FORCE_SETTLE_TIMEOUT, &mut server_handle).await {
        Ok(result) => flatten_server_result(result)?,
        Err(_) => {
            let remaining = server_control.connection_count();
            tracing::error!(
                event = "shutdown_force_timeout",
                phase = state.phase.as_str(),
                trigger = force_trigger.as_str(),
                active_connections = remaining,
                settle_ms = grace_ms(FORCE_SETTLE_TIMEOUT),
                "server did not settle after immediate shutdown"
            );
            server_handle.abort();
            let _ = server_handle.await;
            anyhow::bail!(
                "server did not settle within {} ms after forced shutdown; {remaining} connections remained",
                grace_ms(FORCE_SETTLE_TIMEOUT)
            );
        }
    }

    tracing::info!(
        event = "shutdown_complete",
        phase = Phase::Complete.as_str(),
        trigger = force_trigger.as_str(),
        first_trigger = first_trigger.as_str(),
        stdin_is_tty = state.stdin_is_tty,
        grace_ms = grace_ms(config.grace_period),
        signal_count,
        active_connections = server_control.connection_count(),
        elapsed_ms = elapsed_ms(started),
        forced = true,
        "forceful shutdown complete"
    );
    Ok(Outcome {
        forced: true,
        trigger: force_trigger,
    })
}

fn grace_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn receive_trigger(
    receiver: &mut mpsc::UnboundedReceiver<io::Result<Trigger>>,
) -> io::Result<Trigger> {
    receiver.recv().await.unwrap_or_else(|| {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "shutdown signal stream closed",
        ))
    })
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
        use signal::unix::{signal, SignalKind};

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
    use std::{net::TcpListener, sync::Arc};

    use axum::{routing::get, Router};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::Notify,
    };

    use super::*;

    struct ServerFixture {
        server_handle: JoinHandle<io::Result<()>>,
        control: Handle,
        address: std::net::SocketAddr,
        request_started: Arc<Notify>,
        release_request: Option<Arc<Notify>>,
    }

    async fn start_server(release_request: bool) -> ServerFixture {
        let request_started = Arc::new(Notify::new());
        let release = release_request.then(|| Arc::new(Notify::new()));
        let route_started = Arc::clone(&request_started);
        let route_release = release.clone();

        let app = Router::new().route(
            "/hang",
            get(move || {
                let request_started = Arc::clone(&route_started);
                let release = route_release.clone();
                async move {
                    request_started.notify_one();
                    match release {
                        Some(release) => {
                            release.notified().await;
                            "drained"
                        }
                        None => pending::<&'static str>().await,
                    }
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        listener
            .set_nonblocking(true)
            .expect("make test listener nonblocking");
        let address = listener.local_addr().expect("test listener address");
        let control = Handle::new();
        let server = axum_server::from_tcp(listener)
            .expect("create test server")
            .handle(control.clone())
            .serve(app.into_make_service());
        let server_handle = tokio::spawn(server);

        ServerFixture {
            server_handle,
            control,
            address,
            request_started,
            release_request: release,
        }
    }

    async fn open_hanging_request(fixture: &ServerFixture) -> TcpStream {
        let mut stream = TcpStream::connect(fixture.address)
            .await
            .expect("connect test client");
        stream
            .write_all(b"GET /hang HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write test request");
        time::timeout(Duration::from_secs(1), fixture.request_started.notified())
            .await
            .expect("request did not start");
        wait_for_connections(&fixture.control, 1).await;
        stream
    }

    async fn wait_for_connections(control: &Handle, expected: usize) {
        time::timeout(Duration::from_secs(1), async {
            loop {
                if control.connection_count() == expected {
                    return;
                }
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "connection count did not reach {expected}; current={}",
                control.connection_count()
            )
        });
    }

    fn test_config(stdin_is_tty: bool, grace_period: Duration) -> Config {
        Config {
            grace_period,
            stdin_is_tty,
        }
    }

    fn trigger_channel() -> (
        mpsc::UnboundedSender<io::Result<Trigger>>,
        mpsc::UnboundedReceiver<io::Result<Trigger>>,
    ) {
        mpsc::unbounded_channel()
    }

    fn assert_stream_closed(result: io::Result<usize>) {
        match result {
            Ok(0) | Err(_) => {}
            Ok(read) => panic!("expected closed connection, read {read} bytes"),
        }
    }

    #[tokio::test]
    async fn sigterm_drains_a_real_active_request() {
        let fixture = start_server(true).await;
        let mut client = open_hanging_request(&fixture).await;
        let release = fixture
            .release_request
            .as_ref()
            .expect("release notifier")
            .clone();
        let (trigger_tx, trigger_rx) = trigger_channel();
        let supervisor = tokio::spawn(supervise_with_triggers(
            fixture.server_handle,
            fixture.control.clone(),
            test_config(false, Duration::from_secs(2)),
            trigger_rx,
        ));

        trigger_tx
            .send(Ok(Trigger::Sigterm))
            .expect("send SIGTERM");
        time::sleep(Duration::from_millis(30)).await;
        assert!(!supervisor.is_finished(), "active request was not drained");
        assert_eq!(fixture.control.connection_count(), 1);

        release.notify_one();
        let mut response = Vec::new();
        time::timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("graceful response timed out")
            .expect("read graceful response");
        assert!(
            response
                .windows(b"drained".len())
                .any(|part| part == b"drained")
        );

        let outcome = time::timeout(Duration::from_secs(1), supervisor)
            .await
            .expect("graceful supervisor timed out")
            .expect("supervisor task panicked")
            .expect("graceful supervisor failed");
        assert_eq!(
            outcome,
            Outcome {
                forced: false,
                trigger: Trigger::Sigterm,
            }
        );
        assert_eq!(fixture.control.connection_count(), 0);
    }

    #[tokio::test]
    async fn second_tty_sigint_force_closes_a_real_active_connection() {
        let fixture = start_server(false).await;
        let mut client = open_hanging_request(&fixture).await;
        let (trigger_tx, trigger_rx) = trigger_channel();
        let supervisor = tokio::spawn(supervise_with_triggers(
            fixture.server_handle,
            fixture.control.clone(),
            test_config(true, Duration::from_secs(2)),
            trigger_rx,
        ));

        trigger_tx
            .send(Ok(Trigger::Sigint))
            .expect("send first SIGINT");
        time::sleep(Duration::from_millis(30)).await;
        assert!(!supervisor.is_finished(), "first SIGINT forced shutdown");
        assert_eq!(fixture.control.connection_count(), 1);

        trigger_tx
            .send(Ok(Trigger::Sigint))
            .expect("send second SIGINT");
        let outcome = time::timeout(Duration::from_secs(1), supervisor)
            .await
            .expect("forced supervisor timed out")
            .expect("supervisor task panicked")
            .expect("forced supervisor failed");
        assert_eq!(
            outcome,
            Outcome {
                forced: true,
                trigger: Trigger::Sigint,
            }
        );

        let mut byte = [0_u8; 1];
        let read = time::timeout(Duration::from_secs(1), client.read(&mut byte))
            .await
            .expect("client did not observe forced close");
        assert_stream_closed(read);
        assert_eq!(fixture.control.connection_count(), 0);
    }

    #[tokio::test]
    async fn grace_deadline_force_closes_a_real_active_connection() {
        let fixture = start_server(false).await;
        let mut client = open_hanging_request(&fixture).await;
        let (trigger_tx, trigger_rx) = trigger_channel();
        let supervisor = tokio::spawn(supervise_with_triggers(
            fixture.server_handle,
            fixture.control.clone(),
            test_config(false, Duration::from_millis(50)),
            trigger_rx,
        ));

        trigger_tx
            .send(Ok(Trigger::Sigterm))
            .expect("send SIGTERM");
        let outcome = time::timeout(Duration::from_secs(1), supervisor)
            .await
            .expect("deadline supervisor timed out")
            .expect("supervisor task panicked")
            .expect("deadline supervisor failed");
        assert_eq!(
            outcome,
            Outcome {
                forced: true,
                trigger: Trigger::Timeout,
            }
        );

        let mut byte = [0_u8; 1];
        let read = time::timeout(Duration::from_secs(1), client.read(&mut byte))
            .await
            .expect("client did not observe deadline close");
        assert_stream_closed(read);
        assert_eq!(fixture.control.connection_count(), 0);
    }

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
