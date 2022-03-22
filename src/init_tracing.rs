use console_subscriber::ConsoleLayer;
use opentelemetry::{
    sdk::{propagation::TraceContextPropagator, Resource},
    KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use tracing::subscriber::set_global_default;
use tracing_error::ErrorLayer;
use tracing_log::LogTracer;
use tracing_subscriber::{
    filter::Targets, fmt::format::FmtSpan, layer::SubscriberExt, registry::LookupSpan, Layer,
    Registry,
};
use url::Url;

pub(super) fn init_tracing(
    servic_name: &'static str,
    opentelemetry_url: Option<&Url>,
    buffer_capacity: Option<usize>,
) -> anyhow::Result<()> {
    LogTracer::init()?;

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let targets = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info".into())
        .parse::<Targets>()?;

    let format_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(targets.clone());

    let subscriber = Registry::default()
        .with(format_layer)
        .with(ErrorLayer::default());

    if let Some(buffer_capacity) = buffer_capacity {
        let console_layer = ConsoleLayer::builder()
            .with_default_env()
            .event_buffer_capacity(buffer_capacity)
            .server_addr(([0, 0, 0, 0], 6669))
            .spawn();

        let subscriber = subscriber.with(console_layer);

        with_otel(subscriber, targets, servic_name, opentelemetry_url)
    } else {
        with_otel(subscriber, targets, servic_name, opentelemetry_url)
    }
}

fn with_otel<S>(
    subscriber: S,
    targets: Targets,
    servic_name: &'static str,
    opentelemetry_url: Option<&Url>,
) -> anyhow::Result<()>
where
    S: SubscriberExt + Send + Sync,
    for<'a> S: LookupSpan<'a>,
{
    if let Some(url) = opentelemetry_url {
        let tracer =
            opentelemetry_otlp::new_pipeline()
                .tracing()
                .with_trace_config(opentelemetry::sdk::trace::config().with_resource(
                    Resource::new(vec![KeyValue::new("service.name", servic_name)]),
                ))
                .with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_endpoint(url.as_str()),
                )
                .install_batch(opentelemetry::runtime::Tokio)?;

        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(targets);

        let subscriber = subscriber.with(otel_layer);

        set_global_default(subscriber)?;
    } else {
        set_global_default(subscriber)?;
    }

    Ok(())
}
