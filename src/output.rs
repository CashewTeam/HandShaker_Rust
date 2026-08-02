use std::io::{self, Write};

use serde::Serialize;
use serde_json::{Value, json};

use handshaker_rust::{
    DeviceInfo, Error, Result, TransferDirection, TransferProgress,
    i18n::{self, Localizer, MessageKey, ZhCn},
};

use crate::cli::OutputFormat;

#[derive(Debug, Serialize)]
pub(crate) struct DeviceSummary {
    pub serial: String,
    pub name: Option<String>,
}

pub(crate) struct Outcome {
    pub command: &'static str,
    pub device: Option<DeviceSummary>,
    pub data: Value,
    pub human: String,
    pub warnings: Vec<String>,
}

impl Outcome {
    pub fn new<T: Serialize>(command: &'static str, data: T, human: String) -> Result<Self> {
        Ok(Self {
            command,
            device: None,
            data: serde_json::to_value(data).map_err(|error| {
                Error::Protocol(i18n::format(
                    "error.serialize_output",
                    &[&error.to_string()],
                ))
            })?,
            human,
            warnings: Vec::new(),
        })
    }

    pub fn with_device(mut self, info: &DeviceInfo) -> Self {
        self.device = Some(DeviceSummary {
            serial: info.serial.clone(),
            name: info.name.clone(),
        });
        self
    }
}

pub(crate) fn render(outcome: &Outcome, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Human => {
            if !outcome.human.is_empty() {
                println!("{}", outcome.human);
            }
            for warning in &outcome.warnings {
                eprintln!("{warning}");
            }
        }
        OutputFormat::Json | OutputFormat::Jsonl => {
            let envelope = json!({
                "schema_version": 1,
                "ok": true,
                "command": outcome.command,
                "device": outcome.device,
                "data": outcome.data,
                "warnings": outcome.warnings,
            });
            println!(
                "{}",
                serde_json::to_string(&envelope).map_err(|error| {
                    Error::Protocol(i18n::format("error.serialize_json", &[&error.to_string()]))
                })?
            );
        }
    }
    io::stdout().flush()?;
    Ok(())
}

pub(crate) fn render_error(error: &Error, command: &str, format: OutputFormat) {
    match format {
        OutputFormat::Human => {
            eprintln!("{}", ZhCn.format(MessageKey::Error, &[&error.to_string()]));
        }
        OutputFormat::Json | OutputFormat::Jsonl => {
            let envelope = json!({
                "schema_version": 1,
                "ok": false,
                "command": command,
                "device": null,
                "error": {
                    "code": error.code(),
                    "message": error.to_string(),
                    "details": null
                },
                "warnings": []
            });
            println!("{}", envelope);
        }
    }
}

pub(crate) fn render_progress(
    command: &str,
    info: &DeviceInfo,
    progress: &TransferProgress,
    format: OutputFormat,
) {
    match format {
        OutputFormat::Human => {
            let action = match progress.direction {
                TransferDirection::Download => ZhCn.text(MessageKey::Download),
                TransferDirection::Upload => ZhCn.text(MessageKey::Upload),
            };
            let percent = progress_percent(progress);
            eprint!(
                "\r{}",
                ZhCn.format(
                    MessageKey::Progress,
                    &[
                        action,
                        &progress.transferred.to_string(),
                        &progress.total.to_string(),
                        &percent.to_string(),
                    ],
                )
            );
            if progress.transferred >= progress.total {
                eprintln!();
            }
            let _ = io::stderr().flush();
        }
        OutputFormat::Json => {}
        OutputFormat::Jsonl => {
            let envelope = json!({
                "schema_version": 1,
                "ok": true,
                "command": command,
                "device": DeviceSummary {
                    serial: info.serial.clone(),
                    name: info.name.clone(),
                },
                "event": "progress",
                "data": progress,
                "warnings": [],
            });
            println!("{envelope}");
            let _ = io::stdout().flush();
        }
    }
}

pub(crate) fn progress_percent(progress: &TransferProgress) -> u64 {
    progress
        .transferred
        .saturating_mul(100)
        .checked_div(progress.total)
        .unwrap_or(100)
}
