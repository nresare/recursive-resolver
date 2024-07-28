use crate::resolver::RecursiveResolver;
use anyhow::Result;
use clap::Parser;
use hickory_proto::rr::domain::Name;
use hickory_proto::rr::RecordType;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

mod backend;
#[cfg(test)]
mod fake_backend;
mod resolver;
mod target;

#[derive(Parser, Debug)]
struct Cli {
    #[arg(required = true)]
    name: Name,
    #[arg(short = 't', long = "type", default_value = "A")]
    record_type: RecordType,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing()?;

    let args = Cli::parse();

    let resolver = RecursiveResolver::new();
    let result = resolver.resolve(&args.name, args.record_type).await?;

    println!("{:?}", result);

    Ok(())
}

fn setup_tracing() -> Result<()> {
    let subscriber = FmtSubscriber::builder().with_max_level(Level::DEBUG).finish();
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
