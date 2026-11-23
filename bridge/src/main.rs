// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    env,
    io::{self, BufWriter},
    process::ExitCode,
};

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();
    let command = command.as_deref().and_then(|s| s.to_str());
    let Some(command @ ("search" | "preview")) = command else {
        eprintln!("Usage: helloneedle-worker <search|preview>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("The request is read from standard input");
        return ExitCode::from(2);
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
