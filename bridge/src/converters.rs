// SPDX-License-Identifier: GPL-3.0-or-later
//! Locate a document tool and its data when launched from an installer or a system package.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn executable(name: &str) -> Option<PathBuf> {
    let filename = format!("{name}{}", env::consts::EXE_SUFFIX);
    let sibling = env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(&filename));
    let path = env::var_os("PATH").unwrap_or_default();
    let dirs = env::split_paths(&path).filter(|dir| dir.is_absolute());
    #[cfg(target_os = "macos")]
    let dirs =
        dirs.chain(["/opt/local/bin", "/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    sibling
        .into_iter()
        .chain(dirs.map(|dir| dir.join(&filename)))
        .find(|file| file.is_file())
}

pub(crate) fn command(name: &str) -> Option<Command> {
    let program = executable(name)?;
    let mut command = Command::new(&program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for a GUI-launched utility.
    }
    if let Some(prefix) = program.parent().and_then(Path::parent) {
        #[cfg(target_os = "macos")]
        let prefix = if prefix.ends_with("Contents") {
            prefix.join("Resources")
        } else {
            prefix.into()
        };
        if name == "antiword" {
            let maps = prefix.join("share/antiword");
            if env::var_os("ANTIWORDHOME").is_none()
                && maps.join("UTF-8.txt").is_file()
                && maps.join("fontnames").is_file()
            {
                command.env("ANTIWORDHOME", maps);
            }
        } else {
            #[cfg(any(target_os = "macos", windows))]
            {
                #[cfg(target_os = "macos")]
                let config = prefix.join("fonts/fonts.conf");
                #[cfg(windows)]
                let config = prefix.join("etc/fonts/fonts.conf");
                if config.is_file() && env::var_os("FONTCONFIG_FILE").is_none() {
                    command.env("FONTCONFIG_FILE", &config);
                    command.env("FONTCONFIG_PATH", config.parent()?);
                }
            }
        }
    }
    Some(command)
}
