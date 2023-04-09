use log::*;
use std::fs::File;
use std::io::prelude::*;
use std::io::Read;
use std::path::Path;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(r#"Unable to open file for writing: {}\n{}"#, file, source))]
    OpenForRead {
        file: String,
        source: std::io::Error,
    },

    #[snafu(display(r#"Unable to read the file: {}\n{}"#, file, source))]
    ReadFile {
        file: String,
        source: std::io::Error,
    },

    #[snafu(display(r#"Unable to open file for writing: {}\n{}"#, file, source))]
    OpenForWrite {
        file: String,
        source: std::io::Error,
    },

    #[snafu(display(r#"Unable to write to the file: {}\n{}"#, file, source))]
    WriteFile {
        file: String,
        source: std::io::Error,
    },
}

type Result<T, E = Error> = std::result::Result<T, E>;

pub fn read_file_contents(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).context(OpenForRead {
        file: path.to_string_lossy(),
    })?;
    let mut s = String::new();
    trace!(
        "{} - reading file {}",
        module_path!(),
        path.to_string_lossy()
    );
    file.read_to_string(&mut s).context(ReadFile {
        file: path.to_string_lossy(),
    })?;
    trace!(
        "{} - read {} bytes from file {}",
        module_path!(),
        s.len(),
        path.to_string_lossy()
    );
    Ok(s)
}

pub fn write_str_to_file(path: &Path, contents: &str) -> Result<()> {
    let display = path.display().to_string();

    trace!("{} - opening file {} for writing", module_path!(), &display);

    // Open a file in write-only mode, returns `io::Result<File>`
    let mut file = File::create(path).context(OpenForWrite { file: &display })?;

    trace!(
        "{} - writing {} bytes file {}",
        module_path!(),
        contents.len(),
        &display
    );

    file.write_all(contents.as_bytes())
        .context(WriteFile { file: &display })
}
