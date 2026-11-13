use std::io::{self, BufRead, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    match e4a_host::parse_args(std::env::args().skip(1)) {
        Ok(e4a_host::Command::Help) => {
            print!("{}", e4a_host::USAGE);
            ExitCode::SUCCESS
        }
        Ok(e4a_host::Command::Acp) => match serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::from(1)
            }
        },
        Err(()) => {
            eprint!("{}", e4a_host::USAGE);
            ExitCode::from(2)
        }
    }
}

fn serve() -> Result<(), String> {
    let stdin = io::stdin();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.contains("\"session/cancel\"") {
                e4a_tools::request_cancel();
                continue;
            }
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdout = io::stdout().lock();
    while let Ok(line) = rx.recv() {
        match e4a_host::handle_exchange(&line) {
            Ok(replies) => {
                for reply in replies {
                    stdout
                        .write_all(reply.as_bytes())
                        .map_err(|err| err.to_string())?;
                }
                stdout.flush().map_err(|err| err.to_string())?;
            }
            Err(err) => eprintln!("{err}"),
        }
    }
    Ok(())
}
