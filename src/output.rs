use std::io::{self, Write};

use serde::Serialize;
use serde_json::{Value, json};

use handshaker_rust::{
    DeviceInfo, Error, Result,
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

    pub fn with_warning(mut self, warning: String) -> Self {
        self.warnings.push(warning);
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
            let envelope = success_envelope(outcome);
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
            let envelope = error_envelope(error, command);
            println!("{}", envelope);
        }
    }
}

pub(crate) fn render_batch_progress(
    command: &str,
    info: &DeviceInfo,
    progress: &handshaker_rust::BatchTransferProgress,
    format: OutputFormat,
) {
    match format {
        OutputFormat::Human => {
            let percent = if progress.total == 0 {
                100
            } else {
                (progress.done * 100 / progress.total) as u64
            };
            eprint!(
                "\r{}",
                ZhCn.format(
                    MessageKey::BatchProgress,
                    &[
                        &progress.done.to_string(),
                        &progress.total.to_string(),
                        &percent.to_string(),
                    ],
                )
            );
            if progress.done >= progress.total {
                eprintln!();
            }
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
        }
    }
}

pub(crate) fn success_envelope(outcome: &Outcome) -> Value {
    json!({
        "schema_version": 1,
        "ok": true,
        "command": outcome.command,
        "device": outcome.device,
        "data": outcome.data,
        "warnings": outcome.warnings,
    })
}

pub(crate) fn error_envelope(error: &Error, command: &str) -> Value {
    json!({
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use handshaker_rust::ErrorCode;

    #[test]
    fn success_envelope_has_stable_schema_v1_shape() {
        let outcome =
            Outcome::new("fs.ls", vec!["/sdcard/a"], "human".to_string()).expect("outcome");
        let value = success_envelope(&outcome);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["command"], "fs.ls");
        assert_eq!(value["data"][0], "/sdcard/a");
        assert!(value.get("warnings").is_some());
        assert_eq!(value.as_object().expect("object").len(), 6);
    }

    #[test]
    fn error_envelope_has_stable_code_and_details() {
        let error = Error::Usage("bad arguments".to_string());
        let value = error_envelope(&error, "fs.ls");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], false);
        assert_eq!(value["command"], "fs.ls");
        assert_eq!(value["error"]["code"], serde_json::json!(ErrorCode::Usage));
        assert!(value["error"]["message"].as_str().is_some());
        assert!(value["error"].get("details").is_some());
    }
}
