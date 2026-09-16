use std::path::PathBuf;

use anyhow::Result;
use argh::FromArgs;
use retrolib::{format::reflection_probe::ReflectionProbeData, util::file::map_file};

#[derive(FromArgs, PartialEq, Debug)]
/// inspect a reflection probe resource
#[argh(subcommand, name = "refl")]
pub struct Args {
    #[argh(positional)]
    /// REFL v3 resource (an appended retrotool asset footer is allowed)
    input: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let bytes = map_file(&args.input)?;
    let (probe, _) = ReflectionProbeData::slice(&bytes)?;
    println!("{}", serde_json::to_string_pretty(&probe)?);
    Ok(())
}
