//! # Metrics Collection Module

pub mod handler;

/// Initializes metrics exporters based on enabled features.
///
/// This function sets up the global metrics recorder depending on which feature flags
/// are enabled. It supports three modes:
/// - Both Prometheus and OpenTelemetry (fanout mode)
/// - Prometheus only
/// - OpenTelemetry only
///
/// When both features are enabled, metrics are sent to both exporters simultaneously.
/// The Prometheus recorder also spawns a background task for periodic upkeep of
/// histograms and summaries.
///
/// # Feature Requirements
/// Requires either `prometheus` or `otel` feature to be enabled.
/// When neither feature is enabled, this function is a no-op.
pub fn init() {
    // If both features are enabled, install a fanout so both receive metrics.
    #[cfg(all(feature = "prometheus", feature = "otel"))]
    {
        use metrics::set_global_recorder;
        use metrics_exporter_opentelemetry::Recorder as OtelRecorder;
        use metrics_exporter_prometheus::PrometheusBuilder;
        use metrics_util::layers::FanoutBuilder;
        use opentelemetry::global;
        use tracing::debug;

        debug!("Prometheus + otel metrics (fanout) is enabled");

        // Build Prometheus recorder (not installed globally) and keep handle for scrape.
        let prom_recorder = PrometheusBuilder::new().build_recorder();
        let prom_handle = prom_recorder.handle();
        crate::metrics::handler::set_prom_handle(prom_handle);
        // Spawn periodic upkeep for Prometheus histograms/summaries.
        {
            use std::time::Duration;
            let handle_for_task = prom_recorder.handle();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                loop {
                    tick.tick().await;
                    handle_for_task.run_upkeep();
                }
            });
        }

        // Build OTEL recorder and also set the global OTEL meter provider.
        // install_global() both builds and sets the metrics global recorder, but we
        // need the Recorder instance without setting it globally, so we use build().
        let (meter_provider, otel_recorder) = OtelRecorder::builder(env!("CARGO_PKG_NAME")).build();
        // Ensure OpenTelemetry SDK is set globally so OTLP exporter (if configured elsewhere)
        // can export the metrics produced by the recorder.
        global::set_meter_provider(meter_provider);

        // Fan out to both recorders.
        let fanout = FanoutBuilder::default()
            .add_recorder(prom_recorder)
            .add_recorder(otel_recorder)
            .build();
        let _ = set_global_recorder(fanout);
    }

    // If only Prometheus is enabled, keep existing behavior.
    #[cfg(all(feature = "prometheus", not(feature = "otel")))]
    {
        use metrics_exporter_prometheus::PrometheusBuilder;
        use tracing::debug;
        debug!("Prometheus metrics endpoint is enabled");
        if let Ok(handle) = PrometheusBuilder::new().install_recorder() {
            crate::metrics::handler::set_prom_handle(handle.clone());
            // Spawn periodic upkeep when using install_recorder() as well.
            use std::time::Duration;
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                loop {
                    tick.tick().await;
                    handle.run_upkeep();
                }
            });
        }
    }

    // If only OTEL is enabled, keep existing behavior.
    #[cfg(all(feature = "otel", not(feature = "prometheus")))]
    {
        use metrics_exporter_opentelemetry::Recorder;
        use tracing::debug;
        debug!("Enabling otel metrics reporting");
        let _ = Recorder::builder(env!("CARGO_PKG_NAME")).install_global();
    }
}

/// Records tool execution metrics.
///
/// Tracks tool call count and execution latency by plugin and tool name.
/// This provides insights into tool usage and performance characteristics.
///
/// # Arguments
/// * `plugin` - Name of the plugin executing the tool
/// * `tool` - Name of the tool being executed
/// * `latency_ms` - Tool execution time in milliseconds
///
/// # Feature Requirements
/// Requires either `prometheus` or `otel` feature to be enabled.
/// When neither feature is enabled, this function is a no-op.
pub fn record_tool_metrics(plugin: &str, tool: &str, latency_ms: f64) {
    #[cfg(any(feature = "prometheus", feature = "otel"))]
    {
        use metrics::{counter, histogram};
        counter!(
            "ark_tool_calls_total",
            "plugin" => plugin.to_string(),
            "tool" => tool.to_string()
        )
        .increment(1);
        histogram!(
            "ark_tool_latency_ms",
            "plugin" => plugin.to_string(),
            "tool" => tool.to_string()
        )
        .record(latency_ms);
    }
    #[cfg(not(any(feature = "prometheus", feature = "otel")))]
    {
        // No-op when metrics are disabled
        let _ = (plugin, tool, latency_ms);
    }
}

/// Records API HTTP request metrics.
///
/// Tracks request count and latency by endpoint path, HTTP method, and response status.
/// This provides observability into API usage patterns and performance.
///
/// # Arguments
/// * `path` - The API endpoint path (e.g., "/api/plugins")
/// * `method` - HTTP method (e.g., "GET", "POST")
/// * `status` - HTTP response status code
/// * `latency_ms` - Request processing time in milliseconds
///
/// # Feature Requirements
/// Requires either `prometheus` or `otel` feature to be enabled.
/// When neither feature is enabled, this function is a no-op.
pub fn record_api_http(path: &str, method: &str, status: u16, latency_ms: f64) {
    #[cfg(any(feature = "prometheus", feature = "otel"))]
    {
        use metrics::{counter, histogram};
        let status_s = status.to_string();
        counter!(
            "ark_api_calls_total",
            "path" => path.to_string(),
            "method" => method.to_string(),
            "status" => status_s.clone()
        )
        .increment(1);
        histogram!(
            "ark_api_latency_ms",
            "path" => path.to_string(),
            "method" => method.to_string(),
            "status" => status_s
        )
        .record(latency_ms);
    }
    #[cfg(not(any(feature = "prometheus", feature = "otel")))]
    {
        // No-op when metrics are disabled
        let _ = (path, method, status, latency_ms);
    }
}

/// Records MCP (Model Context Protocol) call metrics.
///
/// Tracks MCP protocol operation count and latency by transport type.
/// This monitors the performance of MCP protocol interactions.
///
/// # Arguments
/// * `transport` - Transport type (e.g., "streamable-http", "stdio")
/// * `latency_ms` - Protocol operation time in milliseconds
///
/// # Feature Requirements
/// Requires either `prometheus` or `otel` feature to be enabled.
/// When neither feature is enabled, this function is a no-op.
pub fn record_mcp_call(transport: &str, latency_ms: f64) {
    #[cfg(any(feature = "prometheus", feature = "otel"))]
    {
        use metrics::{counter, histogram};
        counter!(
            "ark_mcp_calls_total",
            "transport" => transport.to_string()
        )
        .increment(1);
        histogram!(
            "ark_mcp_latency_ms",
            "transport" => transport.to_string()
        )
        .record(latency_ms);
    }
    #[cfg(not(any(feature = "prometheus", feature = "otel")))]
    {
        // No-op when metrics are disabled
        let _ = (transport, latency_ms);
    }
}
