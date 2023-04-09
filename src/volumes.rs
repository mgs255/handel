use log::*;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use http::Uri;
use tokio_stream::StreamExt;

use aws_config::meta::region::RegionProviderChain;
use s3::Client;
use s3::config::Region;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(
        r#"Unable to parse s3 source: {} as a valid URI.\n{}"#,
        s3source,
        source
    ))]
    InvalidSource {
        s3source: String,
        source: http::uri::InvalidUri,
    },

    #[snafu(display(r#"Unable to extract host from URI\n{}"#, s3source))]
    NoSourceHost { s3source: String },

    #[snafu(display("Unable to create temporary file\n{}", source))]
    CreateTmpFile { source: std::io::Error },

    #[snafu(display("Unable to persist temporary file.\n{}", source))]
    PersistTmpFile { source: tempfile::PersistError },

    #[snafu(display(
        "Unable to open zip archive for volume: {} source: {}.\n{}",
        name,
        volume_source,
        source
    ))]
    ZipArchive {
        name: String,
        volume_source: String,
        source: zip::result::ZipError,
    },

    #[snafu(display(
        "Unable to extract zip archive for volume: {} source: {}.\n{}",
        name,
        volume_source,
        source
    ))]
    ExtractZip {
        name: String,
        volume_source: String,
        source: zip::result::ZipError,
    },

    #[snafu(display("Unable download object from S3.\n{}", source))]
    S3GetObject {
        #[snafu(source(from(s3::error::SdkError<s3::operation::get_object::GetObjectError>, Box::new)))]
        source: Box<s3::error::SdkError<s3::operation::get_object::GetObjectError>>,
    },

    #[snafu(display("Error occurred streaming object from S3\n{}", source))]
    S3GetBytes {
        source: s3::primitives::ByteStreamError
    },
}

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, serde::Deserialize, Clone)]
pub struct VolumeInitializer {
    pub name: String,
    pub source: String,
    pub target: String,
}

#[derive(Debug)]
struct S3Location {
    bucket: String,
    key: String,
}

pub struct Volumes {}

impl Volumes {
    pub async fn initialise(volumes: &Option<Vec<VolumeInitializer>>) -> Result<()> {

        let vols = volumes.as_ref()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|v| {
                debug!("{} - considering volume {:?}", module_path!(), &v);
                let s = shellexpand::env(&v.source).ok();
                let t = shellexpand::env(&v.target).ok();

                debug!("{} - expanded source {:?}", module_path!(), &s);
                debug!("{} - expanded target {:?}", module_path!(), &t);

                if s.is_none() {
                    warn!(
                        "{} - Source for volume: {} is invalid: {}",
                        module_path!(),
                        &v.name,
                        &v.source
                    );
                    return None;
                }

                if t.is_none() {
                    warn!(
                        "{} - Target for volume: {} is invalid: {}",
                        module_path!(),
                        &v.name,
                        &v.target
                    );
                    return None;
                }

                let target_dir = t.unwrap().to_string();

                if !target_dir_valid(&target_dir) {
                    return None;
                }

                Some(VolumeInitializer {
                    source: s.unwrap().to_string(),
                    target: target_dir,
                    name: v.name.clone(),
                })
            })
            .collect::<Vec<_>>();

        info!("{} - Volumes: {:?}", module_path!(), &vols);

        for v in &vols {
            info!("Processing volume: {}", &v.name);
            match v.source.to_lowercase().starts_with("s3://") {
                true => unzip_file_from_s3(v).await?,
                false => unzip_local_file(v)?,
            };
        }

        println!("\nFinished initialising volumes.....");

        Ok(())
    }
}

fn unzip_local_file(volume: &VolumeInitializer) -> Result<()> {
    let from = PathBuf::from(&volume.source);
    let to = PathBuf::from(&volume.target);

    info!(
        "{} - Extracting zip for volume: {} to dir: {} ....",
        module_path!(),
        &volume.name,
        &volume.target
    );

    let file = File::open(from).context(CreateTmpFile)?;
    let mut archive = zip::ZipArchive::new(file).context(ZipArchive {
        name: volume.name.to_string(),
        volume_source: volume.source.to_string(),
    })?;

    archive.extract(to).context(ExtractZip {
        name: volume.name.to_string(),
        volume_source: volume.source.to_string(),
    })?;

    Ok(())
}

fn target_dir_valid(dir: &str) -> bool {
    let path_buf = PathBuf::from(dir);
    let path = path_buf.as_path();

    if !path.exists() {
        let _ = std::fs::create_dir_all(path);
        return true;
    }

    dir_is_empty(path)
}

fn dir_is_empty(path: &Path) -> bool {
    path.read_dir().map_or(false, |mut i| i.next().is_none())
}

fn extract_bucket_and_key(uri: &Uri) -> Result<S3Location> {
    uri.host()
        .ok_or(Error::NoSourceHost {
            s3source: uri.to_string(),
        })
        .map(|s| {
            let s3loc = S3Location {
                bucket: s.to_string(),
                key: uri.path().replacen('/', "", 1),
            };

            s3loc
        })
}
fn parse_uri_as_bucket_and_key(path: &str) -> Result<S3Location> {
    let uri = path.parse::<Uri>().context(InvalidSource {
        s3source: path.to_string(),
    })?;

    extract_bucket_and_key(&uri)
}

async fn unzip_file_from_s3(volume: &VolumeInitializer) -> Result<()> {

    let region_provider = RegionProviderChain::default_provider()
        .or_else(Region::new("us-east-1"));
    let shared_config = aws_config::from_env().region(region_provider).load().await;
    let client = Client::new(&shared_config);

    let s3loc = parse_uri_as_bucket_and_key(&volume.source)?;

    debug!(
        "{} - attempting to download from s3 source {:?}",
        module_path!(),
        &s3loc
    );

    let mut file = tempfile::tempfile().context(CreateTmpFile)?;

    let resp = client
        .get_object()
        .bucket(&s3loc.bucket)
        .key(s3loc.key)
        .send()
        .await
        .context(S3GetObject)?;

    debug!("{} - got s3 object resp {:?}", module_path!(), &resp);

    let mut data = resp.body;

    let mut bytes_downloaded: usize = 0;
    while let Some(bytes) = data.try_next().await.context(S3GetBytes)? {
        bytes_downloaded += bytes.len();
        trace!(
            "{} - got {} bytes from source {}",
            module_path!(),
            bytes_downloaded,
            &volume.source
        );
        match file.write_all(&bytes) {
            Ok(_) => {
                trace!(
                    "{} - wrote {} bytes from {} to temporary file",
                    module_path!(),
                    bytes_downloaded,
                    &volume.source
                );
                print!(".")
            }
            Err(e) => {
                error!(
                    "{} - writing to temporary file: {:?}",
                    module_path!(),
                    e.to_string()
                );
                break;
            }
        }
    }

    info!(
        "\nDownloaded {:?} bytes for {} from {}",
        bytes_downloaded, &volume.name, &volume.source
    );

    let file = file;

    let mut archive = zip::ZipArchive::new(file).context(ZipArchive {
        name: volume.name.to_string(),
        volume_source: volume.source.to_string(),
    })?;

    let target_path = PathBuf::from(&volume.target);
    archive.extract(target_path).context(ExtractZip {
        name: volume.name.to_string(),
        volume_source: volume.source.to_string(),
    })?;

    info!(
        "\n{} - Extracted zip file of {:?} bytes from {} to {}", module_path!(),
        bytes_downloaded,
        &volume.source,
        &volume.target
    );

    Ok(())
}
