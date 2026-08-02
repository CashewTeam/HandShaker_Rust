mod cli;
mod output;

use clap::error::ErrorKind;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, OutputFormat, command_name};
use crate::output::render_error;

#[tokio::main]
async fn main() {
    let fallback_output = output_from_raw_args();
    let cli = match Cli::try_parse_localized() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return;
        }
        Err(_error) => {
            let app_error = handshaker_rust::Error::Usage(
                handshaker_rust::i18n::text("cli.parse_error").to_string(),
            );
            render_error(&app_error, "unknown", fallback_output);
            std::process::exit(app_error.exit_code());
        }
    };
    let command = command_name(&cli.command);
    let output = cli.output;
    let is_shell = matches!(cli.command, cli::Command::Shell);
    let filter = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .init();

    let result = if is_shell {
        cli::run(cli).await
    } else {
        tokio::select! {
            result = cli::run(cli) => result,
            signal = tokio::signal::ctrl_c() => match signal {
                Ok(()) => Err(handshaker_rust::Error::Interrupted),
                Err(error) => Err(handshaker_rust::Error::LocalIo(
                    handshaker_rust::i18n::format("error.ctrl_c", &[&error.to_string()]),
                )),
            },
        }
    };
    if let Err(error) = result {
        render_error(&error, command, output);
        std::process::exit(error.exit_code());
    }
}

fn output_from_raw_args() -> OutputFormat {
    let args: Vec<String> = std::env::args().collect();
    for (index, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--output=") {
            return match value {
                "json" => OutputFormat::Json,
                "jsonl" => OutputFormat::Jsonl,
                _ => OutputFormat::Human,
            };
        }
        if arg == "--output" {
            return match args.get(index + 1).map(String::as_str) {
                Some("json") => OutputFormat::Json,
                Some("jsonl") => OutputFormat::Jsonl,
                _ => OutputFormat::Human,
            };
        }
    }
    OutputFormat::Human
}
