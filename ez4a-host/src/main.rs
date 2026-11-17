use std::io::{self, BufRead, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    match ez4a_host::parse_args(std::env::args().skip(1)) {
        Ok(ez4a_host::Command::Help) => {
            print!("{}", ez4a_host::USAGE);
            ExitCode::SUCCESS
        }
        Ok(ez4a_host::Command::Acp) => match serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::from(1)
            }
        },
        Ok(ez4a_host::Command::Run { root, prompt }) => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    eprintln!("{err}");
                    return ExitCode::from(1);
                }
            };
            match runtime.block_on(ez4a_host::run_turn(&root, &prompt)) {
                Ok(answer) => {
                    println!("{answer}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("{err}");
                    ExitCode::from(1)
                }
            }
        }
        Err(()) => {
            eprint!("{}", ez4a_host::USAGE);
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
                ez4a_tools::request_cancel();
                continue;
            }
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdout = io::stdout().lock();
    while let Ok(line) = rx.recv() {
        match ez4a_host::handle_exchange(&line) {
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
