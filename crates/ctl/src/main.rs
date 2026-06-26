//! `mikebom-operator-ctl` — optional debug CLI per plan §4.
//!
//! Initial subcommands intended for feature 001:
//!   - `crd`         emit NamespaceScan CRD YAML (chart-source-of-truth).
//!   - `dry-run`     resolve target pods + show planned Jobs without applying.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mikebom-operator-ctl", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Emit the NamespaceScan CRD as YAML (placeholder until feature 001).
    Crd,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Crd => {
            println!("# CRD emit lands in feature 001 (per plan §10).");
        }
    }
    Ok(())
}
