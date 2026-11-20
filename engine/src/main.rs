use std::process::ExitCode;

use engine::config::Config;

const USAGE: &str = "usage: engine serve [--port N] [--data-dir DIR] [--runtime BUILD] [--gpu NAME] [--sd-server PATH] [--idle-secs N]";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("serve") {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let cfg = match Config::from_args(&args[1..]) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match engine::serve(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
