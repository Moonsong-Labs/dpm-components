mod auth;
mod cli;
mod config;
mod ledger;
mod style;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command, ProfileCommand};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            style::print_error(error);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    if std::env::args_os().len() == 1 {
        let mut command = Cli::command();
        command
            .print_help()
            .map_err(|error| format!("failed to print help: {error}"))?;
        println!();
        return Ok(());
    }

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Profile { command }) => match command {
            ProfileCommand::Add(args) => config::add_profile(args),
            ProfileCommand::List(args) => config::list_profiles(args),
            ProfileCommand::Show(args) => config::show_profile(args),
        },
        Some(Command::Login(args)) => auth::login(args),
        Some(Command::Logout(args)) => auth::logout(args),
        None => {
            if cli.update_id.is_some() {
                ledger::trace_update(&cli)
            } else if cli.message.eq_ignore_ascii_case("hello there") {
                println!("{}", style::heading("✨ General Kenobi!"));
                Ok(())
            } else {
                println!(
                    "{}",
                    style::warning("I was hoping for Kenobi. Why are YOU here?")
                );
                Ok(())
            }
        }
    }
}
