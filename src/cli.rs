use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::bluray::read_title;

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(short, long)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    ReadTitle {
        #[arg(long)]
        title: u32,
        #[arg(long)]
        out_path: PathBuf,
    },
}

pub fn execute_cli() {
    let cli = Cli::parse();

    // You can see how many times a particular flag or argument occurred
    // Note, only flags can have multiple occurrences
    match cli.debug {
        true => println!("Debug mode is on"),
        false => println!("Debug mode is off"),
    }

    // You can check for the existence of subcommands, and if found use their
    // matches just as you would the top level cmd
    match &cli.command {
        Some(Commands::ReadTitle { out_path, title }) => {
            let device = "/dev/sr0";

            read_title(*title, out_path.to_str().unwrap(), device);
        }
        None => {}
    }
}
