use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "trace",
    version,
    about = "Inspect Daml ledger transactions",
    long_about = "Inspect Daml ledger transactions through the participant Ledger API."
)]
struct Cli {
    /// Optional message to include in the mock output.
    #[arg(long, default_value = "trace component is wired correctly")]
    message: String,
}

fn main() {
    let cli = Cli::parse();

    println!("dpm trace mock: {}", cli.message);
}
