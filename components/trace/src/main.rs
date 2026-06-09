use clap::{CommandFactory, Parser};

#[derive(Debug, Parser)]
#[command(
    name = "trace",
    version,
    about = "Inspect Daml ledger transactions",
    long_about = "Inspect Daml ledger transactions through the participant Ledger API."
)]
struct Cli {
    /// Optional message.
    #[arg(long, default_value = "Be a darling and give me a 'Hello there'")]
    message: String,
}

fn main() {
    if std::env::args_os().len() == 1 {
        let mut command = Cli::command();
        command.print_help().expect("failed to print help");
        println!();
        return;
    }

    let cli = Cli::parse();
    if cli.message.eq_ignore_ascii_case("hello there") {
        println!("General Kenobi!");
        return;
    } else {
        println!("I was hoping for Kenobi. Why are YOU here?");
        return;
    }
}
