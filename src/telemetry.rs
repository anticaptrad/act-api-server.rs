//! Tracing + OpenTelemetry initialization.
//!
//! Always installs a console (fmt) subscriber. When `OTEL_EXPORTER_OTLP_ENDPOINT`
//! is set, an OTLP batch exporter is layered on top so traces flow to the
//! collector running in the k8s cluster. When it is unset the service degrades
//! gracefully to console-only tracing instead of panicking.

use std::{sync::Arc, time::Duration};

use next_loggers::{
    JsonObject, LogLevel, LogRecord, Logger, LoggerError, Options, Transport, json,
};
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource, runtime,
    trace::{Tracer, TracerProvider},
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct TelemetryGuard {
    ores_logger: Logger,
    tracer_provider: Option<TracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if self.ores_logger.close().is_err() {
            eprintln!("telemetry: Ores logger shutdown failed");
        }
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

pub fn init(service_name: &str) -> anyhow::Result<TelemetryGuard> {
    let filter = EnvFilter::try_new(
        crate::flags::var("RUST_LOG").unwrap_or_else(|_| "info,act_api_server=debug".into()),
    )?;
    let resource = Resource::new(vec![
        KeyValue::new("service.name", service_name.to_string()),
        KeyValue::new("service.namespace", "anticaptrad"),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);
    let endpoint = crate::flags::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let (tracer_provider, tracer) = endpoint
        .as_deref()
        .and_then(|value| build_tracer_provider(value, resource, service_name).ok())
        .map_or((None, None), |(provider, tracer)| {
            global::set_tracer_provider(provider.clone());
            (Some(provider), Some(tracer))
        });
    install_subscriber(filter, tracer);
    tracing::info!(
        otel.trace_exporter = tracer_provider.is_some(),
        "telemetry initialized"
    );

    let ores_logger = Logger::new(Options {
        app_name: service_name.to_string(),
        name: Some("api-server".to_string()),
        console: false,
        transports: vec![Arc::new(TracingBridgeTransport)],
        ..Options::default()
    });
    let _ = ores_logger
        .info(vec![json!("telemetry initialized")])
        .add_fields(JsonObject::from_iter([
            ("service.name".to_string(), json!(service_name)),
            ("service.namespace".to_string(), json!("anticaptrad")),
            ("log.destination".to_string(), json!("tracing-bridge")),
        ]))
        .send();

    Ok(TelemetryGuard {
        ores_logger,
        tracer_provider,
    })
}

fn build_tracer_provider(
    endpoint: &str,
    resource: Resource,
    service_name: &str,
) -> Result<(TracerProvider, Tracer), ()> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|_| ())?;
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .with_resource(resource)
        .build();
    use opentelemetry::trace::TracerProvider as _;
    let tracer = provider.tracer(service_name.to_string());
    Ok((provider, tracer))
}

fn install_subscriber(filter: EnvFilter, tracer: Option<Tracer>) {
    let result = match tracer {
        Some(tracer) => tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init(),
        None => tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .try_init(),
    };
    if result.is_err() {
        eprintln!("telemetry: subscriber already initialized; keeping existing subscriber");
    }
}

/// Bridges Ores' language-neutral structured record into the existing
/// tracing/OTLP pipeline, avoiding a second global subscriber.
struct TracingBridgeTransport;

impl Transport for TracingBridgeTransport {
    fn write(&self, record: &LogRecord) -> Result<(), LoggerError> {
        let encoded = record.to_json()?;
        match record.level {
            LogLevel::Trace => tracing::trace!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Debug => tracing::debug!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Info => tracing::info!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Warn => tracing::warn!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Error => tracing::error!(ores.record = %encoded, "Ores structured log"),
            LogLevel::Fatal => tracing::error!(ores.record = %encoded, "Ores fatal structured log"),
        }
        Ok(())
    }

    fn is_open_telemetry(&self) -> bool {
        true
    }
}
