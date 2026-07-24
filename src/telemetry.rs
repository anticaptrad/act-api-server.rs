//! Tracing + OpenTelemetry initialization.
//!
//! Always installs a console (fmt) subscriber. When `OTEL_EXPORTER_OTLP_ENDPOINT`
//! is set, an OTLP batch exporter is layered on top so traces flow to the
//! collector running in the k8s cluster. When it is unset the service degrades
//! gracefully to console-only tracing instead of panicking.

use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(service_name: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,act_api_server=debug"));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer());

    match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(endpoint) if !endpoint.is_empty() => {
            let tracer = opentelemetry_otlp::new_pipeline()
                .tracing()
                .with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_endpoint(endpoint),
                )
                .with_trace_config(
                    opentelemetry_sdk::trace::config().with_resource(Resource::new(vec![
                        KeyValue::new("service.name", service_name.to_string()),
                    ])),
                )
                .install_batch(opentelemetry_sdk::runtime::Tokio)?;
            registry
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .init();
            tracing::info!("OTLP trace export enabled");
        }
        _ => {
            registry.init();
            tracing::info!("OTEL_EXPORTER_OTLP_ENDPOINT not set; console tracing only");
        }
    }

    Ok(())
}

/// Flush and shut down the tracer provider so buffered spans are exported.
pub fn shutdown() {
    opentelemetry::global::shutdown_tracer_provider();
}
