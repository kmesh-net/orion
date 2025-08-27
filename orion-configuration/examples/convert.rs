#![allow(clippy::print_stdout)]
use orion_configuration::config::Bootstrap;
use orion_error::{Context, Result};
use std::fs::File;

fn main() -> Result<()> {
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("bootstrap.yaml").with_context_msg("failed to open bootstrap.yaml")?,
    )
    .with_context_msg("failed to convert envoy to orion")?;
    let yaml = serde_yaml::to_string(&bootstrap).with_context_msg("failed to serialize orion")?;
    std::fs::write("orion.yaml", yaml.as_bytes())?;
    let bootstrap: Bootstrap =
        serde_yaml::from_reader(File::open("orion.yaml").with_context_msg("failed to open orion.yaml")?)
            .with_context_msg("failed to read yaml from file")?;
    let yaml = serde_yaml::to_string(&bootstrap).with_context_msg("failed to round-trip serialize orion")?;
    println!("{yaml}");
    Ok(())
}
