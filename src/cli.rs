use clap::{Parser, Subcommand};

use crate::bluray::{disc_info, list_titles, read_title};

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print information about a Blu-Ray disc
    DiscInfo {
        /// Path to a Blu-Ray drive or image
        #[arg(long, default_value = "/dev/sr0")]
        bd_path: String,
    },
    /// List titles available on a Blu-Ray disc
    ListTitles {
        /// Path to a Blu-Ray drive or image
        #[arg(long, default_value = "/dev/sr0")]
        bd_path: String,
    },
    /// Read a Blu-Ray title to a file
    ReadTitle {
        #[arg(long)]
        title: u32,
        #[arg(long)]
        out_path: String,
        /// Path to a Blu-Ray drive or image
        #[arg(long, default_value = "/dev/sr0")]
        bd_path: String,
    },
}

pub fn execute_cli() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::DiscInfo { bd_path } => disc_info(bd_path),
        Commands::ListTitles { bd_path } => list_titles(bd_path),
        Commands::ReadTitle { title, out_path, bd_path } => {
            read_title(*title, out_path, bd_path).map(|bytes| {
                println!("Read {} bytes", bytes);
            })
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
