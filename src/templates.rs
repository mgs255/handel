use std::collections::{HashMap, HashSet};
use std::cmp::Ordering;
use std::ops::RangeInclusive;
use log::*;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::{Error};
use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum TemplateError {
    #[snafu(display(r#"Unable to parse container repository string: {}"#, input))]
    RepositoryFormat { input: String },

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

    #[snafu(display(r#"Unable to read fragment file: {}\n{}"#, file, source))]
    ReadTemplate {
        file: String,
        source: crate::utils::Error,
    },

    #[snafu(display(r#"Unable to parse fragment file: {}\n{}"#, file, source))]
    ParseTemplate {
        file: String,
        source: serde_yaml::Error,
    },

    #[snafu(display(r#"Unable to parse port mapping: {}"#, input))]
    PortMappingFormat {
        input: String,
    },
}

type Result<T, E = TemplateError> = std::result::Result<T, E>;

#[derive(Debug)]
pub struct ImageVersion {
    name: String,
    version: Option<String>,
    repository: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PortMapping {
    source: u16,
    target: u16
}

impl<'de> Deserialize<'de> for PortMapping {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        let captures = Regex::new(r"(?P<src>\d{1,5})(?::(?P<tgt>\d{1,5}))?")
            .map(|r| r.captures(&s))
            .expect("Internal error: invalid regular expression");

        let captures = captures
            .ok_or_else(||D::Error::custom("Port mapping unexpected"))?;

        let src_port = captures.name("src")
            .map(|m| m.as_str().parse::<u16>().unwrap_or(0))
            .ok_or_else(||D::Error::custom("No source port"))?;

        let tgt_port = captures.name("tgt")
            .map(|m| m.as_str().parse::<u16>().unwrap_or(0))
            .unwrap_or(src_port);

        Ok(PortMapping {
            source: src_port,
            target: tgt_port,
        })
    }

}

impl Serialize for PortMapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer,
    {
        let s = format!("{}:{}", self.source, self.target);
        serializer.serialize_str(&s)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ComposeServiceFragment {
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<PortMapping>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ComposeService {
    name: String,
    fragment: ComposeServiceFragment,
}

impl Ord for ComposeService {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl Eq for ComposeService {}

impl PartialOrd for ComposeService {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ComposeService {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

#[derive(Debug)]
pub struct ComposeServiceMap {
    templates: HashMap<String, ComposeService>,
}

impl ComposeServiceFragment {
    pub fn get_version(self: &ComposeServiceFragment) -> Option<ImageVersion> {
        ImageVersion::new(&self.image).ok()
    }
}

impl ComposeService {
    pub fn get_dependencies(self: &ComposeService) -> Vec<String> {
        let mut dependencies = Vec::<String>::new();

        if let Some(i) = &self.fragment.depends_on {
            for s in i {
                dependencies.push(s.to_string());
            }
        };

        dependencies
    }

    pub fn name(self: &ComposeService) -> String {
        self.name.to_string()
    }

    pub fn fragment(self: &ComposeService) -> &ComposeServiceFragment {
        &self.fragment
    }

    pub fn fragment_using_version(
        self: &ComposeService,
        version: Option<String>,
    ) -> ComposeServiceFragment {
        let fragment = &self.fragment;

        if version.is_some() && fragment.get_version().is_some() {
            let current_image_version = fragment.get_version().unwrap();

            let updated = ImageVersion {
                version,
                ..current_image_version
            };

            let updated_image = updated.get();

            let cloned = fragment.clone();

            let updated_fragment = ComposeServiceFragment {
                image: updated_image,
                ..cloned
            };

            return updated_fragment;
        }

        fragment.clone()
    }
}

impl ComposeServiceMap {
    pub async fn new(templates_dir: &str, port_range: Option<(u16,u16)>) -> Result<ComposeServiceMap> {

        let mut templates = HashMap::new();
        let mut target_ports: HashMap<u16,Vec<String>> = HashMap::new();
        let mut assigned_ports = HashSet::<u16>::new();

        let entries = std::fs::read_dir(templates_dir).context(TemplateDirectoryNotReadable {
            dir: templates_dir.to_string(),
        })?;

        for e in entries {
            let entry = e.context(DirEntryNotReadable {
                dir: templates_dir.to_string(),
            })?;

            let m = entry.metadata().context(MetadataNotReadable {
                dir: templates_dir.to_string(),
            })?;

            if m.is_dir() {
                continue;
            }

            let file_name = entry.file_name();
            let file_name = file_name.as_os_str().to_str();
            if file_name.is_none() {
                error!(
                    "{} - Unprocessable template file name: {:?}",
                    module_path!(),
                    entry.file_name()
                );
                continue;
            }

            let b = entry.path();
            let path = b.as_path();
            let stem = b.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let ext = b.extension().and_then(|s| s.to_str()).unwrap_or("");

            if !ext.eq("yml") && !ext.eq("yaml") {
                warn!(
                    "{} - Ignoring invalid file: {}",
                    module_path!(),
                    file_name.unwrap()
                );
                continue;
            }

            let file_name = file_name.unwrap();

            let contents = crate::utils::read_file_contents(path).context(ReadTemplate {
                file: file_name.to_string(),
            })?;

            let service_fragment: ComposeServiceFragment = serde_yaml::from_str(&contents)
                .context(ParseTemplate {
                    file: file_name.to_string(),
                })?;

            let service = ComposeService {
                name: stem.to_string(),
                fragment: service_fragment,
            };

            if let Some(p) = service.fragment.ports.as_ref() {
                p.iter()
                    .for_each(|pm| {
                        assigned_ports.insert(pm.source);
                        target_ports.entry(pm.source)
                            .or_insert_with(Vec::new)
                            .push(service.name.clone());
                    });
            };

            templates.insert(stem.to_string(), service);
        }

        if let true = target_ports.values().any(|s|s.len()>1) {
            let conflicting_ports = target_ports.iter()
                .filter(|(_,v)|v.len()>1)
                .map(|(k,v)|{
                    let conflicts = v.join(", ");
                    format!("\t{}\t{}", k, conflicts)
                })
                .collect::<Vec<_>>();

            eprintln!("Warning: The following host port conflicts exist:\n\tPort\tConflicting\n{}",
                      conflicting_ports.join("\n") );

            if let Some(r) = port_range {
                let free_ports = RangeInclusive::<u16>::new(r.0, r.1)
                    .filter(|p| !assigned_ports.contains(p) )
                    .take(conflicting_ports.len())
                    .map(|p|format!("\t{}", p))
                    .collect::<Vec<_>>();

                eprintln!("The following host ports are free in the port-range:\n{}",
                          free_ports.join("\n") );
            }

        }



        Ok(ComposeServiceMap { templates })
    }

    pub fn get_service_fragment(
        self: &ComposeServiceMap,
        service: &str,
    ) -> Option<&ComposeService> {
        self.templates.get(service)
    }
}

impl ImageVersion {
    pub fn new(image_str: &str) -> Result<ImageVersion> {
        let re = Regex::new(r"(?:(?P<repo>[^/]+)/)?(?P<svc>[^:]+)(?::(?P<version>.+))?")
            .expect("Regex not valid");

        let result = match re.captures(image_str) {
            Some(c) => {
                let name =
                    c.name("svc")
                        .map(|m| m.as_str().to_string())
                        .ok_or(TemplateError::ServiceName {
                            input: image_str.to_string(),
                        })?;

                Ok(ImageVersion {
                    name,
                    version: c.name("version").map(|m| m.as_str().to_string()),
                    repository: c.name("repo").map(|m| m.as_str().to_string()),
                })
            }
            _ => Err(TemplateError::RepositoryFormat {
                input: image_str.to_string(),
            }),
        };

        result
    }

    pub fn get(self: &ImageVersion) -> String {
        format!(
            "{}{}{}",
            match &self.repository {
                Some(r) => format!("{}/", &r),
                None => "".to_string(),
            },
            &self.name,
            match &self.version {
                Some(v) => format!(":{}", &v),
                None => "".to_string(),
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    #[test]
    fn test1() {
        let i = ImageVersion::new("12121212121.dkr.ecr.us-east-1.amazonaws.com/api:1.0.423")
            .unwrap();
        assert_eq!("api", i.name);
        assert_eq!(
            "12121212121.dkr.ecr.us-east-1.amazonaws.com",
            i.repository.unwrap()
        );
        assert_eq!("1.0.423", i.version.unwrap());
    }

    #[test]
    fn test2() {
        let i = ImageVersion::new("wurstmeister/kafka:2.12-2.4.0").unwrap();
        assert_eq!("kafka", i.name);
        assert_eq!("wurstmeister", i.repository.unwrap());
        assert_eq!("2.12-2.4.0", i.version.unwrap());
    }

    #[test]
    fn test3() {
        let i = ImageVersion::new("wurstmeister/kafka").unwrap();
        assert_eq!("kafka", i.name);
        assert_eq!("wurstmeister", i.repository.unwrap());
        assert_eq!(None, i.version);
    }

    #[test]
    fn test4() {
        let i = ImageVersion::new("mailhog/mailhog").unwrap();
        assert_eq!("mailhog", i.name);
        assert_eq!("mailhog", i.repository.unwrap());
        assert_eq!(None, i.version);
    }

    #[test]
    fn test5() {
        let i = ImageVersion::new("memcached:1.6.7").unwrap();
        assert_eq!("memcached", i.name);
        assert_eq!(None, i.repository);
        assert_eq!("1.6.7", i.version.unwrap());
    }

    #[test]
    fn test_fragment_deserialisation() {
        let t = r#"
image: foo
"#;
        let frag: ComposeServiceFragment = serde_yaml::from_str(t).unwrap();
        assert_eq!("foo", frag.image);
        assert!(frag.ports.is_none());
    }

    #[test]
    fn test_fragment_deserialisation2() {
        let t = r#"
image: foo
ports:
    - 121:343
    - 212:434
"#;
        let frag: ComposeServiceFragment = serde_yaml::from_str(t).unwrap();
        assert_eq!("foo", frag.image);
        assert!(frag.ports.is_some());
        let ports = frag.ports.unwrap();
        assert_eq!(ports.len(), 2);
        assert_eq!(121, ports.get(0).unwrap().source);
        assert_eq!(343, ports.get(0).unwrap().target);
        assert_eq!(212, ports.get(1).unwrap().source);
        assert_eq!(434, ports.get(1).unwrap().target);
    }

    #[test]
    fn test_fragment_deserialisation3() {
        let t = r#"
image: foo
platform: amd64
ports:
    - 121:343
    - 212
"#;
        let frag: ComposeServiceFragment = serde_yaml::from_str(t).unwrap();

        assert_eq!("foo", frag.image);
        assert!(frag.platform.is_some());
        assert_eq!("amd64", frag.platform.unwrap());
    }
}
