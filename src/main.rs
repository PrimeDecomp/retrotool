mod argh_version;
mod cmd;
mod format;
mod util;

use argh::FromArgs;

#[derive(FromArgs, PartialEq, Debug)]
/// GameCube/Wii decompilation project tools.
struct TopLevel {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    Cmdl(cmd::cmdl::Args),
    Fmv0(cmd::fmv0::Args),
    Pak(cmd::pak::Args),
    Txtr(cmd::txtr::Args),
    Clsn(cmd::clsn::Args),
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .format_target(false)
        .format_level(false)
        .init();

    let args: TopLevel = argh_version::from_env();
    let result = match args.command {
        SubCommand::Cmdl(args) => cmd::cmdl::run(args),
        SubCommand::Fmv0(args) => cmd::fmv0::run(args),
        SubCommand::Pak(args) => cmd::pak::run(args),
        SubCommand::Txtr(args) => cmd::txtr::run(args),
        SubCommand::Clsn(args) => cmd::clsn::run(args),
    };
    if let Err(e) = result {
        eprintln!("Failed: {e:?}");
        std::process::exit(1);
    }
}
