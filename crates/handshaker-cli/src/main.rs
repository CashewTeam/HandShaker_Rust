mod cli;
mod output;

use clap::error::ErrorKind;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, OutputFormat, command_name};
use crate::output::render_error;

/// Synchronous entry point: create the tokio runtime explicitly and
/// `block_on`, so that returning `ExitCode` drops the runtime normally
/// (all tasks aborted, transport `Drop` cleanup runs, a Ctrl-C'd session
/// never leaks its adb forward).
fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start async runtime: {error}");
            return std::process::ExitCode::from(4);
        }
    };
    runtime.block_on(cli_main())
}

async fn cli_main() -> std::process::ExitCode {
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
            return std::process::ExitCode::SUCCESS;
        }
        Err(_error) => {
            let app_error = handshaker_core::Error::Usage(
                handshaker_core::i18n::text("cli.parse_error").to_string(),
            );
            render_error(&app_error, "unknown", fallback_output);
            return std::process::ExitCode::from(app_error.exit_code() as u8);
        }
    };
    let command = command_name(&cli.command);
    let output = cli.output;
    let is_shell = matches!(cli.command, cli::Command::Shell);
    let is_watch = matches!(cli.command, cli::Command::Watch { .. });
    // `sync run` and `sync watch` drive their own Ctrl-C handling (stopping
    // the job/watch and cleaning the session up); the top-level select must
    // not race them, or SIGINT would exit the process before close_session
    // releases the adb forward (Phase D device finding). Other sync commands
    // (plan/status) keep the top-level interrupt.
    let is_self_handling_ctrl_c = is_shell
        || is_watch
        || matches!(
            cli.command,
            cli::Command::Sync(cli::SyncCommand::Run { .. })
                | cli::Command::Sync(cli::SyncCommand::Watch { .. })
        );
    let filter = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .init();

    // The shell drives its own readline loop and `watch`/`sync` handle
    // Ctrl-C themselves (to unregister monitors and stop jobs); every other
    // command lets the top-level select turn Ctrl-C into a user interrupt.
    let result = if is_self_handling_ctrl_c {
        cli::run(cli).await
    } else {
        tokio::select! {
            result = cli::run(cli) => result,
            signal = tokio::signal::ctrl_c() => match signal {
                Ok(()) => Err(handshaker_core::Error::Interrupted),
                Err(error) => Err(handshaker_core::Error::LocalIo(
                    handshaker_core::i18n::format("error.ctrl_c", &[&error.to_string()]),
                )),
            },
        }
    };
    if let Err(error) = result {
        render_error(&error, command, output);
        // Returning `ExitCode` (instead of `process::exit`) lets every
        // value on the stack — including the tokio runtime — drop normally,
        // so a Ctrl-C'd session still runs its transport cleanup and
        // releases the adb forward (Phase D device finding).
        return std::process::ExitCode::from(error.exit_code() as u8);
    }
    std::process::ExitCode::SUCCESS
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
