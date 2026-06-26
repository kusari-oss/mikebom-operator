//! `mikebom-operator-ctl` — debug CLI for the mikebom Kubernetes operator.
//!
//! `crd`: emit the NamespaceScan `CustomResourceDefinition` as YAML.
//! See `specs/001-crd-yaml-generator/contracts/cli.md` for the stability contract.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use operator::crds::namespace_scan::NamespaceScan;
use operator::crds::serialize::crd_yaml;

#[derive(Parser)]
#[command(name = "mikebom-operator-ctl", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Emit the NamespaceScan CRD as a Kubernetes-compatible YAML manifest.
    Crd {
        /// Write the YAML to this file path instead of stdout. Overwrites any existing file.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mikebom-operator-ctl: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn try_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Crd { output } => emit_crd(output),
    }
}

fn emit_crd(output: Option<PathBuf>) -> Result<()> {
    let yaml = crd_yaml::<NamespaceScan>();
    match output {
        None => {
            print!("{yaml}");
            Ok(())
        }
        Some(path) => std::fs::write(&path, &yaml)
            .with_context(|| format!("failed to write CRD YAML to {}", path.display())),
    }
}
