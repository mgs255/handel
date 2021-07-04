use serde::Serialize;

use crate::images::ContainerImage;
use crate::reference::RunningService;
use crate::templates::{ComposeService, ComposeServiceFragment};
use snafu::{ResultExt, Snafu};
use std::collections::HashMap;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("There was a problem writing the docker-compose file.\n{}", source))]
    UnableToWrite { source: serde_yaml::Error },
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Serialize)]
pub struct DockerCompose {
    version: String,
    services: HashMap<String, ComposeServiceFragment>,
}

impl DockerCompose {
    pub fn generate(
        svcs: &[&ComposeService],
        running: &[RunningService],
        local: &[ContainerImage],
    ) -> Result<String> {
        println!(
            "\nGenerating docker compose file based on {} services.",
            svcs.len()
        );

        let running_svc_lookup =
            running
                .iter()
                .fold(HashMap::<String, &RunningService>::new(), |mut acc, s| {
                    acc.insert(s.name(), s);
                    acc
                });

        let container_lookup =
            local
                .iter()
                .fold(HashMap::<String, &ContainerImage>::new(), |mut acc, i| {
                    acc.insert(i.name(), i);
                    acc
                });

        let versioned = svcs.iter()
            .fold(HashMap::<String,ComposeServiceFragment>::new(), |mut acc, s|{

                let image_version = s.fragment().get_version();

                if image_version.is_none() {
                    eprintln!("Warning - cannot extract image information from template for service: {:?}", s.name());
                    return acc;
                }

                let version = container_lookup.get(&s.name())
                    .map(|i|i.version())
                    .or_else(||running_svc_lookup.get(&s.name()).map(|r|r.version()));

                let fragment = s.fragment_using_version(version);

                acc.insert(s.name(), fragment);

                acc
            });

        let compose = DockerCompose {
            version: String::from("3"),
            services: versioned,
        };

        serde_yaml::to_string(&compose).context(UnableToWrite)
    }
}
