use anyhow::Context;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init_tracing(log_filter: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(log_filter).context("parse log filter")?;

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(true),
        )
        .try_init()
        .context("initialize tracing subscriber")
}
