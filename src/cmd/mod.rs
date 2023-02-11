pub mod pak;
pub mod txtr;

use argh::FromArgs;

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    Pak(pak::Args),
    Txtr(txtr::Args),
}
