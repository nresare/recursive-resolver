use crate::resolver::RecursiveResolver;
use anyhow::Result;
use clap::{Parser, Subcommand};
use hickory_proto::rr::domain::Name;
use hickory_proto::rr::RecordType;
use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::Config;
use opentelemetry_sdk::{runtime, Resource};
use opentelemetry_semantic_conventions::resource::{SERVICE_NAME, SERVICE_VERSION};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry};

mod backend;
mod daemon;
#[cfg(test)]
mod fake_backend;
mod resolver;
mod target;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Daemon {
        #[arg(short, long, default_value_t = 53)]
        port: u16,
    },
    Lookup {
        #[arg()]
        name: Name,

        #[arg(short = 't', long, default_value_t = RecordType::A)]
        record_type: RecordType,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing()?;

    let args = Cli::parse();

    let resolver = RecursiveResolver::new();
    match args.command {
        Commands::Lookup { name, record_type } => {
            let result = resolver.resolve(&name, record_type).await?;
            println!("{:?}", result);
        }
        Commands::Daemon { port } => daemon::daemon(resolver, port).await?,
    }
    Ok(())
}

fn setup_tracing() -> Result<()> {
    let otlp_exporter =
        opentelemetry_otlp::new_exporter().tonic().with_endpoint("http://localhost:4317");

    let resource = Resource::new(vec![
        KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
        KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
    ]);

    let provider = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otlp_exporter)
        .with_trace_config(Config::default().with_resource(resource))
        .install_batch(runtime::Tokio)?;

    let tracer = provider.tracer("daemon");

    let telemetry =
        tracing_opentelemetry::layer().with_tracer(tracer).with_filter(LevelFilter::DEBUG);

    let subscriber = Registry::default().with(telemetry);

    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
