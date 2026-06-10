mod auth;
mod cli;
mod config;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command, ProfileCommand};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
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
            if cli.message.eq_ignore_ascii_case("hello there") {
                println!("General Kenobi!");
            } else {
                println!("I was hoping for Kenobi. Why are YOU here?");
            }
            Ok(())
        }
    }
}
