use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use handshaker_rust::{
    ClientOptions, ConnectionTarget, DeleteOptions, Error, HandShakerClient, RemoteFile, Result,
    TransferOptions, TransferProgress,
};

use crate::messages::{Localizer, MessageKey, ZhCn};
use crate::output::{Outcome, progress_percent, render, render_progress};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Human,
    Json,
    Jsonl,
}

#[derive(Debug, Parser)]
#[command(
    name = "handshaker",
    version,
    about = "兼容 Smartisan HandShaker 的命令行客户端"
)]
pub(crate) struct Cli {
    #[arg(long, global = true, help = "指定 ADB 设备序列号")]
    pub serial: Option<String>,

    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub output: OutputFormat,

    #[arg(long, global = true, default_value = "30s", value_parser = parse_duration)]
    pub timeout: Duration,

    #[arg(long, global = true, help = "确认危险操作")]
    pub yes: bool,

    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[arg(long, global = true, help = "记录完整 SSP 字节流（可能包含敏感内容）")]
    pub wire_log: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(subcommand)]
    Device(DeviceCommand),
    #[command(subcommand)]
    Fs(FsCommand),
    #[command(subcommand)]
    Clipboard(ClipboardCommand),
    Shell,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeviceCommand {
    List,
    Info,
    Ping,
}

#[derive(Debug, Subcommand)]
pub(crate) enum FsCommand {
    Ls {
        path: Option<String>,
        #[arg(long, default_value_t = 1)]
        depth: u32,
    },
    Stat {
        path: String,
    },
    Count {
        path: String,
        #[arg(long, default_value_t = 1)]
        depth: u32,
        #[arg(long = "exclude")]
        exclusions: Vec<String>,
    },
    Exists {
        path: String,
    },
    Mkdir {
        path: String,
    },
    Mv {
        source: String,
        target: String,
    },
    Rm {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(short = 'r', long)]
        recursive: bool,
        #[arg(long)]
        trash: bool,
    },
    Pull {
        remote: String,
        local: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    Push {
        local: PathBuf,
        remote: String,
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClipboardCommand {
    Get,
    Set(ClipboardSetArgs),
    Delete { timestamp: i64 },
    Clear,
}

#[derive(Debug, Args)]
pub(crate) struct ClipboardSetArgs {
    #[arg(conflicts_with = "stdin")]
    text: Option<String>,
    #[arg(long, conflicts_with = "text")]
    stdin: bool,
}

struct CommandContext {
    remote_cwd: String,
    local_cwd: PathBuf,
    in_shell: bool,
}

#[derive(Debug, Serialize)]
struct TransferResult {
    source: String,
    target: String,
    bytes: u64,
}

pub(crate) fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Device(DeviceCommand::List) => "device.list",
        Command::Device(DeviceCommand::Info) => "device.info",
        Command::Device(DeviceCommand::Ping) => "device.ping",
        Command::Fs(FsCommand::Ls { .. }) => "fs.ls",
        Command::Fs(FsCommand::Stat { .. }) => "fs.stat",
        Command::Fs(FsCommand::Count { .. }) => "fs.count",
        Command::Fs(FsCommand::Exists { .. }) => "fs.exists",
        Command::Fs(FsCommand::Mkdir { .. }) => "fs.mkdir",
        Command::Fs(FsCommand::Mv { .. }) => "fs.mv",
        Command::Fs(FsCommand::Rm { .. }) => "fs.rm",
        Command::Fs(FsCommand::Pull { .. }) => "fs.pull",
        Command::Fs(FsCommand::Push { .. }) => "fs.push",
        Command::Clipboard(ClipboardCommand::Get) => "clipboard.get",
        Command::Clipboard(ClipboardCommand::Set(_)) => "clipboard.set",
        Command::Clipboard(ClipboardCommand::Delete { .. }) => "clipboard.delete",
        Command::Clipboard(ClipboardCommand::Clear) => "clipboard.clear",
        Command::Shell => "shell",
    }
}

pub(crate) async fn run(cli: Cli) -> Result<()> {
    let localizer = ZhCn;
    if cli.wire_log.is_some() {
        eprintln!("{}", localizer.text(MessageKey::WireLogWarning));
    }
    if matches!(cli.command, Command::Device(DeviceCommand::List)) {
        let outcome = device_list(cli.timeout).await?;
        return render(&outcome, cli.output);
    }
    if matches!(cli.command, Command::Shell) {
        if cli.output != OutputFormat::Human {
            return Err(Error::Usage(
                localizer.text(MessageKey::ShellOnlyHuman).to_string(),
            ));
        }
        return run_shell(&cli).await;
    }

    let client = connect(&cli).await?;
    let context = CommandContext {
        remote_cwd: client.root_path().to_string(),
        local_cwd: env::current_dir()?,
        in_shell: false,
    };
    let command = command_name(&cli.command);
    let outcome = execute_connected(&cli.command, &client, &context, cli.yes, cli.output).await;
    let close = client.close().await;
    match outcome {
        Ok(outcome) => {
            close?;
            debug_assert_eq!(outcome.command, command);
            render(&outcome, cli.output)
        }
        Err(error) => Err(error),
    }
}

async fn connect(cli: &Cli) -> Result<HandShakerClient> {
    HandShakerClient::connect(
        ConnectionTarget::Adb {
            serial: cli.serial.clone(),
        },
        ClientOptions {
            timeout: cli.timeout,
            wire_log: cli.wire_log.clone(),
            ..Default::default()
        },
    )
    .await
}

async fn device_list(timeout: Duration) -> Result<Outcome> {
    let devices = HandShakerClient::list_adb_devices_with_timeout("adb", timeout).await?;
    let localizer = ZhCn;
    let human = if devices.is_empty() {
        localizer.text(MessageKey::NoDevices).to_string()
    } else {
        let mut lines = vec![localizer.text(MessageKey::DeviceListHeader).to_string()];
        lines.extend(devices.iter().map(|device| {
            format!(
                "{}\t{}\t{}\t{}",
                device.serial,
                device.state,
                device.model.as_deref().unwrap_or("-"),
                device.device.as_deref().unwrap_or("-")
            )
        }));
        lines.join("\n")
    };
    Outcome::new("device.list", devices, human)
}

async fn execute_connected(
    command: &Command,
    client: &HandShakerClient,
    context: &CommandContext,
    yes: bool,
    format: OutputFormat,
) -> Result<Outcome> {
    let localizer = ZhCn;
    let outcome = match command {
        Command::Device(DeviceCommand::List) => {
            return device_list(Duration::from_secs(30)).await;
        }
        Command::Device(DeviceCommand::Info) => Outcome::new(
            "device.info",
            client.device_info(),
            human_device_info(client.device_info()),
        )?,
        Command::Device(DeviceCommand::Ping) => {
            let ping = client.ping().await?;
            Outcome::new(
                "device.ping",
                &ping,
                localizer.format(MessageKey::PingResult, &[&ping.round_trip_ms.to_string()]),
            )?
        }
        Command::Fs(FsCommand::Ls { path, depth }) => {
            let path = resolve_remote(&context.remote_cwd, path.as_deref().unwrap_or("."));
            let files = client.list_dir(&path, *depth).await?;
            Outcome::new("fs.ls", &files, human_files(&files))?
        }
        Command::Fs(FsCommand::Stat { path }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let file = client.stat(&path).await?.ok_or_else(|| Error::RemoteIo {
                code: None,
                message: localizer.format(MessageKey::RemoteMissing, &[&path]),
            })?;
            Outcome::new("fs.stat", &file, human_file(&file))?
        }
        Command::Fs(FsCommand::Count {
            path,
            depth,
            exclusions,
        }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let count = client.file_count(&path, *depth, exclusions.clone()).await?;
            Outcome::new(
                "fs.count",
                serde_json::json!({ "path": path, "count": count }),
                localizer.format(MessageKey::FileCount, &[&count.to_string()]),
            )?
        }
        Command::Fs(FsCommand::Exists { path }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let exists = client.file_exists(&path).await?;
            Outcome::new(
                "fs.exists",
                serde_json::json!({ "path": path, "exists": exists }),
                localizer
                    .text(if exists {
                        MessageKey::Exists
                    } else {
                        MessageKey::Missing
                    })
                    .to_string(),
            )?
        }
        Command::Fs(FsCommand::Mkdir { path }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let file = client.create_dir(&path).await?;
            Outcome::new(
                "fs.mkdir",
                &file,
                localizer.format(MessageKey::DirectoryCreated, &[&file.path]),
            )?
        }
        Command::Fs(FsCommand::Mv { source, target }) => {
            let source = resolve_remote(&context.remote_cwd, source);
            let target = resolve_remote(&context.remote_cwd, target);
            client.rename(&source, &target).await?;
            Outcome::new(
                "fs.mv",
                serde_json::json!({ "source": source, "target": target }),
                localizer.text(MessageKey::RenameDone).to_string(),
            )?
        }
        Command::Fs(FsCommand::Rm {
            paths,
            recursive,
            trash,
        }) => {
            let paths: Vec<_> = paths
                .iter()
                .map(|path| resolve_remote(&context.remote_cwd, path))
                .collect();
            for path in &paths {
                let file = client.stat(path).await?.ok_or_else(|| Error::RemoteIo {
                    code: None,
                    message: localizer.format(MessageKey::RemoteMissing, &[path]),
                })?;
                if file.is_directory && !recursive {
                    return Err(Error::Usage(
                        localizer.format(MessageKey::DeleteRecursiveRequired, &[path]),
                    ));
                }
            }
            confirm(
                &localizer.format(MessageKey::DeleteAction, &[&paths.len().to_string()]),
                yes,
                format,
            )?;
            let deleted = client
                .delete(
                    &paths,
                    DeleteOptions {
                        trash: *trash,
                        sync: false,
                    },
                )
                .await?;
            Outcome::new(
                "fs.rm",
                &deleted,
                localizer.format(MessageKey::DeletedCount, &[&deleted.len().to_string()]),
            )?
        }
        Command::Fs(FsCommand::Pull {
            remote,
            local,
            overwrite,
        }) => {
            let remote = resolve_remote(&context.remote_cwd, remote);
            let local = resolve_local_pull(&context.local_cwd, local.as_deref(), &remote)?;
            if local.exists() {
                if !overwrite {
                    return Err(Error::LocalIo(localizer.format(
                        MessageKey::LocalTargetExists,
                        &[&local.display().to_string()],
                    )));
                }
                confirm(
                    &localizer.format(
                        MessageKey::OverwriteLocalAction,
                        &[&local.display().to_string()],
                    ),
                    yes,
                    format,
                )?;
            }
            let bytes = client
                .download(
                    &remote,
                    &local,
                    transfer_options(*overwrite, "fs.pull", client.device_info(), format),
                )
                .await?;
            let result = TransferResult {
                source: remote,
                target: local.display().to_string(),
                bytes,
            };
            Outcome::new(
                "fs.pull",
                &result,
                localizer.format(MessageKey::DownloadDone, &[&result.bytes.to_string()]),
            )?
        }
        Command::Fs(FsCommand::Push {
            local,
            remote,
            overwrite,
        }) => {
            let local = resolve_local(&context.local_cwd, local);
            let remote = resolve_remote(&context.remote_cwd, remote);
            if client.file_exists(&remote).await? {
                if !overwrite {
                    return Err(Error::RemoteIo {
                        code: None,
                        message: localizer.format(MessageKey::RemoteTargetExists, &[&remote]),
                    });
                }
                confirm(
                    &localizer.format(MessageKey::OverwriteRemoteAction, &[&remote]),
                    yes,
                    format,
                )?;
            }
            let bytes = client
                .upload(
                    &local,
                    &remote,
                    transfer_options(*overwrite, "fs.push", client.device_info(), format),
                )
                .await?;
            let result = TransferResult {
                source: local.display().to_string(),
                target: remote,
                bytes,
            };
            Outcome::new(
                "fs.push",
                &result,
                localizer.format(MessageKey::UploadDone, &[&result.bytes.to_string()]),
            )?
        }
        Command::Clipboard(ClipboardCommand::Get) => {
            let entries = client.clipboard_list().await?;
            Outcome::new("clipboard.get", &entries, human_clipboards(&entries))?
        }
        Command::Clipboard(ClipboardCommand::Set(args)) => {
            let text = if args.stdin {
                if context.in_shell {
                    return Err(Error::Usage(
                        localizer.text(MessageKey::ShellNoStdin).to_string(),
                    ));
                }
                let mut text = String::new();
                io::stdin().read_to_string(&mut text)?;
                text
            } else {
                args.text.clone().ok_or_else(|| {
                    Error::Usage(localizer.text(MessageKey::ClipboardSetRequired).to_string())
                })?
            };
            client.clipboard_set(&text).await?;
            Outcome::new(
                "clipboard.set",
                serde_json::json!({ "bytes": text.len() }),
                localizer.text(MessageKey::ClipboardWritten).to_string(),
            )?
        }
        Command::Clipboard(ClipboardCommand::Delete { timestamp }) => {
            confirm(
                &localizer.format(MessageKey::DeleteClipboardAction, &[&timestamp.to_string()]),
                yes,
                format,
            )?;
            client.clipboard_delete(*timestamp).await?;
            Outcome::new(
                "clipboard.delete",
                serde_json::json!({ "timestamp": timestamp }),
                localizer.text(MessageKey::ClipboardDeleted).to_string(),
            )?
        }
        Command::Clipboard(ClipboardCommand::Clear) => {
            confirm(
                localizer.text(MessageKey::ClearClipboardAction),
                yes,
                format,
            )?;
            client.clipboard_clear().await?;
            Outcome::new(
                "clipboard.clear",
                serde_json::json!({}),
                localizer.text(MessageKey::ClipboardCleared).to_string(),
            )?
        }
        Command::Shell => {
            return Err(Error::Usage(
                localizer.text(MessageKey::ShellNested).to_string(),
            ));
        }
    };
    Ok(outcome.with_device(client.device_info()))
}

async fn run_shell(cli: &Cli) -> Result<()> {
    let client = connect(cli).await?;
    let localizer = ZhCn;
    println!("{}", localizer.text(MessageKey::ShellWelcome));
    let mut context = CommandContext {
        remote_cwd: client.root_path().to_string(),
        local_cwd: env::current_dir()?,
        in_shell: true,
    };
    let mut command_error = None;
    loop {
        print!(
            "handshaker({}) {}> ",
            client.device_info().serial,
            context.remote_cwd
        );
        io::stdout().flush()?;
        let mut line = String::new();
        let read = match io::stdin().read_line(&mut line) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                println!();
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            println!();
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let words = match shell_words::split(line) {
            Ok(words) => words,
            Err(error) => {
                eprintln!(
                    "{}",
                    localizer.format(MessageKey::CommandParseError, &[&error.to_string()])
                );
                continue;
            }
        };
        match words.first().map(String::as_str) {
            Some("exit" | "quit") => break,
            Some("help") if words.len() == 1 => {
                println!("{}", localizer.text(MessageKey::ShellHelp));
                continue;
            }
            Some("pwd") if words.len() == 1 => {
                println!("{}", context.remote_cwd);
                continue;
            }
            Some("lpwd") if words.len() == 1 => {
                println!("{}", context.local_cwd.display());
                continue;
            }
            Some("cd") if words.len() == 2 => {
                let path = resolve_remote(&context.remote_cwd, &words[1]);
                match client.stat(&path).await {
                    Ok(Some(file)) if file.is_directory => context.remote_cwd = path,
                    Ok(_) => eprintln!(
                        "{}",
                        localizer.format(MessageKey::RemoteNotDirectory, &[&path])
                    ),
                    Err(error) => eprintln!(
                        "{}",
                        localizer.format(MessageKey::Error, &[&error.to_string()])
                    ),
                }
                continue;
            }
            Some("lcd") if words.len() == 2 => {
                let path = resolve_local(&context.local_cwd, Path::new(&words[1]));
                if path.is_dir() {
                    context.local_cwd = path;
                } else {
                    eprintln!(
                        "{}",
                        localizer.format(
                            MessageKey::LocalNotDirectory,
                            &[&path.display().to_string()],
                        )
                    );
                }
                continue;
            }
            _ => {}
        }

        let mut argv = vec!["handshaker".to_string()];
        argv.extend(words);
        let parsed = match Cli::try_parse_from(argv) {
            Ok(parsed) => parsed,
            Err(error) => {
                eprint!("{error}");
                continue;
            }
        };
        if matches!(parsed.command, Command::Shell) {
            eprintln!(
                "{}",
                localizer.format(
                    MessageKey::Error,
                    &[localizer.text(MessageKey::ShellNested)],
                )
            );
            continue;
        }
        let close_on_interrupt = matches!(&parsed.command, Command::Fs(FsCommand::Pull { .. }));
        let operation = tokio::select! {
            result = execute_connected(
                &parsed.command,
                &client,
                &context,
                parsed.yes,
                OutputFormat::Human,
            ) => result,
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|error| Error::LocalIo(format!("监听 Ctrl-C 失败：{error}")))?;
                if close_on_interrupt {
                    command_error = Some(Error::Interrupted);
                    break;
                }
                Err(Error::Interrupted)
            }
        };
        match operation {
            Ok(outcome) => render(&outcome, OutputFormat::Human)?,
            Err(error) => {
                eprintln!(
                    "{}",
                    localizer.format(MessageKey::Error, &[&error.to_string()])
                );
                if matches!(
                    error,
                    Error::Transport(_)
                        | Error::Handshake(_)
                        | Error::Timeout(_)
                        | Error::Protocol(_)
                ) {
                    command_error = Some(error);
                    break;
                }
            }
        }
    }
    let close = client.close().await;
    println!("{}", localizer.text(MessageKey::ShellBye));
    if let Some(error) = command_error {
        return Err(error);
    }
    close
}

fn confirm(action: &str, yes: bool, format: OutputFormat) -> Result<()> {
    if yes {
        return Ok(());
    }
    if format != OutputFormat::Human || !io::stdin().is_terminal() {
        return Err(Error::ConfirmationRequired(
            ZhCn.format(MessageKey::ConfirmationRequired, &[action]),
        ));
    }
    let localizer = ZhCn;
    print!("{action}。{}", localizer.text(MessageKey::ConfirmPrompt));
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(Error::ConfirmationRequired(
            localizer.text(MessageKey::UserNotConfirmed).to_string(),
        ))
    }
}

fn transfer_options(
    overwrite: bool,
    command: &'static str,
    device: &handshaker_rust::DeviceInfo,
    format: OutputFormat,
) -> TransferOptions {
    if format == OutputFormat::Json {
        return TransferOptions {
            overwrite,
            progress: None,
        };
    }
    let device = device.clone();
    let last_percent = Arc::new(Mutex::new(None));
    TransferOptions {
        overwrite,
        progress: Some(Arc::new(move |progress: TransferProgress| {
            let percent = progress_percent(&progress);
            let Ok(mut last) = last_percent.lock() else {
                return;
            };
            if *last == Some(percent) {
                return;
            }
            *last = Some(percent);
            render_progress(command, &device, &progress, format);
        })),
    }
}

fn resolve_remote(base: &str, input: &str) -> String {
    let mut parts: Vec<&str> = if input.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|part| !part.is_empty()).collect()
    };
    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    format!("/{}", parts.join("/"))
}

fn resolve_local(base: &Path, input: &Path) -> PathBuf {
    if input.is_absolute() {
        input.to_path_buf()
    } else {
        normalize_local(base.join(input))
    }
}

fn resolve_local_pull(base: &Path, local: Option<&Path>, remote: &str) -> Result<PathBuf> {
    if let Some(local) = local {
        return Ok(resolve_local(base, local));
    }
    let name = remote
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::Usage(ZhCn.format(MessageKey::RemoteNameMissing, &[remote])))?;
    Ok(base.join(name))
}

fn normalize_local(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            value => normalized.push(value.as_os_str()),
        }
    }
    normalized
}

fn human_device_info(info: &handshaker_rust::DeviceInfo) -> String {
    let localizer = ZhCn;
    let battery = info
        .battery_percentage
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "-".to_string());
    localizer.format(
        MessageKey::DeviceInfo,
        &[
            &info.serial,
            info.name.as_deref().unwrap_or("-"),
            info.model.as_deref().unwrap_or("-"),
            info.brand.as_deref().unwrap_or("-"),
            info.smartisan_version.as_deref().unwrap_or("-"),
            info.apk_version_name.as_deref().unwrap_or("-"),
            &info.root_path,
            &battery,
            if info.phone_locked.unwrap_or(false) {
                localizer.text(MessageKey::Yes)
            } else {
                localizer.text(MessageKey::No)
            },
        ],
    )
}

fn human_files(files: &[RemoteFile]) -> String {
    let localizer = ZhCn;
    let mut lines = vec![localizer.text(MessageKey::FileListHeader).to_string()];
    lines.extend(files.iter().map(human_file));
    lines.join("\n")
}

fn human_file(file: &RemoteFile) -> String {
    let localizer = ZhCn;
    format!(
        "{}\t{}\t{}\t{}",
        if file.is_directory {
            localizer.text(MessageKey::Directory)
        } else {
            localizer.text(MessageKey::File)
        },
        file.size,
        file.modified_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        file.path
    )
}

fn human_clipboards(entries: &[handshaker_rust::ClipboardEntry]) -> String {
    let localizer = ZhCn;
    let mut lines = vec![localizer.text(MessageKey::ClipboardHeader).to_string()];
    lines.extend(entries.iter().map(|entry| {
        format!(
            "{}\t{}",
            entry.timestamp_ms,
            entry.text.replace('\n', "\\n")
        )
    }));
    lines.join("\n")
}

fn parse_duration(value: &str) -> std::result::Result<Duration, String> {
    let (number, multiplier) = if let Some(value) = value.strip_suffix("ms") {
        (value, 1_u64)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1_000)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60_000)
    } else {
        (value, 1_000)
    };
    let amount = number
        .parse::<u64>()
        .map_err(|error| ZhCn.format(MessageKey::InvalidDuration, &[value, &error.to_string()]))?;
    Ok(Duration::from_millis(amount.saturating_mul(multiplier)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_resolve_against_current_directory() {
        assert_eq!(
            resolve_remote("/storage/emulated/0", "Download/a"),
            "/storage/emulated/0/Download/a"
        );
        assert_eq!(
            resolve_remote("/storage/emulated/0/Download", "../DCIM"),
            "/storage/emulated/0/DCIM"
        );
        assert_eq!(
            resolve_remote("/storage/emulated/0", "/system/build.prop"),
            "/system/build.prop"
        );
    }

    #[test]
    fn duration_parser_accepts_units() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
    }

    #[test]
    fn command_tree_parses_grouped_commands() {
        let cli = Cli::try_parse_from(["handshaker", "fs", "ls", "/sdcard", "--depth", "2"])
            .expect("parse");
        assert!(matches!(
            cli.command,
            Command::Fs(FsCommand::Ls { depth: 2, .. })
        ));
    }

    #[test]
    fn json_dangerous_operation_requires_yes() {
        let error = confirm("测试危险操作", false, OutputFormat::Json).expect_err("confirmation");
        assert_eq!(error.exit_code(), 8);
    }

    #[test]
    fn overwrite_and_yes_are_independent_flags() {
        let cli = Cli::try_parse_from([
            "handshaker",
            "--yes",
            "fs",
            "pull",
            "/remote/a",
            "/tmp/a",
            "--overwrite",
        ])
        .expect("parse");
        assert!(cli.yes);
        assert!(matches!(
            cli.command,
            Command::Fs(FsCommand::Pull {
                overwrite: true,
                ..
            })
        ));
    }
}
