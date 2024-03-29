use std::collections::HashMap;
use std::mem::swap;
use std::path::Path;

use serde::{Deserialize, Deserializer};
use regex::Regex;

use crate::reference::Reference;
use crate::templates::{ComposeService, ComposeServiceMap};
use crate::volumes::VolumeInitializer;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(r#"Cannot find template or scenario entry for: {}"#, input))]
    NotFound { input: String },

    #[snafu(display(r#"Unable to extract service name from repository string: {}"#, input))]
    ServiceName { input: String },

    #[snafu(display(
        r#"Unable to read the configured template directory, check your configuration: {}\n{}"#,
        dir,
        source
    ))]
    TemplateDirectoryNotReadable { dir: String, source: std::io::Error },

    #[snafu(display(
        r#"Unable to read directory entry, check your directory permissions: {}\n{}"#,
        dir,
        source
    ))]
    DirEntryNotReadable { dir: String, source: std::io::Error },

    #[snafu(display(
        r#"Unable to read directory entry metadata, check your directory permissions: {}\n{}"#,
        dir,
        source
    ))]
    MetadataNotReadable { dir: String, source: std::io::Error },

    #[snafu(display(r#"Unable to read config file: {}\n{}"#, file, source))]
    ReadConfig {
        file: String,
        source: crate::utils::Error,
    },

    #[snafu(display(r#"Unable to parse configuration file: {}\n{}"#, file, source))]
    ParseConfig {
        file: String,
        source: serde_yaml::Error,
    },

    #[snafu(display(
        r#"Unable to build scenario dependencies for scenario: {}\n{}"#,
        scenario,
        source
    ))]
    ScenarioDeps {
        scenario: String,
        #[snafu(source(from(Error, Box::new)))]
        source: Box<Error>,
    },

    #[snafu(display(
        r#"Unable to build service dependencies for service: {}\n{}"#,
        service,
        source
    ))]
    ServiceDeps {
        service: String,
        #[snafu(source(from(Error, Box::new)))]
        source: Box<Error>,
    },

    #[snafu(display(
        r#"Unable to build scenario dependencies for the specified scenario\n{}"#,
        source
    ))]
    ScenarioToplevel {
        #[snafu(source(from(ScenarioError, Box::new)))]
        source: Box<ScenarioError>,
    },
}

#[derive(Debug, Snafu)]
#[snafu(source(from(Error, Box::new)))]
pub struct ScenarioError(Box<Error>);

type Result<T, E = Error> = std::result::Result<T, E>;

pub type ServiceList = Vec<String>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HandelConfig {
    template_folder_path: String,

    #[serde(default)]
    #[serde(deserialize_with = "de_port_range")]
    port_range: Option<(u16,u16)>,

    reference: Option<Reference>,

    #[serde(deserialize_with = "de_scenarios")]
    scenarios: HashMap<String, ServiceList>,

    volume_init: Option<Vec<VolumeInitializer>>,
}

fn de_scenarios<'de, D>(deserializer: D) -> Result<HashMap<String, ServiceList>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = HashMap::<String, ServiceList>::deserialize(deserializer)?;
    Ok(v)
}

fn de_port_range<'de, D>(deserializer: D) -> Result<Option<(u16,u16)>, D::Error>
    where
        D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let re = Regex::new(r"(?P<p1>[1-9]\d{0,4})-(?P<p2>[1-9]\d{0,4})")
        .expect("Regex not valid");

    let ports = match re.captures(&s) {
        Some(c) => {
            let mut p1 = c.name("p1").map(|m| m.as_str().parse::<u16>().unwrap()).unwrap();
            let mut p2 = c.name("p2").map(|m| m.as_str().parse::<u16>().unwrap()).unwrap();
            if p1 > p2 {
                swap(&mut p1,&mut p2)
            }
            Some((p1,p2))
        },
        _ => None,
    };

    Ok(ports)
}

const EMPTY_SERVICE_LIST: &ServiceList = &Vec::<String>::new();

impl HandelConfig {
    pub fn new(file_name: &str) -> Result<HandelConfig> {
        let raw_file =
            crate::utils::read_file_contents(Path::new(&file_name)).context(ReadConfig {
                file: file_name.to_string(),
            })?;

        let config: HandelConfig = serde_yaml::from_str(&raw_file).context(ParseConfig {
            file: file_name.to_string(),
        })?;

        Ok(config)
    }

    pub fn template_dir(self: &HandelConfig) -> &str {
        &self.template_folder_path
    }

    pub fn get_reference(self: &HandelConfig) -> &Option<Reference> {
        &self.reference
    }

    pub fn get_port_range(self: &HandelConfig) -> Option<(u16,u16)> {
        self.port_range
    }

    pub fn volumes(self: &HandelConfig) -> &Option<Vec<VolumeInitializer>> {
        &self.volume_init
    }

    pub fn get_scenarios(self: &HandelConfig) -> Vec<String> {
        let mut scenarios = Vec::new();

        for k in self.scenarios.keys() {
            scenarios.push(k.to_string());
        }

        scenarios.sort();

        scenarios
    }

    pub fn scenario_services(self: &HandelConfig, scenario: &str) -> &ServiceList {
        self.scenarios.get(scenario).unwrap_or(EMPTY_SERVICE_LIST)
    }

    pub fn has_scenario(self: &HandelConfig, scenario: &str) -> bool {
        self.scenarios.contains_key(scenario)
    }

    pub fn build_service_list<'a>(
        self: &'a HandelConfig,
        scenario: &str,
        templates: &'a ComposeServiceMap,
    ) -> Result<Vec<&'a ComposeService>, Error> {
        let mut svcs: HashMap<String, &'a ComposeService> = HashMap::new();

        self.build_services_recursive(scenario, &mut svcs, templates)
            .context(ScenarioDeps {
                scenario: scenario.to_string(),
            })?;

        let mut svcs_list = Vec::new();

        for (_, v) in svcs {
            svcs_list.push(v);
        }

        svcs_list.sort();

        Ok(svcs_list)
    }

    fn build_services_recursive<'a>(
        self: &HandelConfig,
        parent: &str,
        svcs: &mut HashMap<String, &'a ComposeService>,
        templates: &'a ComposeServiceMap,
    ) -> Result<()> {
        let fragment = templates.get_service_fragment(parent);

        if let Some(f) = fragment {
            svcs.insert(parent.to_string(), f);
            for d in f.get_dependencies() {
                if !svcs.contains_key(&d) && templates.get_service_fragment(&d).is_some() {
                    svcs.insert(d.to_string(), templates.get_service_fragment(&d).unwrap());
                    self.build_services_recursive(&d, svcs, templates)
                        .context(ServiceDeps { service: d.clone() })?;
                }
            }
        } else if self.scenarios.contains_key(parent) {
            let services = self.scenario_services(parent);

            for s in services {
                if svcs.contains_key(s) {
                    continue;
                }

                self.build_services_recursive(s, svcs, templates)
                    .context(ScenarioDeps {
                        scenario: s.clone(),
                    })?;
            }
        } else {
            return Err(Error::NotFound {
                input: parent.to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    #[test]
    fn test_config_port_range_ok() {
        let t = r#"
template-folder-path: .
port-range: 1234-5678
scenarios:
  a:
    - b
"#;
        let frag: HandelConfig = serde_yaml::from_str(t).unwrap();
        assert_eq!(frag.port_range.unwrap(),(1234,5678));
    }

    #[test]
    fn test_config_port_range_missing() {
        let t = r#"
template-folder-path: .
scenarios:
  a:
    - b
"#;
        let frag: HandelConfig = serde_yaml::from_str(t).unwrap();
        assert!(frag.port_range.is_none());
    }

    #[test]
    fn test_config_port_range_missing_first() {
        let t = r#"
template-folder-path: .
port-range: -5678
scenarios:
  a:
    - b
"#;
        let frag: HandelConfig = serde_yaml::from_str(t).unwrap();
        assert!(frag.port_range.is_none());
    }

    #[test]
    fn test_config_port_range_missing_last() {
        let t = r#"
template-folder-path: .
port-range: 5678-
scenarios:
  a:
    - b
"#;
        let frag: HandelConfig = serde_yaml::from_str(t).unwrap();
        assert!(frag.port_range.is_none());
    }
}
