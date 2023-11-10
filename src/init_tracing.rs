use crate::config::{LogFormat, OpenTelemetry, Tracing};
use console_subscriber::ConsoleLayer;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{propagation::TraceContextPropagator, Resource};
use tracing::subscriber::set_global_default;
use tracing_error::ErrorLayer;
use tracing_log::LogTracer;
use tracing_subscriber::{layer::SubscriberExt, registry::LookupSpan, Layer, Registry};

pub(super) fn init_tracing(tracing: &Tracing) -> color_eyre::Result<()> {
    color_eyre::install()?;

    LogTracer::init()?;

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let format_layer = tracing_subscriber::fmt::layer();

    match tracing.logging.format {
        LogFormat::Compact => with_format(format_layer.compact(), tracing),
        LogFormat::Json => with_format(format_layer.json(), tracing),
        LogFormat::Normal => with_format(format_layer, tracing),
        LogFormat::Pretty => with_format(format_layer.pretty(), tracing),
    }
}

fn with_format<F>(format_layer: F, tracing: &Tracing) -> color_eyre::Result<()>
where
    F: Layer<Registry> + Send + Sync,
{
    let format_layer = format_layer.with_filter(tracing.logging.targets.targets.clone());

    let subscriber = Registry::default()
        .with(format_layer)
        .with(ErrorLayer::default());

    if let Some(address) = tracing.console.address {
        println!("Starting console on {address}");

        let console_layer = ConsoleLayer::builder()
            .event_buffer_capacity(tracing.console.buffer_capacity)
            .server_addr(address)
            .spawn();

        let subscriber = subscriber.with(console_layer);

        with_subscriber(subscriber, &tracing.opentelemetry)
    } else {
        with_subscriber(subscriber, &tracing.opentelemetry)
    }
}

fn with_subscriber<S>(subscriber: S, otel: &OpenTelemetry) -> color_eyre::Result<()>
where
    S: SubscriberExt + Send + Sync,
    for<'a> S: LookupSpan<'a>,
{
    if let Some(url) = otel.url.as_ref() {
        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_trace_config(
                opentelemetry_sdk::trace::config().with_resource(Resource::new(vec![
                    KeyValue::new("service.name", otel.service_name.clone()),
                ])),
            )
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(url.as_str()),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio)?;

        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(otel.targets.as_ref().targets.clone());

        let subscriber = subscriber.with(otel_layer);

        set_global_default(subscriber)?;
    } else {
        set_global_default(subscriber)?;
    }

    Ok(())
}
