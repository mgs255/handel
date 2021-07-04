use std::collections::HashMap;

use log::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
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
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub struct ImageVersion {
    name: String,
    version: Option<String>,
    repository: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ComposeServiceFragment {
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<String>>,
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
    pub async fn new(templates_dir: &str) -> Result<ComposeServiceMap> {
        let mut templates = HashMap::new();

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

            templates.insert(stem.to_string(), service);
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
                        .ok_or(Error::ServiceName {
                            input: image_str.to_string(),
                        })?;

                Ok(ImageVersion {
                    name,
                    version: c.name("version").map(|m| m.as_str().to_string()),
                    repository: c.name("repo").map(|m| m.as_str().to_string()),
                })
            }
            _ => Err(Error::RepositoryFormat {
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
}
