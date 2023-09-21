use log::*;
use serde::Deserialize;
use snafu::{ResultExt, Snafu};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(r#"Unable to create HTTP client.\n{}"#, source))]
    HttpClient { source: reqwest::Error },

    #[snafu(display(r#"Unable to create HTTP request to {}.\n{}"#, url, source))]
    HttpRequest { url: String, source: reqwest::Error },

    #[snafu(display(r#"Unable to read HTTP response body.\n{}"#, source))]
    HttpResponseBody { source: reqwest::Error },

    #[snafu(display(r#"Unable to parse HTTP response body as JSON.\n{}"#, source))]
    ParseResponseBody { source: serde_json::Error },

    #[snafu(display(
        r#"Unable to execute jq command, is it available and in the path?\n{}"#,
        source
    ))]
    JqExecute { source: std::io::Error },

    #[snafu(display(r#"Unable to get input for jq command"#))]
    JqStdinOpen,

    #[snafu(display(r#"Unable wait for jq command termination\n{}"#, source))]
    JqAwait { source: std::io::Error },

    #[snafu(display(r#"Unable to get input for jq command\n{}"#, source))]
    JqStdinHandle { source: std::io::Error },

    #[snafu(display(r#"Unable to get input for jq command\n{}"#, source))]
    JqStdoutHandle { source: std::io::Error },

    #[snafu(display(r#"Unable to read jq output as utf8\n{}"#, source))]
    JqStdoutRead { source: std::string::FromUtf8Error },
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Deserialize)]
pub struct RunningService {
    name: String,
    version: String,
}



pub struct RunningServices {
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Reference {
    url: String,
    env_mappings: Option<HashMap<String, String>>,
    jq_filter: Option<String>,
}

impl RunningService {

    #[cfg(test)]
    pub fn new(name: &str, version: &str) -> Self {
        RunningService {
            name: name.to_string(),
            version: version.to_string()
        }
    }

    pub fn name(self: &RunningService) -> String {
        self.name.clone()
    }

    pub fn version(self: &RunningService) -> String {
        self.version.clone()
    }
}

impl RunningServices {

    pub async fn load(env: &str, reference: &Option<Reference>) -> Result<Vec<RunningService>> {
        if reference.is_none() {
            return Ok(Vec::new());
        }

        // SAFETY - this is ok as we previously checked for the presence of None.
        let reference = reference.as_ref().unwrap();
        debug!("{} - Reference options: {:?}", module_path!(), &reference);

        // Map the incoming env str to using the env-mappings if they exist.
        let env = match &reference.env_mappings {
            Some(m) => m.get(env).map(|e| e.as_str()).unwrap_or(env),
            None => env,
        };

        let url = reference.url.replace("{env}", env);

        info!(
            "{} - Downloading versions from reference url at: {}",
            module_path!(),
            url
        );

        let response = reqwest::Client::builder()
            .build()
            .context(HttpClient)?
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .context(HttpRequest { url: url.clone() })?;

        debug!("{} - Waiting on body from reference", module_path!());
        let body = response.text().await.context(HttpResponseBody)?;

        debug!(
            "{} - Processing body of length {} from reference",
            module_path!(),
            body.len()
        );

        let filtered_body = match reference.jq_filter.as_ref() {
            Some(f) => apply_filter(f, &body).await,
            None => Ok(body),
        }?;

        let svcs = serde_json::from_str::<Vec<RunningService>>(&filtered_body)
            .context(ParseResponseBody)?;

        info!(
            "{} - Extracted {} versions from reference: {:?}",
            module_path!(),
            svcs.len(),
            &svcs
        );

        Ok(svcs)
    }
}

async fn apply_filter(filter: &str, input: &str) -> Result<String> {
    debug!("jq - Input: {}", input);
    debug!("jq - Filter: {}", filter);

    let mut jq = Command::new("jq")
        .arg(filter)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context(JqExecute)?;

    let jq_stdin = jq.stdin.as_mut().ok_or(Error::JqStdinOpen)?;

    jq_stdin
        .write_all(input.as_bytes())
        .await
        .context(JqStdinHandle)?;

    debug!("Waiting reading output from jq process....");
    let jq_result = jq.wait_with_output().await.context(JqAwait)?;

    let out = String::from_utf8(jq_result.stdout).context(JqStdoutRead)?;
    let err = String::from_utf8(jq_result.stderr).context(JqStdoutRead)?;
    if !err.is_empty() {
        error!(
            "{} failed to apply jq command to the given input - {}",
            module_path!(),
            err
        );
    }

    debug!("Returning processed reference versions: {:?}", &out);

    Ok(out)
}
