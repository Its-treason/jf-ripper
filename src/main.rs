use crate::cli::execute_cli;
mod bluray;
mod cli;
pub mod config;
pub mod rip;
pub mod tmdb;
pub mod transcode;
pub mod distributed;

fn main() {
    execute_cli();
}
