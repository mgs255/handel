#![warn(rust_2018_idioms)]

#[macro_use]
extern crate clap;
use clap::App;
use log::*;

use templates::ComposeServiceMap;

use crate::compose::DockerCompose;
use crate::images::ContainerImages;
use crate::reference::RunningServices;
use crate::volumes::Volumes;
use config::HandelConfig;
use snafu::{ResultExt, Snafu};

mod compose;
mod config;
mod images;
mod reference;
mod templates;
mod utils;
mod volumes;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(
        r#"Problem occurred trying to read configuration file: {}\n{}"#,
        file,
        source
    ))]
    ConfigFile {
        file: String,
        source: crate::config::Error,
    },

    #[snafu(display(r#"Problem occurred trying to load service fragments.\n{}"#, source))]
    Fragments { source: crate::templates::TemplateError },

    #[snafu(display(
        r#"Problem occurred trying to build required services list.\n{}"#,
        source
    ))]
    BuildServices { source: crate::config::Error },

    #[snafu(display(
        r#"Problem occurred trying to generate scenario configuration for scenario: {}\n{}"#,
        scenario,
        source
    ))]
    Generate {
        scenario: String,
        source: crate::compose::Error,
    },

    #[snafu(display(
        r#"Problem occurred trying to write the docker-compose fileload service fragments.\n{}"#,
        source
    ))]
    WriteComposeFile { source: crate::utils::Error },
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[tokio::main]
async fn main() -> Result<()> {
    let yaml = load_yaml!("./cli.yml");
    let matches = App::from_yaml(yaml)
        .version(crate_version!())
        .get_matches();
    let config_file = matches
        .value_of("config")
        .expect("The input file is required - should default to handel.yml");
    let env = matches.value_of("env").expect("An environment is required");
    let since = matches
        .value_of("since")
        .expect("Expecting a value for since");
    let verbose = matches.occurrences_of("verbosity") as usize + 1;
    let quiet = matches.is_present("quiet");

    stderrlog::new()
        .module(module_path!())
        .quiet(quiet)
        .verbosity(verbose)
        .timestamp(stderrlog::Timestamp::Off)
        .init()
        .unwrap();

    let config = HandelConfig::new(config_file).context(ConfigFile {
        file: config_file.to_string(),
    })?;

    let scenario = matches.value_of("scenario")
        .or_else(|| {
            eprintln!("Expecting a scenario to be provided - the config file defines the following scenarios:\n\t{}",
                config.get_scenarios().join("\n\t") );
            std::process::exit(1);
        }).unwrap();

    let (versions, images, fragment_map, volumes) = tokio::join!(
        RunningServices::load(env, config.get_reference()),
        ContainerImages::find(since),
        ComposeServiceMap::new(config.template_dir(),config.get_port_range()),
        Volumes::initialise(config.volumes())
    );

    volumes.unwrap_or_else(|e| {
        error!("Unable to initialise volumes.\n{:?}", e);
        std::process::exit(1);
    });

    let fragment_map = fragment_map.context(Fragments)?;

    if !config.has_scenario(scenario) {
        eprintln!("Expecting a valid scenario to be provided ({} supplied) - the config file defines the following scenarios:\n\t{}",
                  scenario, config.get_scenarios().join("\n\t") );
        std::process::exit(1);
    }

    let required_services = config
        .build_service_list(scenario, &fragment_map)
        .context(BuildServices)?;

    let running_svcs = versions.unwrap_or_else(|e| {
        warn!(
            "Warning: Unable to fetch running versions data for {}\n{:?}",
            &env, e
        );
        Vec::new()
    });

    let images = images.unwrap_or_else(|e| {
        warn!(
            "\nWarning: Unable to read local container images from docker.\n{:?}",
            e
        );
        Vec::new()
    });

    if !required_services.is_empty() {
        let mut names = required_services
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>();
        names.sort();
        println!("\nRequired services:\n\t{}", names.join("\n\t"));
    }

    let contents =
        DockerCompose::generate(&required_services, &running_svcs, &images).context(Generate {
            scenario: scenario.to_string(),
        })?;

    let path = std::path::Path::new("docker-compose.yml");

    crate::utils::write_str_to_file(path, &contents).context(WriteComposeFile)
}
