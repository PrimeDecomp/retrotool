mod argh_version;
mod cmd;
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
enum SubCommand {
    Pak(cmd::pak::Args),
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .format_target(false)
        .format_level(false)
        .init();

    let args: TopLevel = argh_version::from_env();
    let result = match args.command {
        SubCommand::Pak(args) => cmd::pak::run(args),
    };
    if let Err(e) = result {
        eprintln!("Failed: {e:?}");
        std::process::exit(1);
    }
}
