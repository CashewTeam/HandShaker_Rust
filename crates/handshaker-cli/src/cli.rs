use std::env;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use serde::Serialize;

use handshaker_core::{
    ClientEvent, ClientOptions, ClipboardEntry, ConnectionTarget, DeviceInfo, Error, EventCallbacks,
    EventFilter, EventStreamError, HandShakerClient, RemoteFile, Result, StateStore, SyncConfig,
    SyncDiff, SyncRunResult, SyncSnapshot, SyncStore, apply_file_change, check_conflicts,
    default_config_dir, execute_plan,
    i18n::{self, Localizer, MessageKey, ZhCn},
    plan_diff, sync_config,
};

use handshaker_application::{
    BatchTransferItemDto, BatchTransferRequest, ConnectRequest, CountFilesRequest,
    CreateDirectoryRequest, DeletePathsRequest, DeviceDescriptor, DeviceId, FileEntryDto,
    HandShakerRuntime, ListDevicesRequest, ListFilesRequest, MovePathRequest, PublicError,
    RuntimeConfig, SessionId, StatFileRequest, TransferFailureDto, TransportKind, TreeTransferDto,
};

use crate::output::{Outcome, render};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Human,
    Json,
    Jsonl,
}

#[derive(Debug, Serialize)]
struct DryRunReport {
    files: usize,
    dirs: usize,
    bytes: u64,
    dry_run: bool,
}

/// CLI-side rendering shape for a remote file entry, mirroring the legacy
/// core `RemoteFile` JSON contract so migrating commands stay byte-compatible
/// (the application DTO uses `created_at_ms`/`modified_at_ms`/`media_id`).
#[derive(Debug, Serialize)]
struct CliFileEntry {
    path: String,
    size: u64,
    created_at: Option<u64>,
    modified_at: Option<u64>,
    is_directory: bool,
    checksum: Option<String>,
    is_trash: Option<bool>,
    id: Option<u64>,
    ext_data: Option<String>,
}

/// Map an application file entry onto the CLI rendering shape. `ext_data` is
/// media-channel-only in core and never populated by directory listings, so
/// it is always `None` here.
fn cli_file_entry(file: &FileEntryDto) -> CliFileEntry {
    CliFileEntry {
        path: file.path.clone(),
        size: file.size,
        created_at: file.created_at_ms,
        modified_at: file.modified_at_ms,
        is_directory: file.is_directory,
        checksum: file.checksum.clone(),
        is_trash: file.is_trash,
        id: file.media_id,
        ext_data: None,
    }
}

/// Detect a missing `--` in `fs pull REMOTE LOCAL`: the second positional was
/// absorbed into `remote` and names a path that exists under the CLI's local
/// working directory (`local_cwd`, which the shell `lcd` command mutates).
fn pull_target_misparsed(remote: &[String], local: Option<&PathBuf>, local_cwd: &Path) -> bool {
    local.is_none() && remote.len() > 1 && local_cwd.join(&remote[1]).exists()
}

/// Local recursive scan used by `--dry-run` to estimate a push tree without
/// touching the device.
fn collect_local_tree<T: Localizer>(
    local: &Path,
    remote: &str,
    items: &mut Vec<BatchTransferItemDto>,
    directories: &mut std::collections::BTreeSet<String>,
    bytes: &mut u64,
    localizer: &T,
) -> Result<()> {
    let remote_base = remote.trim_end_matches('/');
    let entries = std::fs::read_dir(local).map_err(|error| {
        Error::LocalIo(localizer.format(
            MessageKey::ReadDirFailed,
            &[&local.display().to_string(), &error.to_string()],
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::LocalIo(localizer.format(
                MessageKey::ReadDirFailed,
                &[&local.display().to_string(), &error.to_string()],
            ))
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // Match the real upload walk: symlinks are skipped, so a dry-run must
        // not count them either.
        let file_type = entry.file_type().map_err(|error| {
            Error::LocalIo(localizer.format(
                MessageKey::ReadDirFailed,
                &[&path.display().to_string(), &error.to_string()],
            ))
        })?;
        if file_type.is_dir() {
            let remote_dir = format!("{remote_base}/{name}");
            directories.insert(remote_dir.clone());
            collect_local_tree(&path, &remote_dir, items, directories, bytes, localizer)?;
        } else if file_type.is_file() {
            items.push(BatchTransferItemDto {
                source: path.display().to_string(),
                target: format!("{remote_base}/{name}"),
            });
            *bytes += entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        }
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[command(name = "handshaker", version)]
pub(crate) struct Cli {
    #[arg(long, global = true)]
    pub serial: Option<String>,

    #[arg(long, global = true, conflicts_with = "serial", value_parser = parse_socket_addr)]
    pub wifi: Option<SocketAddr>,

    /// Connect over USB AOA instead of ADB/WiFi. With `--serial`, the value is
    /// the accessory `bus-ports` location (e.g. `1-2`).
    #[arg(long, global = true, conflicts_with = "wifi")]
    pub usb: bool,

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
    localize_subcommand(&mut command, "sync", "cli.command.sync");
    localize_subcommand(&mut command, "shell", "cli.command.shell");
    localize_subcommand(&mut command, "batch", "cli.command.batch");
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
    if let Some(sync) = command.find_subcommand_mut("sync") {
        localize_subcommand(sync, "plan", "cli.command.sync_plan");
        localize_subcommand(sync, "run", "cli.command.sync_run");
        localize_subcommand(sync, "watch", "cli.command.sync_watch");
        localize_subcommand(sync, "status", "cli.command.sync_status");
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
    #[command(subcommand)]
    Sync(SyncCommand),
    Shell,
    /// Read commands from stdin (one per line) and run them on a single
    /// persistent connection. Non-interactive; exit/quit ends the session.
    Batch,
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
        #[arg(value_name = "REMOTE", required = true, num_args = 1..)]
        remote: Vec<String>,
        #[arg(value_name = "LOCAL", last = true)]
        local: Option<PathBuf>,
        #[arg(short = 'r', long)]
        recursive: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        dry_run: bool,
    },
    Push {
        #[arg(value_name = "LOCAL", required = true, num_args = 1..)]
        local: Vec<PathBuf>,
        #[arg(value_name = "REMOTE", last = true)]
        remote: String,
        #[arg(short = 'r', long)]
        recursive: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        dry_run: bool,
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

/// Photo sync (phone -> host). The ledger lives in
/// `<config>/sync/<device_uuid>.json` and is committed atomically after each
/// run; re-runs are idempotent.
#[derive(Debug, Subcommand)]
pub(crate) enum SyncCommand {
    /// Preview the diff (downloads/deletes/conflicts) without touching files.
    Plan {
        /// Phone-side photo root to sync.
        #[arg(long = "root", id = "sync_root")]
        root: Option<String>,
        /// Local destination directory.
        #[arg(long = "output-dir", id = "sync_output_dir")]
        output_dir: Option<PathBuf>,
    },
    /// Execute a full sync: download new/modified photos, remove deleted
    /// ones, and commit the ledger atomically.
    Run {
        /// Phone-side photo root to sync.
        #[arg(long = "root", id = "sync_root")]
        root: Option<String>,
        /// Local destination directory.
        #[arg(long = "output-dir", id = "sync_output_dir")]
        output_dir: Option<PathBuf>,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Full sync, then keep watching for FILE_CHANGE(38) pushes and apply
    /// them incrementally until interrupted.
    Watch {
        /// Phone-side photo root to sync.
        #[arg(long = "root", id = "sync_root")]
        root: Option<String>,
        /// Local destination directory.
        #[arg(long = "output-dir", id = "sync_output_dir")]
        output_dir: Option<PathBuf>,
        /// Skip the confirmation prompt (initial sync can delete local files).
        #[arg(long)]
        yes: bool,
    },
    /// Show the local sync ledger summary.
    Status,
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
        Command::Sync(SyncCommand::Plan { .. }) => "sync.plan",
        Command::Sync(SyncCommand::Run { .. }) => "sync.run",
        Command::Sync(SyncCommand::Watch { .. }) => "sync.watch",
        Command::Sync(SyncCommand::Status) => "sync.status",
        Command::Shell => "shell",
        Command::Batch => "batch",
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
    if matches!(cli.command, Command::Batch) {
        return run_batch(&cli).await;
    }
    if matches!(cli.command, Command::Watch { .. }) {
        return watch(&cli).await;
    }
    // Parameter validation must happen before any device connection so
    // missing required arguments surface as usage errors (exit 2).
    if let Command::Sync(command) = &cli.command {
        let missing_output_dir = match command {
            SyncCommand::Plan { output_dir, .. }
            | SyncCommand::Run { output_dir, .. }
            | SyncCommand::Watch { output_dir, .. } => output_dir.is_none(),
            SyncCommand::Status => false,
        };
        if missing_output_dir {
            return Err(Error::Usage(
                i18n::text("sync.output_dir_required").to_string(),
            ));
        }
    }

    let app = connect(&cli).await?;
    let context = CommandContext {
        remote_cwd: app.client.root_path().to_string(),
        local_cwd: env::current_dir()?,
        in_shell: false,
    };
    let command = command_name(&cli.command);
    let outcome = execute_connected(&cli.command, &app, &context, cli.yes, cli.output).await;
    let close = close_session(app).await;
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

/// CLI-owned view of an open application session: business services
/// (runtime), the session id, and the transition client for commands that
/// are not yet migrated to the application layer (M8 Phase 3).
pub(crate) struct AppSession {
    pub runtime: Arc<HandShakerRuntime>,
    pub session_id: SessionId,
    pub client: Arc<HandShakerClient>,
}

/// Build the runtime configuration from CLI options.
fn runtime_config(cli: &Cli) -> RuntimeConfig {
    RuntimeConfig {
        adb_path: PathBuf::from("adb"),
        default_timeout: cli.timeout,
        heartbeat_interval: Duration::from_secs(10),
        state_dir: None,
        wire_log: cli.wire_log.clone(),
        event_capacity: 1024,
    }
}

/// Resolve the CLI connection target to an application `DeviceDescriptor`.
/// Mirrors core `ConnectionTarget` semantics: an explicit ADB serial is
/// looked up among online devices; without one, the single online ADB device
/// is auto-selected (0/multiple -> DeviceSelection, exit 3); USB uses the
/// accessory location; WiFi uses `IP:PORT`.
async fn select_device_descriptor(
    cli: &Cli,
    runtime: &HandShakerRuntime,
) -> Result<DeviceDescriptor> {
    if cli.usb {
        let location = cli.serial.clone().unwrap_or_default();
        return Ok(DeviceDescriptor {
            id: DeviceId(location.clone()),
            display_name: if location.is_empty() {
                None
            } else {
                Some(location.clone())
            },
            model: None,
            transport: TransportKind::UsbAccessory,
            transport_address: location,
            available: true,
            adb: None,
            usb: None,
        });
    }
    if let Some(address) = cli.wifi {
        return Ok(DeviceDescriptor {
            id: DeviceId(format!("wifi:{address}")),
            display_name: Some(address.to_string()),
            model: None,
            transport: TransportKind::Wifi,
            transport_address: address.to_string(),
            available: true,
            adb: None,
            usb: None,
        });
    }
    let devices = runtime
        .list_devices(ListDevicesRequest {
            include_adb: true,
            include_wifi: false,
            include_usb: false,
            wifi_browse_timeout: Duration::from_secs(3),
        })
        .await
        .map_err(app_error)?;
    let online: Vec<_> = devices.into_iter().filter(|d| d.available).collect();
    match &cli.serial {
        Some(serial) => online
            .into_iter()
            .find(|d| &d.id.0 == serial)
            .ok_or_else(|| {
                Error::DeviceSelection(i18n::format("adb.device_unavailable", &[serial]))
            }),
        None => match online.len() {
            0 => Err(Error::DeviceSelection(
                i18n::text("adb.no_online_device").to_string(),
            )),
            1 => Ok(online.into_iter().next().expect("one device")),
            count => Err(Error::DeviceSelection(i18n::format(
                "adb.multiple_devices",
                &[&count.to_string()],
            ))),
        },
    }
}

/// Map an application-layer error onto the CLI error taxonomy. Connect-class
/// failures collapse into `Transport` (exit 4); device-selection errors are
/// raised directly by the CLI selector, so this path only sees connect-class
/// failures.
fn app_error(error: PublicError) -> Error {
    Error::Transport(error.to_string())
}

/// Unified close for an application session: drop the CLI's client handle so
/// the runtime's disconnect takes sole ownership and sends QUIT, then removes
/// the session from the registry (idempotent).
async fn close_session(app: AppSession) -> Result<()> {
    let AppSession {
        runtime,
        session_id,
        client,
    } = app;
    drop(client);
    runtime.disconnect(session_id).await.map_err(app_error)
}

async fn connect(cli: &Cli) -> Result<AppSession> {
    if cli.wifi.is_some() {
        // First connects and resets require acting on the phone; give a hint
        // before the handshake blocks waiting for the trust dialog.
        eprintln!("{}", ZhCn.text(MessageKey::WifiTrustHint));
    }
    let runtime = Arc::new(
        HandShakerRuntime::create(runtime_config(cli))
            .await
            .map_err(app_error)?,
    );
    let device = select_device_descriptor(cli, &runtime).await?;
    let session_id = runtime
        .connect(ConnectRequest { device })
        .await
        .map_err(app_error)?;
    let client = runtime
        .session_client(session_id)
        .await
        .map_err(app_error)?;
    Ok(AppSession {
        runtime,
        session_id,
        client,
    })
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
    if cli.usb {
        return ConnectionTarget::Usb {
            location_id: cli.serial.clone(),
        };
    }
    match cli.wifi {
        Some(address) => ConnectionTarget::Wifi { address },
        None => ConnectionTarget::Adb {
            serial: cli.serial.clone(),
        },
    }
}

async fn device_list(timeout: Duration) -> Result<Outcome> {
    use handshaker_application::{
        DeviceDescriptor, HandShakerRuntime, ListDevicesRequest, RuntimeConfig, TransportKind,
    };

    // Route through the application service layer (M8): the CLI keeps its
    // output model; the application owns discovery + transport details.
    let config = RuntimeConfig {
        default_timeout: timeout,
        adb_path: "adb".into(),
        ..RuntimeConfig::default()
    };
    let runtime = HandShakerRuntime::create(config)
        .await
        .map_err(|error| handshaker_core::Error::LocalIo(error.message))?;
    let request = ListDevicesRequest {
        include_adb: true,
        include_wifi: false,
        include_usb: true,
        wifi_browse_timeout: timeout,
    };
    let devices = runtime
        .list_devices(request)
        .await
        .map_err(|error| handshaker_core::Error::LocalIo(error.message))?;
    let _ = runtime.shutdown().await;

    let localizer = ZhCn;
    let mut lines = Vec::new();
    let adb_rows: Vec<DeviceDescriptor> = devices
        .iter()
        .filter(|device| device.transport == TransportKind::Adb)
        .cloned()
        .collect();
    if !adb_rows.is_empty() {
        lines.push(localizer.text(MessageKey::DeviceListHeader).to_string());
        lines.extend(adb_rows.iter().map(|device| {
            let detail = device.adb.as_ref().expect("adb detail present");
            format!(
                "{}\t{}\t{}\t{}",
                device.id.0,
                detail.state,
                detail.model.as_deref().unwrap_or("-"),
                detail.device.as_deref().unwrap_or("-")
            )
        }));
    }
    let usb_rows: Vec<DeviceDescriptor> = devices
        .iter()
        .filter(|device| device.transport == TransportKind::UsbAccessory)
        .cloned()
        .collect();
    if !usb_rows.is_empty() {
        lines.push(localizer.text(MessageKey::UsbDeviceListHeader).to_string());
        lines.extend(usb_rows.iter().map(|device| {
            let detail = device.usb.as_ref().expect("usb detail present");
            format!(
                "{}\t{:04x}:{:04x}\t{}",
                device.id.0,
                detail.vendor_id,
                detail.product_id,
                sanitize_human(detail.serial.as_deref().unwrap_or("-"))
            )
        }));
    }
    if lines.is_empty() {
        lines.push(localizer.text(MessageKey::NoDevices).to_string());
    }
    // Rebuild the legacy JSON payload so the CLI contract is unchanged.
    let adb_payload: Vec<serde_json::Value> = adb_rows
        .iter()
        .map(|device| {
            let detail = device.adb.as_ref().expect("adb detail present");
            serde_json::json!({
                "serial": device.id.0,
                "state": detail.state,
                "product": detail.product,
                "model": detail.model,
                "device": detail.device,
            })
        })
        .collect();
    let usb_payload: Vec<serde_json::Value> = usb_rows
        .iter()
        .map(|device| {
            let detail = device.usb.as_ref().expect("usb detail present");
            serde_json::json!({
                "location": device.id.0,
                "bus_number": detail.bus_number,
                "serial": detail.serial,
                "vendor_id": detail.vendor_id,
                "product_id": detail.product_id,
                "mode": detail.mode,
            })
        })
        .collect();
    let payload = serde_json::json!({
        "adb": adb_payload,
        "usb": usb_payload,
    });
    Outcome::new("device.list", payload, lines.join("\n"))
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

fn human_trust_records(records: &[handshaker_core::TrustRecordInfo]) -> String {
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
    session: &AppSession,
    context: &CommandContext,
    yes: bool,
    format: OutputFormat,
) -> Result<Outcome> {
    let localizer = ZhCn;
    let client = &session.client;
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
            let entries = session
                .runtime
                .list_files(ListFilesRequest {
                    session_id: session.session_id,
                    path,
                    depth: *depth,
                })
                .await
                .map_err(app_error)?;
            let files: Vec<_> = entries.iter().map(cli_file_entry).collect();
            Outcome::new("fs.ls", &files, human_files(&files))?
        }
        Command::Fs(FsCommand::Stat { path }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let file = session
                .runtime
                .stat_file(StatFileRequest {
                    session_id: session.session_id,
                    path: path.clone(),
                })
                .await
                .map_err(app_error)?
                .ok_or_else(|| Error::RemoteIo {
                    code: None,
                    message: localizer.format(MessageKey::RemoteMissing, &[&path]),
                })?;
            let entry = cli_file_entry(&file);
            Outcome::new("fs.stat", &entry, human_file(&entry))?
        }
        Command::Fs(FsCommand::Count {
            path,
            depth,
            exclusions,
        }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let count = session
                .runtime
                .count_files(CountFilesRequest {
                    session_id: session.session_id,
                    path: path.clone(),
                    depth: *depth,
                    exclusions: exclusions.clone(),
                })
                .await
                .map_err(app_error)?;
            Outcome::new(
                "fs.count",
                serde_json::json!({ "path": path, "count": count }),
                localizer.format(MessageKey::FileCount, &[&count.to_string()]),
            )?
        }
        Command::Fs(FsCommand::Exists { path }) => {
            let path = resolve_remote(&context.remote_cwd, path);
            let exists = session
                .runtime
                .stat_file(StatFileRequest {
                    session_id: session.session_id,
                    path: path.clone(),
                })
                .await
                .map_err(app_error)?
                .is_some();
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
            session
                .runtime
                .create_directory(CreateDirectoryRequest {
                    session_id: session.session_id,
                    path: path.clone(),
                })
                .await
                .map_err(app_error)?;
            // The phone returns the created entry on success; stat it back so
            // the JSON contract (a RemoteFile-shaped entry) is preserved.
            let file = session
                .runtime
                .stat_file(StatFileRequest {
                    session_id: session.session_id,
                    path: path.clone(),
                })
                .await
                .map_err(app_error)?
                .ok_or_else(|| Error::RemoteIo {
                    code: None,
                    message: localizer.format(MessageKey::RemoteMissing, &[&path]),
                })?;
            let entry = cli_file_entry(&file);
            Outcome::new(
                "fs.mkdir",
                &entry,
                localizer.format(MessageKey::DirectoryCreated, &[&path]),
            )?
        }
        Command::Fs(FsCommand::Mv { source, target }) => {
            let source = resolve_remote(&context.remote_cwd, source);
            let target = resolve_remote(&context.remote_cwd, target);
            session
                .runtime
                .move_path(MovePathRequest {
                    session_id: session.session_id,
                    source: source.clone(),
                    target: target.clone(),
                })
                .await
                .map_err(app_error)?;
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
                let file = session
                    .runtime
                    .stat_file(StatFileRequest {
                        session_id: session.session_id,
                        path: path.clone(),
                    })
                    .await
                    .map_err(app_error)?
                    .ok_or_else(|| Error::RemoteIo {
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
            let deleted = session
                .runtime
                .delete_paths(DeletePathsRequest {
                    session_id: session.session_id,
                    paths: paths.clone(),
                    trash: *trash,
                    sync: false,
                })
                .await
                .map_err(app_error)?;
            let entries: Vec<_> = deleted.deleted.iter().map(cli_file_entry).collect();
            Outcome::new(
                "fs.rm",
                &entries,
                localizer.format(MessageKey::DeletedCount, &[&entries.len().to_string()]),
            )?
        }
        Command::Fs(FsCommand::Pull {
            remote,
            local,
            recursive,
            overwrite,
            dry_run,
        }) => {
            // 0.3.x compat: the two-argument form `fs pull REMOTE LOCAL` (no
            // `--`) is absorbed by the multi-value `remote` positional. When
            // the second argument names an existing local path, it is almost
            // certainly a missing `--`; refuse loudly instead of fetching the
            // local path from the device.
            if pull_target_misparsed(&remote, local.as_ref(), &context.local_cwd) {
                return Err(Error::Usage(
                    localizer.format(MessageKey::PullNeedsSeparator, &[]),
                ));
            }
            let single_file_mode = remote.len() == 1 && !*recursive;
            let mut items: Vec<BatchTransferItemDto> = Vec::new();
            let mut trees: Vec<TreeTransferDto> = Vec::new();
            let mut existing_targets = 0_usize;
            let mut total_bytes = 0_u64;
            let mut seen_targets = std::collections::BTreeSet::new();
            for remote_path in remote {
                let remote_path = resolve_remote(&context.remote_cwd, &remote_path);
                let info = client.stat(&remote_path).await?;
                if info.as_ref().is_some_and(|file| file.is_directory) {
                    if !*recursive {
                        return Err(Error::Usage(
                            localizer.format(MessageKey::RecursiveRequired, &[&remote_path]),
                        ));
                    }
                    let local_dir = if let Some(local) = local {
                        resolve_local(&context.local_cwd, local)
                    } else {
                        context.local_cwd.clone()
                    };
                    trees.push(TreeTransferDto {
                        source: remote_path,
                        target: local_dir.display().to_string(),
                    });
                } else {
                    let local_target = if single_file_mode {
                        resolve_local_pull(&context.local_cwd, local.as_deref(), &remote_path)?
                    } else {
                        let base = if let Some(local) = local {
                            resolve_local(&context.local_cwd, local)
                        } else {
                            context.local_cwd.clone()
                        };
                        base.join(remote_name(&remote_path).ok_or_else(|| {
                            Error::Usage(
                                localizer.format(MessageKey::RemoteNameMissing, &[&remote_path]),
                            )
                        })?)
                    };
                    if local_target.exists() {
                        existing_targets += 1;
                    }
                    if !seen_targets.insert(local_target.display().to_string()) {
                        return Err(Error::Usage(localizer.format(
                            MessageKey::DuplicateTarget,
                            &[&local_target.display().to_string()],
                        )));
                    }
                    if let Some(file) = info.as_ref() {
                        total_bytes += file.size;
                    }
                    items.push(BatchTransferItemDto {
                        source: remote_path,
                        target: local_target.display().to_string(),
                    });
                }
            }
            if *dry_run {
                let mut files = items.len();
                let mut dirs = trees.len();
                let mut bytes = total_bytes;
                for tree in &trees {
                    let entries = session
                        .runtime
                        .list_files(ListFilesRequest {
                            session_id: session.session_id,
                            path: tree.source.clone(),
                            depth: u32::MAX,
                        })
                        .await
                        .map_err(app_error)?;
                    for entry in entries {
                        if entry.is_directory {
                            dirs += 1;
                        } else {
                            files += 1;
                            bytes += entry.size;
                        }
                    }
                }
                let report = DryRunReport {
                    files,
                    dirs,
                    bytes,
                    dry_run: true,
                };
                return Ok(Outcome::new(
                    "fs.pull",
                    &report,
                    localizer.format(
                        MessageKey::DryRunReport,
                        &[&files.to_string(), &dirs.to_string(), &bytes.to_string()],
                    ),
                )?);
            }
            if existing_targets > 0 {
                if !overwrite {
                    return Err(Error::LocalIo(localizer.format(
                        MessageKey::LocalTargetExists,
                        &[&items[0].target.clone()],
                    )));
                }
                confirm(
                    &localizer.format(
                        MessageKey::OverwriteLocalBatch,
                        &[&existing_targets.to_string()],
                    ),
                    yes,
                    format,
                )?;
            }
            let result = session
                .runtime
                .batch_download(BatchTransferRequest {
                    session_id: session.session_id,
                    files: items,
                    trees,
                    overwrite: *overwrite,
                })
                .await
                .map_err(app_error)?;
            let outcome = Outcome::new(
                "fs.pull",
                &result,
                localizer.format(
                    MessageKey::BatchDone,
                    &[
                        &result.ok.len().to_string(),
                        &result.failures.len().to_string(),
                    ],
                ),
            )?;
            if !result.failures.is_empty() {
                outcome.with_warning(localizer.format(
                    MessageKey::BatchFailures,
                    &[&result.failures.len().to_string()],
                ))
            } else {
                outcome
            }
        }
        Command::Fs(FsCommand::Push {
            local,
            remote,
            recursive,
            overwrite,
            dry_run,
        }) => {
            let remote = resolve_remote(&context.remote_cwd, &remote);
            let single_file_mode = local.len() == 1 && !recursive && !remote.ends_with('/');
            let mut items: Vec<BatchTransferItemDto> = Vec::new();
            let mut trees: Vec<TreeTransferDto> = Vec::new();
            let mut existing_targets = 0_usize;
            let mut collected_failures: Vec<TransferFailureDto> = Vec::new();
            let mut total_bytes = 0_u64;
            let mut seen_targets = std::collections::BTreeSet::new();
            for local_path in local {
                let local_path = resolve_local(&context.local_cwd, &local_path);
                let metadata = match tokio::fs::metadata(&local_path).await {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        collected_failures.push(TransferFailureDto {
                            source: local_path.display().to_string(),
                            target: remote.clone(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                if metadata.is_dir() {
                    if !recursive {
                        return Err(Error::Usage(localizer.format(
                            MessageKey::RecursiveRequired,
                            &[&local_path.display().to_string()],
                        )));
                    }
                    trees.push(TreeTransferDto {
                        source: local_path.display().to_string(),
                        target: remote.clone(),
                    });
                } else {
                    let target = if single_file_mode {
                        remote.clone()
                    } else {
                        format!(
                            "{}/{}",
                            remote.trim_end_matches('/'),
                            local_path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        )
                    };
                    if client.file_exists(&target).await? {
                        existing_targets += 1;
                    }
                    if !seen_targets.insert(target.clone()) {
                        return Err(Error::Usage(
                            localizer.format(MessageKey::DuplicateTarget, &[&target]),
                        ));
                    }
                    total_bytes += metadata.len();
                    items.push(BatchTransferItemDto {
                        source: local_path.display().to_string(),
                        target,
                    });
                }
            }
            if *dry_run {
                let mut files = items.len();
                let mut dirs = trees.len();
                let mut bytes = total_bytes;
                for tree in &trees {
                    let mut tree_items = Vec::new();
                    let mut tree_dirs = std::collections::BTreeSet::new();
                    let mut tree_bytes = 0_u64;
                    collect_local_tree(
                        Path::new(&tree.source),
                        &tree.target,
                        &mut tree_items,
                        &mut tree_dirs,
                        &mut tree_bytes,
                        &localizer,
                    )?;
                    dirs += tree_dirs.len();
                    files += tree_items.len();
                    bytes += tree_bytes;
                }
                let report = DryRunReport {
                    files,
                    dirs,
                    bytes,
                    dry_run: true,
                };
                return Ok(Outcome::new(
                    "fs.push",
                    &report,
                    localizer.format(
                        MessageKey::DryRunReport,
                        &[&files.to_string(), &dirs.to_string(), &bytes.to_string()],
                    ),
                )?);
            }
            if existing_targets > 0 {
                if !overwrite {
                    return Err(Error::RemoteIo {
                        code: None,
                        message: localizer
                            .format(MessageKey::RemoteTargetExists, &[&items[0].target.clone()]),
                    });
                }
                confirm(
                    &localizer.format(
                        MessageKey::OverwriteRemoteBatch,
                        &[&existing_targets.to_string()],
                    ),
                    yes,
                    format,
                )?;
            }
            let mut result = session
                .runtime
                .batch_upload(BatchTransferRequest {
                    session_id: session.session_id,
                    files: items,
                    trees,
                    overwrite: *overwrite,
                })
                .await
                .map_err(app_error)?;
            result.failures.extend(collected_failures);
            let outcome = Outcome::new(
                "fs.push",
                &result,
                localizer.format(
                    MessageKey::BatchDone,
                    &[
                        &result.ok.len().to_string(),
                        &result.failures.len().to_string(),
                    ],
                ),
            )?;
            if !result.failures.is_empty() {
                outcome.with_warning(localizer.format(
                    MessageKey::BatchFailures,
                    &[&result.failures.len().to_string()],
                ))
            } else {
                outcome
            }
        }
        Command::Clipboard(ClipboardCommand::Get) => {
            let entries = session
                .runtime
                .list_clipboards(session.session_id)
                .await
                .map_err(app_error)?;
            let entries: Vec<_> = entries
                .iter()
                .map(|entry| ClipboardEntry {
                    text: entry.text.clone(),
                    timestamp_ms: entry.timestamp_ms,
                })
                .collect();
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
            session
                .runtime
                .set_clipboard(session.session_id, &text)
                .await
                .map_err(app_error)?;
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
            session
                .runtime
                .delete_clipboard(session.session_id, *timestamp)
                .await
                .map_err(app_error)?;
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
            session
                .runtime
                .clear_clipboards(session.session_id)
                .await
                .map_err(app_error)?;
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
        Command::Batch => {
            return Err(Error::Usage(
                localizer.text(MessageKey::ShellNested).to_string(),
            ));
        }
        Command::Watch { .. } => {
            return Err(Error::Usage(
                localizer.text(MessageKey::WatchNested).to_string(),
            ));
        }
        Command::Sync(SyncCommand::Watch { .. }) if context.in_shell => {
            return Err(Error::Usage(i18n::text("sync.watch_nested").to_string()));
        }
        Command::Media(command) => {
            return media_command(client, &context, command).await;
        }
        Command::Sync(command) => {
            return sync_command(client, &context, command, format).await;
        }
    };
    Ok(outcome.with_device(client.device_info()))
}

/// Default preview cap for `media` listings: a functional preview of a large
/// media library, overridable with `--limit` or `--all`.
const DEFAULT_MEDIA_PREVIEW_LIMIT: usize = 50;

/// Default phone-side photo root for sync when `--root` is omitted.
const DEFAULT_SYNC_ROOT: &str = "/storage/emulated/0/DCIM/Camera";

/// Photo sync commands: plan / run / watch / status (phone -> host, one-way).
async fn sync_command(
    client: &HandShakerClient,
    context: &CommandContext,
    command: &SyncCommand,
    format: OutputFormat,
) -> Result<Outcome> {
    match command {
        SyncCommand::Status => {
            let device_uuid = sync_device_uuid(client)?;
            let store = SyncStore::discover(&default_config_dir()?, &device_uuid);
            let snapshot = store.load()?.unwrap_or_default();
            let files = snapshot.files.len();
            let bytes: u64 = snapshot.files.values().map(|record| record.size).sum();
            let data = serde_json::json!({
                "device_uuid": device_uuid,
                "files": files,
                "bytes": bytes,
            });
            let human = i18n::format(
                "sync.status_line",
                &[&files.to_string(), &bytes.to_string()],
            );
            return Ok(Outcome::new("sync.status", data, human)?);
        }
        SyncCommand::Plan { root, output_dir } => {
            let output_dir = required_output_dir(output_dir, context)?;
            let config = sync_config_for(client, root.as_deref(), &output_dir)?;
            let store = SyncStore::discover(&default_config_dir()?, &config.device_uuid);
            let snapshot = store.load()?.unwrap_or_default();
            let (diff, conflicts) = plan_for(client, &config, &snapshot).await?;
            let data = serde_json::json!({
                "added": diff.added,
                "info_modified": diff.info_modified,
                "deleted": diff.deleted,
                "conflicts": conflicts,
                "total": diff.added.len() + diff.info_modified.len() + diff.deleted.len(),
            });
            let human = plan_human(&diff, &conflicts);
            return Ok(Outcome::new("sync.plan", data, human)?);
        }
        SyncCommand::Run {
            root,
            output_dir,
            yes: run_yes,
        } => {
            let output_dir = required_output_dir(output_dir, context)?;
            let config = sync_config_for(client, root.as_deref(), &output_dir)?;
            confirm(&i18n::text("sync.confirm_run"), *run_yes, format)?;
            let store = SyncStore::discover(&default_config_dir()?, &config.device_uuid);
            let snapshot = store.load()?.unwrap_or_default();
            // Single PHOTO_SYNC_REQUEST(37): fetch state and diff in one pass
            // (a second 37 would be rejected while the phone is SYNCING; an
            // up-front SYNC_MONITOR(false) reset is NOT sent — on-device it
            // left the phone answering 37 with a heartbeat, 2026-08-03).
            let phone_files = photo_sync_files(client, &config, &snapshot).await?;
            let diff = plan_diff(&phone_files, &snapshot);
            let conflicts = check_conflicts(&diff, &snapshot);
            let (result, updated) =
                execute_plan(client, &config, &phone_files, &diff, &snapshot, &conflicts).await?;
            store.save(&updated)?;
            let _ = client.sync_monitor(false).await?;
            let data = serde_json::json!({
                "downloaded": result.downloaded,
                "deleted": result.deleted,
                "failures": result.failures,
                "conflicts": result.conflicts,
            });
            let human = run_human(&result);
            return Ok(Outcome::new("sync.run", data, human)?);
        }
        SyncCommand::Watch {
            root,
            output_dir,
            yes: watch_yes,
        } => {
            let output_dir = required_output_dir(output_dir, context)?;
            let config = sync_config_for(client, root.as_deref(), &output_dir)?;
            confirm(&i18n::text("sync.confirm_run"), *watch_yes, format)?;
            let store = SyncStore::discover(&default_config_dir()?, &config.device_uuid);
            let snapshot = store.load()?.unwrap_or_default();
            // Single PHOTO_SYNC_REQUEST(37) (a second one would be rejected
            // while the phone is SYNCING; no up-front reset — see Run).
            let phone_files = photo_sync_files(client, &config, &snapshot).await?;
            let diff = plan_diff(&phone_files, &snapshot);
            let conflicts = check_conflicts(&diff, &snapshot);
            let (result, mut updated) =
                execute_plan(client, &config, &phone_files, &diff, &snapshot, &conflicts).await?;
            store.save(&updated)?;
            if !result.failures.is_empty() {
                eprintln!(
                    "{}",
                    i18n::format("sync.run_failures", &[&result.failures.len().to_string()])
                );
            }
            // Enter real-time mode; a rejection here is a real error because
            // we just synced (the phone must be in SYNCING state now).
            if !client.sync_monitor(true).await? {
                return Err(Error::Protocol(
                    i18n::text("sync.monitor_rejected").to_string(),
                ));
            }
            let mut events = client.subscribe_events(EventFilter::all());
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Ok(ClientEvent::FileChanged(changes)) => {
                            let mut failures = 0_usize;
                            for change in &changes {
                                match apply_file_change(client, &config, change, &mut updated).await {
                                    Ok(part) => failures += part.failures.len(),
                                    Err(_) => failures += 1,
                                }
                            }
                            let _ = store.save(&updated);
                            if format == OutputFormat::Jsonl {
                                let envelope = sync_watch_envelope(client, changes.len(), failures);
                                println!("{envelope}");
                            } else {
                                println!(
                                    "{}",
                                    i18n::format(
                                        "sync.watch_applied",
                                        &[
                                            &changes.len().to_string(),
                                            &failures.to_string(),
                                        ]
                                    )
                                );
                            }
                            let _ = io::stdout().flush();
                        }
                        Ok(_) => {}
                        Err(EventStreamError::Lagged { missed }) => {
                            eprintln!(
                                "{}",
                                i18n::format("watch.lagged", &[&missed.to_string()])
                            );
                        }
                        Err(EventStreamError::Closed) => {
                            return Err(Error::Transport(
                                i18n::text("watch.disconnected").to_string(),
                            ));
                        }
                    },
                    _ = tokio::signal::ctrl_c() => {
                        let _ = client.sync_monitor(false).await;
                        let _ = store.save(&updated);
                        return Err(Error::Interrupted);
                    }
                }
            }
        }
    }
}

fn sync_device_uuid(client: &HandShakerClient) -> Result<String> {
    let device_uuid = client
        .device_info()
        .phone_id
        .clone()
        .ok_or_else(|| Error::Protocol(i18n::text("sync.device_uuid_missing").to_string()))?;
    if !device_uuid
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err(Error::Protocol(
            i18n::text("sync.device_uuid_invalid").to_string(),
        ));
    }
    Ok(device_uuid)
}

fn required_output_dir(output_dir: &Option<PathBuf>, context: &CommandContext) -> Result<PathBuf> {
    let Some(output_dir) = output_dir else {
        return Err(Error::Usage(
            i18n::text("sync.output_dir_required").to_string(),
        ));
    };
    Ok(resolve_local(&context.local_cwd, output_dir))
}

fn sync_config_for(
    client: &HandShakerClient,
    root: Option<&str>,
    output_dir: &Path,
) -> Result<SyncConfig> {
    let device_uuid = sync_device_uuid(client)?;
    let state = StateStore::discover()?.load_or_create()?;
    Ok(sync_config(
        &device_uuid,
        root.unwrap_or(DEFAULT_SYNC_ROOT),
        &output_dir.display().to_string(),
        &state.host_uuid.to_string(),
    ))
}

/// Build the ledger file list to send as the previous snapshot, then ask the
/// phone for its current state and return the raw photo list.
async fn photo_sync_files(
    client: &HandShakerClient,
    config: &SyncConfig,
    snapshot: &SyncSnapshot,
) -> Result<Vec<RemoteFile>> {
    let ledger: Vec<RemoteFile> = snapshot
        .files
        .iter()
        .map(|(path, record)| RemoteFile {
            path: path.clone(),
            size: record.size,
            created_at: None,
            modified_at: record.modified_at,
            is_directory: false,
            checksum: record.checksum.clone(),
            is_trash: None,
            id: None,
            ext_data: record.ext_data.clone(),
        })
        .collect();
    let result = client.photo_sync(&config.pc_id, &ledger).await?;
    if result.is_success == Some(false) {
        return Err(Error::Protocol(
            i18n::text("sync.photo_sync_rejected").to_string(),
        ));
    }
    // The phone answers with its whole photo library; keep only entries under
    // the configured sync root. Component-boundary match (Path::strip_prefix
    // is segment-wise, so a sibling like DCIM/Camera2 does not match
    // phone_root DCIM/Camera); local_destination re-checks on every use.
    Ok(result
        .files
        .into_iter()
        .filter(|file| {
            Path::new(&file.path)
                .strip_prefix(Path::new(&config.phone_root))
                .is_ok()
        })
        .collect())
}

async fn plan_for(
    client: &HandShakerClient,
    config: &SyncConfig,
    snapshot: &SyncSnapshot,
) -> Result<(SyncDiff, Vec<String>)> {
    let phone_files = photo_sync_files(client, config, snapshot).await?;
    let diff = plan_diff(&phone_files, snapshot);
    let conflicts = check_conflicts(&diff, snapshot);
    Ok((diff, conflicts))
}

fn plan_human(diff: &SyncDiff, conflicts: &[String]) -> String {
    let mut lines = Vec::new();
    lines.push(i18n::format(
        "sync.plan_added",
        &[&diff.added.len().to_string()],
    ));
    if !diff.info_modified.is_empty() {
        lines.push(i18n::format(
            "sync.plan_info",
            &[&diff.info_modified.len().to_string()],
        ));
    }
    if !diff.deleted.is_empty() {
        lines.push(i18n::format(
            "sync.plan_deleted",
            &[&diff.deleted.len().to_string()],
        ));
    }
    if !conflicts.is_empty() {
        lines.push(i18n::format(
            "sync.plan_conflicts",
            &[&conflicts.len().to_string()],
        ));
    }
    lines.join("\n")
}

fn run_human(result: &SyncRunResult) -> String {
    let mut lines = Vec::new();
    lines.push(i18n::format(
        "sync.run_done",
        &[
            &result.downloaded.len().to_string(),
            &result.deleted.len().to_string(),
        ],
    ));
    if !result.failures.is_empty() {
        lines.push(i18n::format(
            "sync.run_failures",
            &[&result.failures.len().to_string()],
        ));
    }
    if !result.conflicts.is_empty() {
        lines.push(i18n::format(
            "sync.plan_conflicts",
            &[&result.conflicts.len().to_string()],
        ));
    }
    lines.join("\n")
}

fn sync_watch_envelope(
    client: &HandShakerClient,
    applied: usize,
    failures: usize,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "ok": true,
        "command": "sync.watch",
        "device": {
            "serial": client.device_info().serial,
            "name": client.device_info().name,
        },
        "event": "sync.watch",
        "data": {
            "applied": applied,
            "failures": failures,
        },
        "warnings": [],
    })
}

/// Strip control characters from a device-controlled string before printing
/// it in human output, so a hostile phone cannot inject terminal escapes.
fn sanitize_human(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !matches!(
                *character,
                // C0 controls, DEL, C1 controls (CSI/OSC) and Unicode bidi
                // overrides (spoofing via RTL reordering).
                '\0'..='\u{1F}'
                    | '\u{7F}'
                    | '\u{80}'..='\u{9F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
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
            let images: Vec<handshaker_core::ImageFile> = targets
                .iter()
                .map(|target| {
                    if target.chars().all(|character| character.is_ascii_digit()) {
                        let media_id = target.parse::<u64>().map_err(|_| {
                            Error::Usage(
                                localizer.format(MessageKey::MediaThumbnailInvalidId, &[target]),
                            )
                        })?;
                        Ok(handshaker_core::ImageFile {
                            media_id: Some(media_id),
                            ..Default::default()
                        })
                    } else {
                        Ok(handshaker_core::ImageFile {
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
    let app = connect(cli).await?;
    let localizer = ZhCn;
    let mut editor = DefaultEditor::new().map_err(|error| {
        Error::LocalIo(i18n::format(
            "shell.readline_init_failed",
            &[&error.to_string()],
        ))
    })?;
    println!("{}", localizer.text(MessageKey::ShellWelcome));
    let client = &app.client;
    let mut context = CommandContext {
        remote_cwd: client.root_path().to_string(),
        local_cwd: env::current_dir()?,
        in_shell: true,
    };
    let mut command_error = None;
    loop {
        let prompt = i18n::format(
            "shell.prompt",
            &[
                &sanitize_human(&client.device_info().serial),
                &sanitize_human(&context.remote_cwd),
            ],
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
                &app,
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
    let close = close_session(app).await;
    println!("{}", localizer.text(MessageKey::ShellBye));
    if let Some(error) = command_error {
        return Err(error);
    }
    close
}

/// Non-interactive batch mode: read one command per line from stdin and run
/// them sequentially on a single persistent connection (heartbeat keeps the
/// session alive; the phone's accessory session stays open until `exit` /
/// `quit` or EOF, avoiding the single-shot re-identify cost of per-command
/// connections). Output follows `--output`; command failures are reported to
/// stderr and the run continues, except transport/handshake/timeout/protocol
/// errors which abort. Exits non-zero when any command failed.
async fn run_batch(cli: &Cli) -> Result<()> {
    let app = connect(cli).await?;
    let localizer = ZhCn;
    let client = &app.client;
    let mut context = CommandContext {
        remote_cwd: client.root_path().to_string(),
        local_cwd: env::current_dir()?,
        in_shell: true,
    };
    let mut failed = 0u32;
    let mut fatal = None;
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(line) = lines.next() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                // Abort-class: no in-loop print (main.rs renders once), and
                // the unified close below still runs.
                fatal = Some(Error::LocalIo(
                    localizer.format(MessageKey::BatchReadFailed, &[&error.to_string()]),
                ));
                break;
            }
        };
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
                failed += 1;
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
                failed += 1;
                continue;
            }
        };
        if matches!(parsed.command, Command::Shell | Command::Batch) {
            eprintln!(
                "{}",
                localizer.format(
                    MessageKey::Error,
                    &[localizer.text(MessageKey::ShellNested)],
                )
            );
            failed += 1;
            continue;
        }
        match execute_connected(&parsed.command, &app, &context, parsed.yes, cli.output).await {
            Ok(outcome) => {
                if let Err(error) = render(&outcome, cli.output) {
                    // Output write failure (e.g. EPIPE when piping stdout to
                    // head): abort via the unified close + fatal path.
                    fatal = Some(error);
                    break;
                }
            }
            Err(error) => {
                let fatal_class = matches!(
                    error,
                    Error::Transport(_)
                        | Error::Handshake(_)
                        | Error::Timeout(_)
                        | Error::Protocol(_)
                );
                // main.rs renders the returned error; only print in-loop for
                // per-command failures that continue.
                if !fatal_class {
                    eprintln!(
                        "{}",
                        localizer.format(MessageKey::Error, &[&error.to_string()])
                    );
                }
                if fatal_class {
                    fatal = Some(error);
                    break;
                }
                failed += 1;
            }
        }
    }
    let close = close_session(app).await;
    // Abort-class errors (transport/handshake/timeout/protocol, stdin or
    // render failures) take precedence over the per-command failure summary so
    // scripts can tell an aborted run from a completed one.
    if let Some(error) = fatal {
        return Err(error);
    }
    if failed > 0 {
        eprintln!(
            "{}",
            localizer.format(MessageKey::BatchFailures, &[&failed.to_string()])
        );
        return Err(Error::LocalIo(
            localizer.format(MessageKey::BatchFailures, &[&failed.to_string()]),
        ));
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
    Ok(base.join(
        remote_name(remote)
            .ok_or_else(|| Error::Usage(ZhCn.format(MessageKey::RemoteNameMissing, &[remote])))?,
    ))
}

/// Last path component of a remote path, if any.
fn remote_name(remote: &str) -> Option<String> {
    remote
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Batch options mirroring the CLI's overwrite switch and (for human output)
/// a per-file progress line.
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

fn human_device_info(info: &handshaker_core::DeviceInfo) -> String {
    let localizer = ZhCn;
    let battery = info
        .battery_percentage
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "-".to_string());
    localizer.format(
        MessageKey::DeviceInfo,
        &[
            &sanitize_human(&info.serial),
            &sanitize_human(info.name.as_deref().unwrap_or("-")),
            &sanitize_human(info.model.as_deref().unwrap_or("-")),
            &sanitize_human(info.brand.as_deref().unwrap_or("-")),
            &sanitize_human(info.smartisan_version.as_deref().unwrap_or("-")),
            &sanitize_human(info.apk_version_name.as_deref().unwrap_or("-")),
            &sanitize_human(&info.root_path),
            &battery,
            if info.phone_locked.unwrap_or(false) {
                localizer.text(MessageKey::Yes)
            } else {
                localizer.text(MessageKey::No)
            },
        ],
    )
}

fn human_files(files: &[CliFileEntry]) -> String {
    let localizer = ZhCn;
    let mut lines = vec![localizer.text(MessageKey::FileListHeader).to_string()];
    lines.extend(files.iter().map(human_file));
    lines.join("\n")
}

fn human_file(file: &CliFileEntry) -> String {
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

fn human_clipboards(entries: &[handshaker_core::ClipboardEntry]) -> String {
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
    fn cli_file_entry_mirrors_remote_file_json_contract() {
        let entry = FileEntryDto {
            path: "/storage/emulated/0/a.txt".to_string(),
            size: 7,
            created_at_ms: Some(1000),
            modified_at_ms: Some(2000),
            is_directory: false,
            checksum: Some("abc".to_string()),
            is_trash: Some(false),
            media_id: Some(42),
        };
        let cli = cli_file_entry(&entry);
        // The CLI JSON keys must match the legacy core RemoteFile contract
        // (path/size/created_at/modified_at/is_directory/checksum/is_trash/
        // id/ext_data) so migrating commands stay byte-compatible.
        let value = serde_json::to_value(&cli).expect("serialize");
        let object = value.as_object().expect("object");
        let expected: std::collections::BTreeSet<&str> = [
            "path",
            "size",
            "created_at",
            "modified_at",
            "is_directory",
            "checksum",
            "is_trash",
            "id",
            "ext_data",
        ]
        .into_iter()
        .collect();
        let actual: std::collections::BTreeSet<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(actual, expected);
        assert_eq!(cli.id, Some(42));
        assert_eq!(cli.created_at, Some(1000));
        assert_eq!(cli.modified_at, Some(2000));
        assert!(cli.ext_data.is_none());
    }

    #[test]
    fn collect_local_tree_counts_files_dirs_and_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("src");
        std::fs::create_dir_all(root.join("sub")).expect("create dirs");
        std::fs::write(root.join("a.txt"), b"hello").expect("write a");
        std::fs::write(root.join("sub").join("b.bin"), b"1234567890").expect("write b");

        let mut items = Vec::new();
        let mut dirs = std::collections::BTreeSet::new();
        let mut bytes = 0_u64;
        collect_local_tree(
            &root,
            "/remote/base",
            &mut items,
            &mut dirs,
            &mut bytes,
            &ZhCn,
        )
        .expect("scan");

        assert_eq!(items.len(), 2, "one file per level");
        assert_eq!(dirs.len(), 1, "sub directory recorded");
        assert!(dirs.contains("/remote/base/sub"));
        assert_eq!(bytes, 15, "5 + 10 bytes");
        let targets: Vec<_> = items.iter().map(|item| item.target.clone()).collect();
        assert!(targets.contains(&"/remote/base/a.txt".to_string()));
        assert!(targets.contains(&"/remote/base/sub/b.bin".to_string()));
    }

    #[test]
    fn pull_target_misparsed_detects_missing_separator() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local_cwd = temp.path().join("lcd");
        std::fs::create_dir_all(&local_cwd).expect("create lcd dir");
        let existing = local_cwd.join("out.txt");
        std::fs::write(&existing, b"x").expect("write");
        // A file with the same name in the process cwd but not under local_cwd.
        let cwd_only = temp.path().join("cwd-only.txt");
        std::fs::write(&cwd_only, b"x").expect("write");

        // Two positionals with an existing local second arg -> misparse.
        assert!(pull_target_misparsed(
            &[
                "/storage/emulated/0/a.txt".to_string(),
                "out.txt".to_string(),
            ],
            None,
            &local_cwd,
        ));
        // Second arg does not exist under local_cwd -> cannot tell, keep old
        // behavior (even if the process cwd has a same-named file).
        assert!(!pull_target_misparsed(
            &[
                "/storage/emulated/0/a.txt".to_string(),
                "cwd-only.txt".to_string(),
            ],
            None,
            &local_cwd,
        ));
        // Explicit `--` LOCAL -> fine.
        assert!(!pull_target_misparsed(
            &["/storage/emulated/0/a.txt".to_string()],
            Some(&existing),
            &local_cwd,
        ));
        // Three remotes (multi-source to cwd) -> fine.
        assert!(!pull_target_misparsed(
            &["/a".to_string(), "/b".to_string(), "out.txt".to_string(),],
            None,
            &local_cwd,
        ));
    }

    #[test]
    fn dry_run_report_serializes_with_dry_run_flag() {
        let report = DryRunReport {
            files: 3,
            dirs: 1,
            bytes: 42,
            dry_run: true,
        };
        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(json["files"], 3);
        assert_eq!(json["dirs"], 1);
        assert_eq!(json["bytes"], 42);
        assert_eq!(json["dry_run"], true);
    }

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
            vec!["fs", "pull", "/sdcard/a", "--", "/tmp/a"],
            vec!["fs", "push", "/tmp/a", "--", "/sdcard/a"],
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
            "--overwrite",
            "--",
            "/tmp/a",
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
            "--",
            "/sdcard/remote.txt",
        ])
        .expect("parse push");
        match cli.command {
            Command::Fs(FsCommand::Push { local, remote, .. }) => {
                assert_eq!(local, vec![PathBuf::from("/tmp/local.txt")]);
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
        let event = ClientEvent::ClipboardChanged(vec![handshaker_core::ClipboardEntry {
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
