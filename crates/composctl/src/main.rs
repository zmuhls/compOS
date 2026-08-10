use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "composctl", version, about = "CompOS vault client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print version information.
    Version,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!(
                "composctl {} (vault format {})",
                compos_core::VERSION,
                compos_core::VAULT_FORMAT
            );
        }
    }
    Ok(())
}
