use chrono::{DateTime, Duration, Utc};
use log::*;
use regex::Regex;
use serde::Deserialize;
use tokio::process::Command;

use std::collections::HashMap;
use std::str::FromStr;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(r#"Unable spawn docker command.\n{}"#, source))]
    SpawnDockerCommand { source: std::io::Error },

    #[snafu(display(r#"Unable to read docker output.\n{}"#, source))]
    ReadChildOutput { source: std::io::Error },

    #[snafu(display(r#"Unable to parse docker output.\n{}"#, source))]
    ParseChildOutput { source: std::string::FromUtf8Error },

    #[snafu(display(r#"Unable to read docker output.\n{}"#, source))]
    ReadChildLine { source: std::io::Error },

    #[snafu(display(r#"Unable to terminate child process.\n{}"#, source))]
    WaitChild { source: std::io::Error },

    #[snafu(display(r#"Unable to parse container image json output.\n{}"#, source))]
    ParseContainerImage { source: serde_json::Error },

    #[snafu(display(r#"Not a valid value for duration {}."#, input))]
    NoValue { input: String },

    #[snafu(display(r#"Not a valid value for duration {}.\n{}"#, input, source))]
    ParseNumeric {
        input: String,
        source: std::num::ParseFloatError,
    },

    #[snafu(display(r#"Unable to read HTTP response body.\n{}"#, source))]
    HttpResponseBody { source: reqwest::Error },

    #[snafu(display(r#"Unable to parse HTTP response body as JSON.\n{}"#, source))]
    ParseResponseBody { source: serde_json::Error },
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct LocalContainerImage {
    #[serde(with = "docker_image_datetime_format")]
    created_at: DateTime<Utc>,
    #[serde(rename = "ID")]
    id: String,
    repository: String,
    tag: String,
    size: String,
}

#[derive(Debug, Clone)]
pub struct ContainerImage {
    name: String,
    container: LocalContainerImage,
}

#[derive(Debug, Clone)]
pub struct ContainerImages {
}

impl ContainerImage {
    fn new(name: &str, image: LocalContainerImage) -> ContainerImage {
        ContainerImage {
            name: name.to_string(),
            container: image,
        }
    }

    pub fn version(self: &ContainerImage) -> String {
        self.container.tag.to_string()
    }

    pub fn name(self: &ContainerImage) -> String {
        self.name.clone()
    }
}

impl ContainerImages {
    pub async fn find(since: &str) -> Result<Vec<ContainerImage>> {
        let since = parse_since_string(since)?;

        debug!("{} - got since duration: {:?}", module_path!(), &since);

        let container_age_limit = Utc::now()
            .checked_sub_signed(since)
            .expect("Internal error: unable to calculate minimum datetime from given since string");

        let output = Command::new("docker")
            .arg("images")
            .arg("--format")
            .arg("{{json .}}")
            .output()
            .await
            .context(ReadChildOutput)?;

        let mut image_map: HashMap<String, ContainerImage> = HashMap::new();

        String::from_utf8(output.stdout)
            .context(ParseChildOutput)?
            .lines()
            .filter_map(|line| serde_json::from_str::<LocalContainerImage>(line).ok())
            .filter(|lc| {
                debug!("id: {} tag: {} size: {}", &lc.id, &lc.tag, &lc.size);
                !matches!(lc.tag.as_str(), "TRUNK")
            })
            .filter_map(|lc| match "<none>".eq(&lc.repository) {
                true => None,
                false => Some(lc),
            })
            .take_while(|lc| {
                trace!(
                    "{} - parsed container from docker command output {:?}",
                    module_path!(),
                    &lc
                );

                if lc.created_at.le(&container_age_limit) {
                    info!(
                        "{} - ignoring container {} which is too old: {:?}",
                        module_path!(),
                        &lc.repository,
                        &lc.created_at.to_rfc2822()
                    );
                    return false;
                }

                true
            })
            .for_each(|lc| {
                let service_name = get_service_name_from_repository(&lc.repository);

                if let Some(sn) = service_name {
                    if !image_map.contains_key(&sn) ||
                        // Eurgh - https://github.com/rust-lang/rust/issues/53667
                        image_map.get(&sn).unwrap().version().ends_with("-SNAPSHOT")
                    {
                        let c = ContainerImage::new(&sn, lc);
                        image_map.insert(sn, c);
                    }
                }
            });

        let images = image_map.values().cloned().collect::<Vec<_>>();

        if !images.is_empty() {
            let names = images
                .iter()
                .filter(|&c| !c.container.tag.ends_with("TRUNK"))
                .map(|c| format!("{}:{}", &c.name, &c.container.tag))
                .collect::<Vec<_>>();
            println!("\nRecent images:\n\t{}", names.join("\n\t"));
        }

        Ok(images)
    }
}

fn parse_since_string(since: &str) -> Result<Duration> {
    let captures = Regex::new(r"(?P<value>\d{0,10}(?:\.\d{0,5})?)(?P<units>s|m|h|d|w)?")
        .map(|r| r.captures(since))
        .expect("Internal error: invalid regular expression");

    let captures = captures.ok_or(Error::NoValue {
        input: since.to_string(),
    })?;

    let value = captures.name("value").map_or("24", |d| d.as_str());

    let value = f64::from_str(value).context(ParseNumeric {
        input: value.to_string(),
    })?;

    Ok(match captures.name("units").map_or("h", |d| d.as_str()) {
        "s" => Duration::seconds(value.round() as i64),
        "m" => Duration::seconds((value * 60.0).round() as i64),
        "d" => Duration::seconds((value * 86400.0).round() as i64),
        "w" => Duration::seconds((value * 604800.0).round() as i64),
        _ => Duration::minutes((value * 60.0).round() as i64),
    })
}

fn get_service_name_from_repository(repo: &str) -> Option<String> {
    let re = Regex::new(r"(?:(?P<repo>[^/]+)/)?(?P<svc>[^:]+)(?::(?P<version>.+))?")
        .expect("Regex not valid");

    match re.captures(repo) {
        Some(c) => c.name("svc").map(|m| m.as_str().to_string()),
        _ => None,
    }
}

mod docker_image_datetime_format {
    use chrono::{DateTime, Local, TimeZone, Utc};
    use serde::{self, Deserialize, Deserializer};

    const FORMAT: &str = "%Y-%m-%d %H:%M:%S %z %Z";

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        // Convert the local date timestamps to UTC.
        Local
            .datetime_from_str(&s, FORMAT)
            .map_err(serde::de::Error::custom)
            .map(|l| DateTime::<Utc>::from_utc(l.naive_utc(), Utc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;

    #[test]
    fn test_parse_no_units() {
        assert_eq!(Duration::hours(3), parse_since_string("3").unwrap());
    }

    #[test]
    fn test_parse_5h() {
        assert_eq!(Duration::hours(5), parse_since_string("5h").unwrap());
    }

    #[test]
    fn test_parse_0_5h() {
        assert_eq!(Duration::minutes(30), parse_since_string("0.5h").unwrap());
    }

    #[test]
    fn test_parse_1d() {
        assert_eq!(Duration::hours(24), parse_since_string("1d").unwrap());
    }

    #[test]
    fn test_parse_20m() {
        assert_eq!(Duration::minutes(20), parse_since_string("20m").unwrap());
    }

    #[test]
    fn test_parse_1800s() {
        assert_eq!(Duration::minutes(30), parse_since_string("1800s").unwrap());
    }

    #[test]
    fn test_parse_0_25m() {
        assert_eq!(Duration::seconds(15), parse_since_string("0.25m").unwrap());
    }

    #[test]
    fn test_parse_2w() {
        assert_eq!(Duration::weeks(2), parse_since_string("2w").unwrap());
    }

    #[test]
    fn test_parse_0_01d() {
        assert_eq!(Duration::seconds(864), parse_since_string("0.01d").unwrap());
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct TestTime {
        #[serde(with = "docker_image_datetime_format")]
        created_at: DateTime<Utc>,
    }

    #[test]
    fn test_time_deserializer() {
        let expected: DateTime<Utc> = Utc.ymd(2020, 2, 27).and_hms(07, 35, 09);
        let test_data = r#"{"CreatedAt":"2020-02-27 07:35:09 +0000 UTC"}"#;
        let deser: TestTime = serde_yaml::from_str(test_data).unwrap();

        assert_eq!(expected, deser.created_at, "Times should match");
    }
}
