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
    ClientEvent, ClientOptions, ConnectionTarget, DeleteOptions, DeviceInfo, Error, EventCallbacks,
    EventFilter, EventStreamError, HandShakerClient, RemoteFile, Result, TransferOptions,
    TransferProgress,
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
    localize_subcommand(&mut command, "media", "cli.command.media");
    localize_subcommand(&mut command, "shell", "cli.command.shell");
    localize_subcommand(&mut command, "watch", "cli.command.watch");

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
    if let Some(media) = command.find_subcommand_mut("media") {
        localize_subcommand(media, "photo", "cli.command.media_photo");
        localize_subcommand(media, "video", "cli.command.media_video");
        localize_subcommand(media, "audio", "cli.command.media_audio");
        localize_subcommand(media, "thumbnail", "cli.command.media_thumbnail");
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
    if has_argument(&command, "watch_path") {
        command = command.mut_arg("watch_path", |arg| {
            arg.help(i18n::text("cli.arg.watch_path"))
        });
    }
    if has_argument(&command, "media_limit") {
        command = command.mut_arg("media_limit", |arg| {
            arg.help(i18n::text("cli.arg.media_limit"))
        });
    }
    if has_argument(&command, "thumb_output_dir") {
        command = command.mut_arg("thumb_output_dir", |arg| {
            arg.help(i18n::text("cli.arg.thumb_output_dir"))
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
    #[command(subcommand)]
    Media(MediaCommand),
    Shell,
    /// Watch the connected device for events (directory monitor, clipboard,
    /// device info and other pushes). Registers monitors for each --path and
    /// streams events until interrupted or the connection closes.
    Watch {
        /// Remote directories to monitor; repeatable. Omit to only listen.
        #[arg(long = "path", id = "watch_path")]
        paths: Vec<String>,
    },
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

/// Shared preview options for media library listings.
#[derive(Debug, Args, Clone)]
pub(crate) struct MediaPreviewArgs {
    /// Show at most this many entries (default preview limit).
    #[arg(long = "limit", id = "media_limit")]
    limit: Option<usize>,
    /// Show the whole library, ignoring the preview limit.
    #[arg(long = "all")]
    all: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MediaCommand {
    /// Preview the photo library.
    Photo(MediaPreviewArgs),
    /// Preview the video library.
    Video(MediaPreviewArgs),
    /// Preview the audio library.
    Audio(MediaPreviewArgs),
    /// Fetch thumbnails for media ids or paths and write them to a directory.
    Thumbnail {
        /// Media ids or remote paths; numeric values are treated as ids.
        #[arg(index = 1, required = true)]
        targets: Vec<String>,
        /// Local directory the thumbnails are written to.
        #[arg(long = "output-dir", id = "thumb_output_dir")]
        output_dir: PathBuf,
    },
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
        Command::Media(MediaCommand::Photo(_)) => "media.photo",
        Command::Media(MediaCommand::Video(_)) => "media.video",
        Command::Media(MediaCommand::Audio(_)) => "media.audio",
        Command::Media(MediaCommand::Thumbnail { .. }) => "media.thumbnail",
        Command::Shell => "shell",
        Command::Watch { .. } => "watch",
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
    if matches!(cli.command, Command::Watch { .. }) {
        return watch(&cli).await;
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

async fn watch(cli: &Cli) -> Result<()> {
    let client = connect_with_all_callbacks(cli).await?;
    let localizer = ZhCn;
    let Command::Watch { paths } = &cli.command else {
        unreachable!("watch command");
    };
    for (index, path) in paths.iter().enumerate() {
        if let Err(error) = client.monitor_folder(path, true).await {
            // Unregister everything registered before the failing entry. The
            // phone treats duplicate unregister calls as idempotent no-ops,
            // so repeated --path values are safe to unregister twice.
            for registered in paths.iter().take(index) {
                let _ = client.monitor_folder(registered, false).await;
            }
            return Err(error);
        }
    }
    if !paths.is_empty() {
        eprintln!("{}", localizer.text(MessageKey::WatchRegistered));
    }
    let mut events = client.subscribe_events(EventFilter::all());
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    match cli.output {
                        OutputFormat::Jsonl => {
                            let envelope = watch_envelope(client.device_info(), &event);
                            println!("{envelope}");
                        }
                        _ => println!(
                            "{}",
                            sanitize_human(
                                &serde_json::to_string(&event).expect("event json")
                            )
                        ),
                    }
                    let _ = io::stdout().flush();
                }
                Err(EventStreamError::Lagged { missed }) => {
                    eprintln!(
                        "{}",
                        localizer.format(MessageKey::WatchLagged, &[&missed.to_string()])
                    );
                }
                Err(EventStreamError::Closed) => {
                    return Err(Error::Transport(
                        localizer.text(MessageKey::WatchDisconnected).to_string(),
                    ));
                }
            },
            _ = tokio::signal::ctrl_c() => {
                // Graceful stop: unregister every monitor so the phone stops
                // watching, then report the user interrupt.
                for path in paths {
                    let _ = client.monitor_folder(path, false).await;
                }
                return Err(Error::Interrupted);
            }
        }
    }
}

fn watch_envelope(info: &DeviceInfo, event: &ClientEvent) -> serde_json::Value {
    let info = info;
    serde_json::json!({
        "schema_version": 1,
        "ok": true,
        "command": "watch",
        "device": {
            "serial": info.serial,
            "name": info.name,
        },
        "event": "watch",
        "data": event,
        "warnings": [],
    })
}

async fn connect(cli: &Cli) -> Result<HandShakerClient> {
    if cli.wifi.is_some() {
        // First connects and resets require acting on the phone; give a hint
        // before the handshake blocks waiting for the trust dialog.
        eprintln!("{}", ZhCn.text(MessageKey::WifiTrustHint));
    }
    let target = connection_target(cli);
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

/// Connect for `watch`, enabling every phone-side push callback so directory,
/// clipboard, device and media events are all delivered.
async fn connect_with_all_callbacks(cli: &Cli) -> Result<HandShakerClient> {
    if cli.wifi.is_some() {
        // First connects and resets require acting on the phone; give a hint
        // before the handshake blocks waiting for the trust dialog.
        eprintln!("{}", ZhCn.text(MessageKey::WifiTrustHint));
    }
    let target = connection_target(cli);
    HandShakerClient::connect_with_event_callbacks(
        target,
        ClientOptions {
            timeout: cli.timeout,
            wire_log: cli.wire_log.clone(),
            ..Default::default()
        },
        EventCallbacks {
            device_info: true,
            photo_library: true,
            audio_library: true,
            video_library: true,
        },
    )
    .await
}

fn connection_target(cli: &Cli) -> ConnectionTarget {
    match cli.wifi {
        Some(address) => ConnectionTarget::Wifi { address },
        None => ConnectionTarget::Adb {
            serial: cli.serial.clone(),
        },
    }
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
        Command::Watch { .. } => {
            return Err(Error::Usage(
                localizer.text(MessageKey::WatchNested).to_string(),
            ));
        }
        Command::Media(command) => {
            return media_command(client, &context, command).await;
        }
    };
    Ok(outcome.with_device(client.device_info()))
}

/// Default preview cap for `media` listings: a functional preview of a large
/// media library, overridable with `--limit` or `--all`.
const DEFAULT_MEDIA_PREVIEW_LIMIT: usize = 50;

/// Strip control characters from a device-controlled string before printing
/// it in human output, so a hostile phone cannot inject terminal escapes.
fn sanitize_human(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !matches!(
                *character,
                '\0'..='\u{1F}' | '\u{7F}' | '\u{80}'..='\u{9F}'
            )
        })
        .collect()
}

fn media_preview_limit(args: &MediaPreviewArgs) -> Option<usize> {
    if args.all {
        return None;
    }
    Some(args.limit.unwrap_or(DEFAULT_MEDIA_PREVIEW_LIMIT))
}

async fn media_command(
    client: &HandShakerClient,
    context: &CommandContext,
    command: &MediaCommand,
) -> Result<Outcome> {
    let localizer = ZhCn;
    match command {
        MediaCommand::Photo(args) => {
            let library = client.get_photo_library().await?;
            let limit = media_preview_limit(args);
            let (shown, entries_truncated) = truncate(&library.images, limit);
            let (albums, albums_truncated) = truncate(&library.albums, limit);
            let truncated = entries_truncated || albums_truncated;
            let human = shown
                .iter()
                .enumerate()
                .map(|(index, image)| {
                    format!(
                        "{}\t{}\t{}x{}",
                        index + 1,
                        sanitize_human(
                            image
                                .title
                                .as_deref()
                                .or(image.path.as_deref())
                                .unwrap_or("")
                        ),
                        image.width.unwrap_or(0),
                        image.height.unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let data = serde_json::json!({
                "images": shown,
                "albums": albums,
                "camera_album_id": library.camera_album_id,
            });
            let (human, data) = preview_summary(
                human,
                data,
                library.images.len(),
                truncated,
                limit,
                localizer,
            );
            Ok(Outcome::new("media.photo", data, human)?)
        }
        MediaCommand::Video(args) => {
            let library = client.get_video_library().await?;
            let limit = media_preview_limit(args);
            let (shown, entries_truncated) = truncate(&library.videos, limit);
            let (albums, albums_truncated) = truncate(&library.albums, limit);
            let truncated = entries_truncated || albums_truncated;
            let human = shown
                .iter()
                .enumerate()
                .map(|(index, video)| {
                    format!(
                        "{}\t{}\t{}s",
                        index + 1,
                        sanitize_human(video.path.as_deref().unwrap_or("")),
                        video.duration.unwrap_or(0.0)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let data = serde_json::json!({
                "videos": shown,
                "albums": albums,
            });
            let (human, data) = preview_summary(
                human,
                data,
                library.videos.len(),
                truncated,
                limit,
                localizer,
            );
            Ok(Outcome::new("media.video", data, human)?)
        }
        MediaCommand::Audio(args) => {
            let library = client.get_audio_library().await?;
            let limit = media_preview_limit(args);
            let (shown, entries_truncated) = truncate(&library.tracks, limit);
            let (albums, albums_truncated) = truncate(&library.albums, limit);
            let truncated = entries_truncated || albums_truncated;
            let human = shown
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    format!(
                        "{}\t{}\t{}\t{}",
                        index + 1,
                        sanitize_human(track.title.as_deref().unwrap_or("")),
                        sanitize_human(track.artist.as_deref().unwrap_or("")),
                        sanitize_human(track.path.as_deref().unwrap_or(""))
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let data = serde_json::json!({
                "tracks": shown,
                "albums": albums,
            });
            let (human, data) = preview_summary(
                human,
                data,
                library.tracks.len(),
                truncated,
                limit,
                localizer,
            );
            Ok(Outcome::new("media.audio", data, human)?)
        }
        MediaCommand::Thumbnail {
            targets,
            output_dir,
        } => {
            let output_dir = resolve_local(&context.local_cwd, output_dir);
            std::fs::create_dir_all(&output_dir).map_err(|error| {
                Error::LocalIo(i18n::format(
                    "media.output_dir_create_failed",
                    &[&output_dir.display().to_string(), &error.to_string()],
                ))
            })?;
            let images: Vec<handshaker_rust::ImageFile> = targets
                .iter()
                .map(|target| {
                    if target.chars().all(|character| character.is_ascii_digit()) {
                        let media_id = target.parse::<u64>().map_err(|_| {
                            Error::Usage(
                                localizer.format(MessageKey::MediaThumbnailInvalidId, &[target]),
                            )
                        })?;
                        Ok(handshaker_rust::ImageFile {
                            media_id: Some(media_id),
                            ..Default::default()
                        })
                    } else {
                        Ok(handshaker_rust::ImageFile {
                            path: Some(resolve_remote(&context.remote_cwd, target)),
                            ..Default::default()
                        })
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            let thumbnails = client.get_thumbnails(&images, &[], &[]).await?;
            let mut written = Vec::new();
            let mut failed = Vec::new();
            for (index, target) in targets.iter().enumerate() {
                // Match responses by the echoed media_id/path instead of by
                // request position: a phone that reorders or dedupes entries
                // must not misattribute thumbnail bytes.
                let request = &images[index];
                let image = thumbnails.images.iter().find(|image| {
                    (request.media_id.is_some() && image.media_id == request.media_id)
                        || (request.path.is_some() && image.path == request.path)
                });
                let Some(image) = image else {
                    failed.push(serde_json::json!({
                        "target": target,
                        "reason": "missing_response"
                    }));
                    continue;
                };
                if image.thumbnail_error {
                    failed.push(serde_json::json!({
                        "target": target,
                        "reason": "thumbnail_error"
                    }));
                    eprintln!(
                        "{}",
                        localizer.format(MessageKey::MediaThumbnailFailed, &[target])
                    );
                    continue;
                }
                let Some(bytes) = &image.thumbnail else {
                    failed.push(serde_json::json!({
                        "target": target,
                        "reason": "missing_data"
                    }));
                    continue;
                };
                let name = thumbnail_file_name(target, index);
                tokio::fs::write(output_dir.join(&name), bytes)
                    .await
                    .map_err(|error| {
                        Error::LocalIo(i18n::format(
                            "media.thumbnail_write_failed",
                            &[&name, &error.to_string()],
                        ))
                    })?;
                written.push(serde_json::json!({
                    "target": target,
                    "file": output_dir.join(&name).display().to_string(),
                }));
            }
            let human = if failed.is_empty() {
                localizer.format(
                    MessageKey::MediaThumbnailWritten,
                    &[&written.len().to_string()],
                )
            } else {
                localizer.format(
                    MessageKey::MediaThumbnailPartial,
                    &[&written.len().to_string(), &failed.len().to_string()],
                )
            };
            Ok(Outcome::new(
                "media.thumbnail",
                serde_json::json!({ "written": written, "failed": failed }),
                human,
            )?)
        }
    }
}

fn truncate<T>(entries: &[T], limit: Option<usize>) -> (&[T], bool) {
    match limit {
        Some(limit) if entries.len() > limit => (&entries[..limit], true),
        Some(limit) => (&entries[..entries.len().min(limit)], false),
        None => (entries, false),
    }
}

fn preview_summary(
    human: String,
    data: serde_json::Value,
    total: usize,
    truncated: bool,
    limit: Option<usize>,
    localizer: ZhCn,
) -> (String, serde_json::Value) {
    let mut data = data;
    data["total"] = serde_json::json!(total);
    // Always present: JSON consumers can distinguish a capped preview from an
    // absent flag without probing the number of entries.
    data["truncated"] = serde_json::json!(truncated);
    let human = if truncated {
        format!(
            "{}\n{}",
            human,
            localizer.format(
                MessageKey::MediaPreviewTruncated,
                &[&total.to_string(), &limit.unwrap_or(0).to_string()],
            )
        )
    } else {
        human
    };
    (human, data)
}

/// Local-only thumbnail file name derived from the caller's target: numeric
/// targets become `<id>.jpg`, paths become the last path component. Remote
/// names are never used, so a hostile phone cannot escape the output dir.
fn thumbnail_file_name(target: &str, index: usize) -> String {
    let component = target
        .rsplit(['/', '\\'])
        .next()
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| index.to_string());
    let stem = component
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(component);
    // Strip path separators, drive-letter colons and control characters so a
    // hostile or odd target can never escape the output directory.
    let stem: String = stem
        .chars()
        .filter(|character| !matches!(*character, '/' | '\\' | ':' | '\0'..='\u{1F}' | '\u{7F}'))
        .collect();
    let stem = if stem.is_empty() {
        index.to_string()
    } else {
        stem
    };
    format!("{index}_{stem}.jpg")
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

    #[test]
    fn watch_accepts_repeatable_paths() {
        let cli = Cli::try_parse_from([
            "handshaker",
            "watch",
            "--path",
            "/storage/emulated/0/DCIM",
            "--path",
            "/storage/emulated/0/Pictures",
        ])
        .expect("parse watch");
        match cli.command {
            Command::Watch { paths } => {
                assert_eq!(
                    paths,
                    vec![
                        "/storage/emulated/0/DCIM".to_string(),
                        "/storage/emulated/0/Pictures".to_string()
                    ]
                );
            }
            _ => panic!("expected watch command"),
        }
    }

    #[test]
    fn watch_envelope_carries_event_payload() {
        let event = ClientEvent::ClipboardChanged(vec![handshaker_rust::ClipboardEntry {
            text: "hello".to_string(),
            timestamp_ms: 42,
        }]);
        let device = DeviceInfo {
            serial: "serial-1".to_string(),
            name: Some("phone".to_string()),
            ..DeviceInfo::default()
        };
        let envelope = super::watch_envelope(&device, &event);
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["command"], "watch");
        assert_eq!(envelope["ok"], true);
        assert_eq!(envelope["event"], "watch");
        assert_eq!(envelope["data"]["kind"], "clipboard_changed");
        assert_eq!(envelope["device"]["serial"], "serial-1");
    }

    #[test]
    fn media_preview_limit_defaults_to_preview_cap() {
        let args = MediaPreviewArgs {
            limit: None,
            all: false,
        };
        assert_eq!(
            super::media_preview_limit(&args),
            Some(super::DEFAULT_MEDIA_PREVIEW_LIMIT)
        );

        let limited = MediaPreviewArgs {
            limit: Some(7),
            all: false,
        };
        assert_eq!(super::media_preview_limit(&limited), Some(7));

        let all = MediaPreviewArgs {
            limit: None,
            all: true,
        };
        assert_eq!(super::media_preview_limit(&all), None);
    }

    #[test]
    fn media_truncation_reports_total_and_flags() {
        let entries = vec![1, 2, 3, 4, 5];
        let (shown, truncated) = super::truncate(&entries, Some(3));
        assert_eq!(shown, &[1, 2, 3][..]);
        assert!(truncated);

        let (all_shown, truncated) = super::truncate(&entries, None);
        assert_eq!(all_shown, &entries[..]);
        assert!(!truncated);
    }

    #[test]
    fn thumbnail_file_names_are_local_and_safe() {
        assert_eq!(
            super::thumbnail_file_name("42", 0),
            "0_42.jpg",
            "numeric targets become index_id.jpg"
        );
        assert_eq!(
            super::thumbnail_file_name("/storage/emulated/0/DCIM/a.jpg", 3),
            "3_a.jpg",
            "path targets keep only the last component"
        );
        assert_eq!(
            super::thumbnail_file_name("../../evil/../x", 1),
            "1_x.jpg",
            "parent segments cannot escape the output directory"
        );
    }

    #[test]
    fn human_output_strips_terminal_escape_characters() {
        let escaped = "\u{1B}[2Jevil\u{9B}CSItitle";
        // ESC and CSI control characters are removed; the residue "[2J" is
        // inert plain text once the escape introducer is gone.
        assert_eq!(super::sanitize_human(escaped), "[2JevilCSItitle");
        assert_eq!(super::sanitize_human("normal text"), "normal text");
    }
}
