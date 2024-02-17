use crate::images::ContainerImage;
use crate::reference::RunningService;
use crate::templates::{ComposeService, ComposeServiceFragment};
use serde::Serialize;
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

        let mut svc_versions = Vec::<String>::new();

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
                    acc.insert(i.repository(), i);
                    acc
                });

        let versioned = svcs.iter()
            .fold(HashMap::<String,ComposeServiceFragment>::new(), |mut acc, s|{

                let repo = s.image();
                let service_name = s.name();
                let image_version = s.fragment().get_version();

                if image_version.is_none() {
                    eprintln!("Warning - cannot extract image information from template for \
                    service: {:?}", &service_name);
                    return acc;
                }

                let image_version = image_version.unwrap();

                let image_name = image_version.get_name();

                let version = container_lookup.get(&repo).map(|i|i.version())
                    .or_else(||running_svc_lookup.get(&service_name).map(|r|r.version()))
                    .or_else(||running_svc_lookup.get(&image_name).map(|r|r.version()))
                    .or_else(||image_version.get_version());

                let image_parts : Vec<&str> = repo.splitn(2, '/' ).collect();
                let plain_repo = match image_parts.len() {
                    2 => image_parts.get(1).unwrap(),
                    _ => repo.as_str()
                };


                let svc_name = if let Some(v) = &version {
                    format!("{} -> {}:{}", &service_name, &plain_repo, &v.clone())
                } else {
                    format!("{} -> {}", &service_name, &plain_repo)
                };

                svc_versions.push(svc_name.to_owned());

                let fragment = s.fragment_using_version(version);

                acc.insert(service_name, fragment);

                acc
            });

        println!(
            "\nGenerating docker compose file based on {} services:\n\t{}",
            svcs.len(),
            svc_versions.join("\n\t")
        );

        let compose = DockerCompose {
            version: String::from("3"),
            services: versioned,
        };

        serde_yaml::to_string(&compose).context(UnableToWrite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    #[test]
    fn test_can_use_image_name() {
        let t = r#"
image: 12121212121.dkr.ecr.us-east-1.amazonaws.com/contentrepo:1.0.400
"#;
        let frag: ComposeServiceFragment = serde_yaml::from_str(t).unwrap();

        let svcs = [&ComposeService::new("content-repo",
         "12121212121.dkr.ecr.us-east-1.amazonaws.com/contentrepo:1.0.423", &frag)];

        let running = [RunningService::new("contentrepo", "1.0.425")];
        let local = [];

        let result = DockerCompose::generate(&svcs, &running, &local);

        let expected = r#"version: '3'
services:
  content-repo:
    image: 12121212121.dkr.ecr.us-east-1.amazonaws.com/contentrepo:1.0.425
"#;

        assert!(result.is_ok());
        assert_eq!(expected, result.unwrap());
    }
}
