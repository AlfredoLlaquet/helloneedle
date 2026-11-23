// SPDX-License-Identifier: GPL-3.0-or-later

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Identity {
    path: String,
    path_bytes: String,
    path_encoding: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_url: Option<String>,
}

#[cfg(unix)]
fn native_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn native_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(unix)]
fn from_bytes(bytes: Vec<u8>) -> Result<PathBuf, &'static str> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    if bytes.contains(&0) {
        return Err("NUL in path");
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn from_bytes(bytes: Vec<u8>) -> Result<PathBuf, &'static str> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    if bytes.len() % 2 != 0 {
        return Err("Invalid UTF-16 path length");
    }
    let wide: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if wide.contains(&0) {
        return Err("NUL in path");
    }
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

pub(crate) fn decode(text: &str, encoded: Option<&str>) -> Result<PathBuf, &'static str> {
    let path = match encoded {
        Some(encoded) => from_bytes(
            STANDARD
                .decode(encoded)
                .map_err(|_| "Invalid base64 path")?,
        )?,
        None => {
            if text.contains('\0') {
                return Err("NUL in path");
            }
            PathBuf::from(text)
        }
    };
    if path.as_os_str().is_empty() {
        return Err("An empty path is not valid");
    }
    Ok(path)
}

pub(crate) fn identity(path: &Path) -> Identity {
    let bytes = native_bytes(path);
    let file_url = if cfg!(unix) && path.is_absolute() {
        use std::fmt::Write;
        let mut url = String::from("file://");
        for &byte in &bytes {
            if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
                url.push(char::from(byte));
            } else {
                write!(url, "%{byte:02X}").unwrap();
            }
        }
        Some(url)
    } else {
        None
    };
    Identity {
        path: path.to_string_lossy().into_owned(),
        path_bytes: STANDARD.encode(bytes),
        path_encoding: if cfg!(windows) { "utf16le" } else { "unix" },
        file_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_path_round_trips() {
        let path = Path::new("a folder/😀é.txt");
        let info = identity(path);
        assert_eq!(decode("", Some(&info.path_bytes)).unwrap(), path);
        assert!(decode("", Some("!")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_identity_and_url_survive_display() {
        let path = from_bytes(b"/folder/100% \xff".to_vec()).unwrap();
        let info = identity(&path);
        assert_eq!(
            info.file_url.as_deref(),
            Some("file:///folder/100%25%20%FF")
        );
        assert_eq!(decode(&info.path, Some(&info.path_bytes)).unwrap(), path);
    }
}
