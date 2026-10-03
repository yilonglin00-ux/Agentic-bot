use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("code") => {
            let mut project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut serve = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--project" => {
                        if let Some(value) = args.next() {
                            project = PathBuf::from(value);
                        }
                    }
                    // Internal: the per-project session host that all views attach to.
                    "--serve" => serve = true,
                    _ => {}
                }
            }
            let repo = std::env::args().any(|a| a == "--repo");
            let result = if serve {
                app_lib::code_terminal::serve(project)
            } else if !repo {
                // Noki app running: Noki Code as a program in this terminal
                // (own session, router, previews). Otherwise the stand-alone
                // JackOD session as before.
                match app_lib::code_cli::run(&project) {
                    Ok(true) => Ok(()),
                    Ok(false) => app_lib::code_terminal::run_cli(project),
                    Err(e) => Err(e),
                }
            } else {
                app_lib::code_terminal::run_cli(project)
            };
            if let Err(error) = result {
                eprintln!("[ERROR] {error}");
                std::process::exit(1);
            }
        }
        // Timeline client: one JSON request (argument or stdin) to the running
        // Noki app, prints its JSON reply. Used to record direct edits of a
        // Noki project in that project's history (and live terminal).
        Some("protokoll") => {
            use std::io::{BufRead, Read, Write};
            let anfrage = match args.next() {
                Some(a) if a != "-" => a,
                _ => {
                    let mut t = String::new();
                    let _ = std::io::stdin().read_to_string(&mut t);
                    t
                }
            };
            let wert: serde_json::Value = match serde_json::from_str(&anfrage) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[ERROR] Ungültiges JSON: {e}");
                    std::process::exit(2);
                }
            };
            let Ok(mut s) = std::os::unix::net::UnixStream::connect(app_lib::intelligence::code_socket_pfad()) else {
                eprintln!("[ERROR] Noki läuft nicht (kein Code-Dienst erreichbar).");
                std::process::exit(1);
            };
            let _ = writeln!(s, "{}", serde_json::json!({ "protokoll": true }));
            let _ = writeln!(s, "{wert}");
            let mut antwort = String::new();
            let _ = std::io::BufReader::new(&s).read_line(&mut antwort);
            print!("{antwort}");
            let ok = serde_json::from_str::<serde_json::Value>(&antwort).map(|v| v["typ"] == "ok").unwrap_or(false);
            std::process::exit(if ok { 0 } else { 1 });
        }
        _ => {
            eprintln!("Nutzung: noki code [--project PFAD] | noki protokoll '<json>'");
            std::process::exit(2);
        }
    }
}
