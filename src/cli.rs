use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use serde::Serialize;

use handshaker_rust::{
    ClientOptions, ConnectionTarget, DeleteOptions, Error, HandShakerClient, RemoteFile, Result,
    TransferOptions, TransferProgress,
    i18n::{self, Localizer, MessageKey, ZhCn},
};

use crate::output::{Outcome, progress_percent, render, render_progress};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Human,
    Json,
    Jsonl,
}

#[derive(Debug, Parser)]
#[command(name = "handshaker", version)]
pub(crate) struct Cli {
    #[arg(long, global = true)]
    pub serial: Option<String>,

    #[arg(long, global = true, conflicts_with = "serial", value_parser = parse_socket_addr)]
    pub wifi: Option<SocketAddr>,

    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub output: OutputFormat,

    #[arg(long, global = true, default_value = "30s", value_parser = parse_duration)]
    pub timeout: Duration,

    #[arg(long, global = true)]
    pub yes: bool,

    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[arg(long, global = true)]
    pub wire_log: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub(crate) fn try_parse_localized() -> std::result::Result<Self, clap::Error> {
        Self::try_parse_localized_from(env::args_os())
    }

    pub(crate) fn try_parse_localized_from<I, T>(
        arguments: I,
    ) -> std::result::Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut command = localized_command();
        let mut matches = command.try_get_matches_from_mut(arguments)?;
        Self::from_arg_matches_mut(&mut matches)
    }
}

fn localized_command() -> clap::Command {
    let mut command = Cli::command()
        .about(i18n::text("cli.about"))
        .help_template(i18n::text("cli.help_template"));
    command = localize_arguments(command);
    command = add_localized_help(command, true);

    localize_subcommand(&mut command, "device", "cli.command.device");
    localize_subcommand(&mut command, "fs", "cli.command.fs");
    localize_subcommand(&mut command, "clipboard", "cli.command.clipboard");
    localize_subcommand(&mut command, "trust", "cli.command.trust");
    localize_subcommand(&mut command, "shell", "cli.command.shell");

    if let Some(device) = command.find_subcommand_mut("device") {
        localize_subcommand(device, "list", "cli.command.list");
        localize_subcommand(device, "info", "cli.command.info");
        localize_subcommand(device, "ping", "cli.command.ping");
        localize_subcommand(device, "discover", "cli.command.discover");
    }
    if let Some(fs) = command.find_subcommand_mut("fs") {
        for (name, key) in [
            ("ls", "cli.command.ls"),
            ("stat", "cli.command.stat"),
            ("count", "cli.command.count"),
            ("exists", "cli.command.exists"),
            ("mkdir", "cli.command.mkdir"),
            ("mv", "cli.command.mv"),
            ("rm", "cli.command.rm"),
            ("pull", "cli.command.pull"),
            ("push", "cli.command.push"),
        ] {
            localize_subcommand(fs, name, key);
        }
    }
    if let Some(clipboard) = command.find_subcommand_mut("clipboard") {
        localize_subcommand(clipboard, "get", "cli.command.get");
        localize_subcommand(clipboard, "set", "cli.command.set");
        localize_subcommand(clipboard, "delete", "cli.command.delete");
        localize_subcommand(clipboard, "clear", "cli.command.clear");
    }
    if let Some(trust) = command.find_subcommand_mut("trust") {
        localize_subcommand(trust, "list", "cli.command.trust_list");
        localize_subcommand(trust, "remove", "cli.command.trust_remove");
        localize_subcommand(trust, "reset", "cli.command.trust_reset");
    }
    command
}

fn localize_subcommand(command: &mut clap::Command, name: &str, key: &str) {
    if let Some(subcommand) = command.find_subcommand_mut(name) {
        let mut localized = subcommand.clone().about(i18n::text(key));
        let template = if localized.get_subcommands().next().is_some() {
            "cli.help_template"
        } else if localized.get_positionals().next().is_some() {
            "cli.help_template_leaf"
        } else {
            "cli.help_template_options"
        };
        localized = localized.help_template(i18n::text(template));
        localized = add_localized_help(localized, false);
        localized = localize_arguments(localized);
        localized = localize_command_arguments(localized, name);
        *subcommand = localized;
    }
}

fn localize_arguments(mut command: clap::Command) -> clap::Command {
    if has_argument(&command, "serial") {
        command = command.mut_arg("serial", |arg| {
            arg.help(i18n::text("cli.serial"))
                .value_name(i18n::text("cli.value.serial"))
        });
    }
    if has_argument(&command, "wifi") {
        command = command.mut_arg("wifi", |arg| {
            arg.help(i18n::text("cli.wifi"))
                .value_name(i18n::text("cli.value.wifi"))
        });
    }
    if has_argument(&command, "output") {
        command = command.mut_arg("output", |arg| {
            arg.help(i18n::text("cli.output"))
                .value_name(i18n::text("cli.value.output"))
                .hide_default_value(true)
                .hide_possible_values(true)
        });
    }
    if has_argument(&command, "timeout") {
        command = command.mut_arg("timeout", |arg| {
            arg.help(i18n::text("cli.timeout"))
                .value_name(i18n::text("cli.value.timeout"))
                .hide_default_value(true)
        });
    }
    if has_argument(&command, "discover_timeout") {
        command = command.mut_arg("discover_timeout", |arg| {
            arg.help(i18n::text("cli.arg.browse_timeout"))
                .value_name(i18n::text("cli.value.browse_timeout"))
                .hide_default_value(true)
        });
    }
    if has_argument(&command, "yes") {
        command = command.mut_arg("yes", |arg| arg.help(i18n::text("cli.yes")));
    }
    if has_argument(&command, "verbose") {
        command = command.mut_arg("verbose", |arg| arg.help(i18n::text("cli.verbose")));
    }
    if has_argument(&command, "wire_log") {
        command = command.mut_arg("wire_log", |arg| {
            arg.help(i18n::text("cli.wire_log"))
                .value_name(i18n::text("cli.value.wire_log"))
        });
    }
    command
}

fn add_localized_help(mut command: clap::Command, include_version: bool) -> clap::Command {
    command = command
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .help(i18n::text("cli.help"))
                .action(ArgAction::Help),
        );
    if include_version {
        command = command.disable_version_flag(true).arg(
            clap::Arg::new("version")
                .short('V')
                .long("version")
                .help(i18n::text("cli.version"))
                .action(ArgAction::Version),
        );
    }
    command
}

fn has_argument(command: &clap::Command, id: &str) -> bool {
    command
        .get_arguments()
        .any(|argument| argument.get_id() == id)
}

fn localize_command_arguments(mut command: clap::Command, name: &str) -> clap::Command {
    let arguments: &[(&str, &str)] = match name {
        "ls" => &[("path", "cli.arg.path"), ("depth", "cli.arg.depth")],
        "stat" | "exists" | "mkdir" => &[("path", "cli.arg.path")],
        "count" => &[
            ("path", "cli.arg.path"),
            ("depth", "cli.arg.depth"),
            ("exclusions", "cli.arg.exclusions"),
        ],
        "mv" => &[("source", "cli.arg.source"), ("target", "cli.arg.target")],
        "rm" => &[
            ("paths", "cli.arg.paths"),
            ("recursive", "cli.arg.recursive"),
            ("trash", "cli.arg.trash"),
        ],
        "pull" => &[
            ("remote", "cli.arg.remote"),
            ("local", "cli.arg.local"),
            ("overwrite", "cli.arg.overwrite"),
        ],
        "push" => &[
            ("local", "cli.arg.local"),
            ("remote", "cli.arg.remote"),
            ("overwrite", "cli.arg.overwrite"),
        ],
        "set" => &[("text", "cli.arg.text"), ("stdin", "cli.arg.stdin")],
        "delete" => &[("timestamp", "cli.arg.timestamp")],
        "remove" | "reset" => &[("device_uuid", "cli.arg.device_uuid")],
        _ => &[],
    };
    for (id, key) in arguments {
        if has_argument(&command, id) {
            command = command.mut_arg(*id, |arg| {
                let arg = arg.help(i18n::text(key));
                let arg = match *id {
                    "path" | "depth" | "exclusions" | "source" | "target" | "paths" | "remote"
                    | "local" | "text" | "timestamp" | "device_uuid" => {
                        arg.value_name(i18n::text(key))
                    }
                    _ => arg,
                };
                if *id == "depth" {
                    arg.hide_default_value(true)
                } else {
                    arg
                }
            });
        }
    }
    command
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(subcommand)]
    Device(DeviceCommand),
    #[command(subcommand)]
    Fs(FsCommand),
    #[command(subcommand)]
    Clipboard(ClipboardCommand),
    #[command(subcommand)]
    Trust(TrustCommand),
    Shell,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeviceCommand {
    List,
    Info,
    Ping,
    Discover {
        /// mDNS browse window duration, e.g. "6s".
        #[arg(long = "browse-timeout", id = "discover_timeout", default_value = "6s")]
        timeout: String,
    },
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
        #[arg(index = 1)]
        remote: String,
        #[arg(index = 2)]
        local: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    Push {
        #[arg(index = 1)]
        local: PathBuf,
        #[arg(index = 2)]
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

#[derive(Debug, Subcommand)]
pub(crate) enum TrustCommand {
    List,
    Remove { device_uuid: String },
    Reset { device_uuid: String },
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
        Command::Device(DeviceCommand::Discover { .. }) => "device.discover",
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
        Command::Trust(TrustCommand::List) => "trust.list",
        Command::Trust(TrustCommand::Remove { .. }) => "trust.remove",
        Command::Trust(TrustCommand::Reset { .. }) => "trust.reset",
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
    if matches!(cli.command, Command::Device(DeviceCommand::Discover { .. })) {
        let outcome = device_discover(&cli).await?;
        return render(&outcome, cli.output);
    }
    if matches!(cli.command, Command::Trust(TrustCommand::List)) {
        let outcome = trust_list().await?;
        return render(&outcome, cli.output);
    }
    if matches!(cli.command, Command::Trust(TrustCommand::Remove { .. })) {
        let outcome = trust_remove(&cli).await?;
        return render(&outcome, cli.output);
    }
    if matches!(cli.command, Command::Trust(TrustCommand::Reset { .. })) {
        let outcome = trust_reset(&cli).await?;
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
    if cli.wifi.is_some() {
        // First connects and resets require acting on the phone; give a hint
        // before the handshake blocks waiting for the trust dialog.
        eprintln!("{}", ZhCn.text(MessageKey::WifiTrustHint));
    }
    let target = match cli.wifi {
        Some(address) => ConnectionTarget::Wifi { address },
        None => ConnectionTarget::Adb {
            serial: cli.serial.clone(),
        },
    };
    HandShakerClient::connect(
        target,
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

async fn device_discover(cli: &Cli) -> Result<Outcome> {
    let timeout = match &cli.command {
        Command::Device(DeviceCommand::Discover { timeout }) => {
            parse_duration(timeout).map_err(|message| Error::Usage(message))?
        }
        _ => unreachable!("device_discover only runs for Device::Discover"),
    };
    let devices = HandShakerClient::discover_wifi_devices(timeout).await?;
    let localizer = ZhCn;
    let human = if devices.is_empty() {
        localizer.text(MessageKey::NoWifiDevices).to_string()
    } else {
        let mut lines = vec![localizer.text(MessageKey::WifiDeviceListHeader).to_string()];
        lines.extend(devices.iter().map(|device| {
            let address = device.addresses.first().map(String::as_str).unwrap_or("-");
            format!(
                "{}\t{}\t{}\t{}",
                device.instance, address, device.port, device.host
            )
        }));
        lines.join("\n")
    };
    Outcome::new("device.discover", devices, human)
}

fn human_trust_records(records: &[handshaker_rust::TrustRecordInfo]) -> String {
    let localizer = ZhCn;
    if records.is_empty() {
        return localizer.text(MessageKey::TrustNone).to_string();
    }
    let mut lines = vec![localizer.text(MessageKey::TrustListHeader).to_string()];
    lines.extend(records.iter().map(|record| {
        format!(
            "{}\t{}",
            record.device_uuid,
            record.device_name.as_deref().unwrap_or("-")
        )
    }));
    lines.join("\n")
}

async fn trust_list() -> Result<Outcome> {
    let records = HandShakerClient::list_trusted_devices().await?;
    Outcome::new("trust.list", records.clone(), human_trust_records(&records))
}

async fn trust_remove(cli: &Cli) -> Result<Outcome> {
    let device_uuid = match &cli.command {
        Command::Trust(TrustCommand::Remove { device_uuid }) => device_uuid,
        _ => unreachable!("trust_remove only runs for Trust::Remove"),
    };
    confirm(
        &ZhCn.format(MessageKey::TrustRemoveAction, &[device_uuid]),
        cli.yes,
        cli.output,
    )?;
    let removed = HandShakerClient::remove_trusted_device(device_uuid).await?;
    if !removed {
        return Err(Error::Configuration(
            ZhCn.format(MessageKey::TrustMissing, &[device_uuid]),
        ));
    }
    Outcome::new(
        "trust.remove",
        serde_json::json!({ "device_uuid": device_uuid }),
        ZhCn.format(MessageKey::TrustRemoved, &[device_uuid]),
    )
}

async fn trust_reset(cli: &Cli) -> Result<Outcome> {
    let device_uuid = match &cli.command {
        Command::Trust(TrustCommand::Reset { device_uuid }) => device_uuid,
        _ => unreachable!("trust_reset only runs for Trust::Reset"),
    };
    let address = cli
        .wifi
        .ok_or_else(|| Error::Usage(ZhCn.text(MessageKey::TrustResetNeedsWifi).to_string()))?;
    confirm(
        &ZhCn.format(MessageKey::TrustResetAction, &[device_uuid]),
        cli.yes,
        cli.output,
    )?;
    HandShakerClient::reset_wifi_trust(address, device_uuid, client_options(cli)).await?;
    Outcome::new(
        "trust.reset",
        serde_json::json!({ "device_uuid": device_uuid, "address": address.to_string() }),
        ZhCn.format(MessageKey::TrustResetDone, &[device_uuid]),
    )
}

fn client_options(cli: &Cli) -> ClientOptions {
    ClientOptions {
        timeout: cli.timeout,
        wire_log: cli.wire_log.clone(),
        ..Default::default()
    }
}

fn parse_socket_addr(value: &str) -> std::result::Result<SocketAddr, String> {
    value
        .parse::<SocketAddr>()
        .map_err(|error| i18n::format("cli.arg.wifi_invalid", &[value, &error.to_string()]))
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
        Command::Device(DeviceCommand::Discover { .. }) => {
            unreachable!("device.discover is handled before connecting");
        }
        Command::Trust(_) => {
            unreachable!("trust commands are handled before connecting");
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
    let mut editor = DefaultEditor::new().map_err(|error| {
        Error::LocalIo(i18n::format(
            "shell.readline_init_failed",
            &[&error.to_string()],
        ))
    })?;
    println!("{}", localizer.text(MessageKey::ShellWelcome));
    let mut context = CommandContext {
        remote_cwd: client.root_path().to_string(),
        local_cwd: env::current_dir()?,
        in_shell: true,
    };
    let mut command_error = None;
    loop {
        let prompt = i18n::format(
            "shell.prompt",
            &[&client.device_info().serial, &context.remote_cwd],
        );
        let line = match editor.readline(&prompt) {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => {
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(error) => {
                return Err(Error::LocalIo(i18n::format(
                    "shell.readline_failed",
                    &[&error.to_string()],
                )));
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        editor.add_history_entry(line).map_err(|error| {
            Error::LocalIo(i18n::format("shell.history_failed", &[&error.to_string()]))
        })?;
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

        let argv = shell_command_argv(words);
        let parsed = match Cli::try_parse_localized_from(argv) {
            Ok(parsed) => parsed,
            Err(error)
                if matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
            {
                let _ = error.print();
                continue;
            }
            Err(_) => {
                eprintln!(
                    "{}",
                    localizer.format(MessageKey::Error, &[i18n::text("cli.parse_error")],)
                );
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
                signal.map_err(|error| {
                    Error::LocalIo(i18n::format("error.ctrl_c", &[&error.to_string()]))
                })?;
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

fn shell_command_argv(words: Vec<String>) -> Vec<String> {
    let mut argv = vec!["handshaker".to_string()];
    if words.first().map(String::as_str) == Some("ls") {
        argv.push("fs".to_string());
    }
    argv.extend(words);
    argv
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
    print!("{}", i18n::format("confirm.prompt_with_action", &[action]));
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
    fn localized_command_tree_parses_leaf_commands() {
        let cli = Cli::try_parse_localized_from(["handshaker", "fs", "ls", "/sdcard"])
            .expect("localized parse");
        assert!(matches!(cli.command, Command::Fs(FsCommand::Ls { .. })));
    }

    #[test]
    fn command_tree_parses_every_v01_leaf_command() {
        let commands = [
            vec!["device", "list"],
            vec!["device", "info"],
            vec!["device", "ping"],
            vec!["fs", "ls"],
            vec!["fs", "stat", "/sdcard/a"],
            vec!["fs", "count", "/sdcard"],
            vec!["fs", "exists", "/sdcard/a"],
            vec!["fs", "mkdir", "/sdcard/a"],
            vec!["fs", "mv", "/sdcard/a", "/sdcard/b"],
            vec!["fs", "rm", "/sdcard/a", "--recursive"],
            vec!["fs", "pull", "/sdcard/a", "/tmp/a"],
            vec!["fs", "push", "/tmp/a", "/sdcard/a"],
            vec!["clipboard", "get"],
            vec!["clipboard", "set", "text"],
            vec!["clipboard", "set", "--stdin"],
            vec!["clipboard", "delete", "42"],
            vec!["clipboard", "clear"],
            vec!["shell"],
        ];
        for command in commands {
            let mut argv = vec!["handshaker"];
            argv.extend(command);
            Cli::try_parse_localized_from(argv).expect("v0.1 command should parse");
        }
    }

    #[test]
    fn localized_leaf_help_uses_display_help() {
        let error = Cli::try_parse_localized_from(["handshaker", "fs", "ls", "--help"])
            .expect_err("help should stop parsing");
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp, "{error}");
    }

    #[test]
    fn shell_ls_alias_expands_to_fs_ls() {
        let argv = shell_command_argv(vec!["ls".into(), "--depth".into(), "2".into()]);
        assert_eq!(argv, vec!["handshaker", "fs", "ls", "--depth", "2"]);
        let parsed = Cli::try_parse_localized_from(argv).expect("parse shell ls alias");
        assert!(matches!(
            parsed.command,
            Command::Fs(FsCommand::Ls { depth: 2, .. })
        ));
    }

    #[test]
    fn json_dangerous_operation_requires_yes() {
        let error =
            confirm("dangerous operation", false, OutputFormat::Json).expect_err("confirmation");
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

    #[test]
    fn push_uses_documented_local_then_remote_order() {
        let cli = Cli::try_parse_from([
            "handshaker",
            "fs",
            "push",
            "/tmp/local.txt",
            "/sdcard/remote.txt",
        ])
        .expect("parse push");
        match cli.command {
            Command::Fs(FsCommand::Push { local, remote, .. }) => {
                assert_eq!(local, PathBuf::from("/tmp/local.txt"));
                assert_eq!(remote, "/sdcard/remote.txt");
            }
            _ => panic!("expected fs push"),
        }
    }
}
