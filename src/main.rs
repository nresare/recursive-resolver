use crate::resolver::RecursiveResolver;
use anyhow::Result;
use clap::{Parser, Subcommand};
use hickory_proto::rr::domain::Name;
use hickory_proto::rr::RecordType;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

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
    let subscriber = FmtSubscriber::builder().with_max_level(Level::DEBUG).finish();
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
