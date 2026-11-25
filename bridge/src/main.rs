// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    env,
    io::{self, BufWriter},
    process::ExitCode,
};

// Interactive previews run as separate processes at the desktop's normal priority.
fn prefer_interactive_work() {
    #[cfg(unix)]
    // SAFETY: nice changes only the calling worker's scheduling priority.
    unsafe {
        libc::nice(3);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetPriorityClass, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
            NORMAL_PRIORITY_CLASS,
        };
        // SAFETY: this pseudo-handle is valid for the lifetime of this process.
        let worker = GetCurrentProcess();
        if GetPriorityClass(worker) == NORMAL_PRIORITY_CLASS {
            SetPriorityClass(worker, BELOW_NORMAL_PRIORITY_CLASS);
        }
    }
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();
    let command = command.as_deref().and_then(|s| s.to_str());
    if command == Some("--version") && arguments.len() == 0 {
        println!("helloneedle-worker {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if command == Some("--parallelism") && arguments.len() == 0 {
        println!("{}", helloneedle_worker::available_processors());
        return ExitCode::SUCCESS;
    }
    let Some(command @ ("search" | "preview")) = command else {
        eprintln!("Usage: helloneedle-worker <search|preview|--version|--parallelism>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("The request is read from standard input");
        return ExitCode::from(2);
    }
    if command == "search" {
        prefer_interactive_work();
    }
    match helloneedle_worker::protocol::run(
        command,
        io::stdin().lock(),
        BufWriter::new(io::stdout().lock()),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
