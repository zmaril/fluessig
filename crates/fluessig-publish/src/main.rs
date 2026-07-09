use clap::Parser;
use fluessig_publish::Cli;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    fluessig_publish::run(cli)
}
