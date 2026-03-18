use crate::cli::execute_cli;
mod bluray;
mod cli;
pub mod transcode;

fn main() {
    execute_cli();
}
