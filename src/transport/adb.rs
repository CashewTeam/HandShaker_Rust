use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{Instant, sleep, timeout_at};

use crate::domain::AdbDevice;
use crate::error::{Error, Result};
use crate::transport::{ConnectedTransport, TransportConnector};

const PHONE_PORT: u16 = 10086;
const SERVICE_COMPONENT: &str = "com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService";

pub(crate) struct AdbConnector {
    adb_path: PathBuf,
    requested_serial: Option<String>,
    timeout: Duration,
}

impl AdbConnector {
    pub fn new(adb_path: PathBuf, requested_serial: Option<String>, timeout: Duration) -> Self {
        Self {
            adb_path,
            requested_serial,
            timeout,
        }
    }
}

#[async_trait]
impl TransportConnector for AdbConnector {
    async fn connect(&self) -> Result<ConnectedTransport> {
        let devices = list_devices_with_timeout(&self.adb_path, self.timeout).await?;
        let device = select_device(devices, self.requested_serial.as_deref())?;

        run_adb(
            &self.adb_path,
            &[
                "-s",
                &device.serial,
                "shell",
                "am",
                "startservice",
                "--user",
                "0",
                "-n",
                SERVICE_COMPONENT,
                "--ei",
                "ADB_PORT",
                &PHONE_PORT.to_string(),
            ],
            self.timeout,
        )
        .await?;

        let before = list_forward_ports(&self.adb_path, &device.serial, self.timeout).await?;
        let output = run_adb(
            &self.adb_path,
            &[
                "-s",
                &device.serial,
                "forward",
                "tcp:0",
                &format!("tcp:{PHONE_PORT}"),
            ],
            self.timeout,
        )
        .await?;
        let port = match output.trim().parse::<u16>() {
            Ok(port) => port,
            Err(error) => {
                cleanup_new_forwards(&self.adb_path, &device.serial, &before, self.timeout).await?;
                return Err(Error::Transport(format!(
                    "无法解析 adb 分配的本地端口 {:?}：{error}",
                    output.trim()
                )));
            }
        };
        let cleanup = AdbForward::new(
            self.adb_path.clone(),
            device.serial.clone(),
            port,
            self.timeout,
        );

        let deadline = Instant::now() + self.timeout;
        let stream = loop {
            match TcpStream::connect(("127.0.0.1", port)).await {
                Ok(stream) => break stream,
                Err(error) if Instant::now() < deadline => {
                    tracing::debug!(%error, port, "等待 ADB 转发端口就绪");
                    sleep(Duration::from_millis(100)).await;
                }
                Err(error) => {
                    return Err(Error::Transport(format!(
                        "连接 ADB 转发端口 127.0.0.1:{port} 失败：{error}"
                    )));
                }
            }
        };
        stream
            .set_nodelay(true)
            .map_err(|error| Error::Transport(format!("设置 TCP_NODELAY 失败：{error}")))?;

        Ok(ConnectedTransport {
            stream,
            device,
            cleanup,
        })
    }
}

pub(crate) async fn list_devices(adb_path: &Path) -> Result<Vec<AdbDevice>> {
    list_devices_with_timeout(adb_path, Duration::from_secs(30)).await
}

pub(crate) async fn list_devices_with_timeout(
    adb_path: &Path,
    command_timeout: Duration,
) -> Result<Vec<AdbDevice>> {
    let output = run_adb(adb_path, &["devices", "-l"], command_timeout).await?;
    Ok(parse_devices(&output))
}

fn parse_devices(output: &str) -> Vec<AdbDevice> {
    let mut devices = Vec::new();
    for line in output.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else { continue };
        let state = parts.next().unwrap_or("unknown");
        let mut product = None;
        let mut model = None;
        let mut device = None;
        for part in parts {
            if let Some(value) = part.strip_prefix("product:") {
                product = Some(value.to_string());
            } else if let Some(value) = part.strip_prefix("model:") {
                model = Some(value.to_string());
            } else if let Some(value) = part.strip_prefix("device:") {
                device = Some(value.to_string());
            }
        }
        devices.push(AdbDevice {
            serial: serial.to_string(),
            state: state.to_string(),
            product,
            model,
            device,
        });
    }
    devices
}

fn select_device(devices: Vec<AdbDevice>, requested: Option<&str>) -> Result<AdbDevice> {
    let online: Vec<_> = devices
        .into_iter()
        .filter(|device| device.state == "device")
        .collect();
    if let Some(serial) = requested {
        return online
            .into_iter()
            .find(|device| device.serial == serial)
            .ok_or_else(|| Error::DeviceSelection(format!("设备 {serial} 不在线或不存在")));
    }
    match online.len() {
        0 => Err(Error::DeviceSelection("没有在线的 ADB 设备".to_string())),
        1 => Ok(online.into_iter().next().expect("one device")),
        count => Err(Error::DeviceSelection(format!(
            "检测到 {count} 台在线设备，请使用 --serial 指定"
        ))),
    }
}

async fn run_adb(adb_path: &Path, args: &[&str], command_timeout: Duration) -> Result<String> {
    let mut child = Command::new(adb_path)
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::AdbUnavailable(format!("{}：{error}", adb_path.display())))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let deadline = Instant::now() + command_timeout;
    let status = match timeout_at(deadline, child.wait()).await {
        Ok(result) => {
            result.map_err(|error| Error::Transport(format!("等待 adb 子进程失败：{error}")))?
        }
        Err(_) => {
            stdout_task.abort();
            stderr_task.abort();
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(Error::Timeout(format!("adb {}", args.join(" "))));
        }
    };
    let outputs = timeout_at(deadline, async {
        let stdout = (&mut stdout_task).await;
        let stderr = (&mut stderr_task).await;
        (stdout, stderr)
    })
    .await;
    let (stdout, stderr) = match outputs {
        Ok(outputs) => outputs,
        Err(_) => {
            stdout_task.abort();
            stderr_task.abort();
            return Err(Error::Timeout(format!("收集 adb {} 输出", args.join(" "))));
        }
    };
    let stdout = stdout
        .map_err(|error| Error::Transport(format!("读取 adb stdout 失败：{error}")))?
        .map_err(|error| Error::Transport(format!("读取 adb stdout 失败：{error}")))?;
    let stderr = stderr
        .map_err(|error| Error::Transport(format!("读取 adb stderr 失败：{error}")))?
        .map_err(|error| Error::Transport(format!("读取 adb stderr 失败：{error}")))?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
        return Err(Error::Transport(if stderr.is_empty() {
            stdout
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&stdout).to_string())
}

async fn list_forward_ports(
    adb_path: &Path,
    serial: &str,
    command_timeout: Duration,
) -> Result<HashSet<u16>> {
    let output = run_adb(
        adb_path,
        &["-s", serial, "forward", "--list"],
        command_timeout,
    )
    .await?;
    Ok(parse_forward_ports(&output, serial))
}

fn parse_forward_ports(output: &str, serial: &str) -> HashSet<u16> {
    let remote_port = format!("tcp:{PHONE_PORT}");
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let listed_serial = fields.next()?;
            let local = fields.next()?;
            let remote = fields.next()?;
            if listed_serial != serial || remote != remote_port {
                return None;
            }
            local.strip_prefix("tcp:")?.parse().ok()
        })
        .collect()
}

async fn cleanup_new_forwards(
    adb_path: &Path,
    serial: &str,
    before: &HashSet<u16>,
    command_timeout: Duration,
) -> Result<()> {
    let after = list_forward_ports(adb_path, serial, command_timeout).await?;
    let Some(port) = unique_added_forward(before, &after)? else {
        return Ok(());
    };
    run_adb(
        adb_path,
        &["-s", serial, "forward", "--remove", &format!("tcp:{port}")],
        command_timeout,
    )
    .await?;
    Ok(())
}

fn unique_added_forward(before: &HashSet<u16>, after: &HashSet<u16>) -> Result<Option<u16>> {
    let added: Vec<_> = after.difference(before).copied().collect();
    let [port] = added.as_slice() else {
        if added.is_empty() {
            return Ok(None);
        }
        return Err(Error::Transport(format!(
            "动态端口输出无效，且检测到 {} 个并发新增 forward；为避免误删未自动清理",
            added.len()
        )));
    };
    Ok(Some(*port))
}

pub(crate) struct AdbForward {
    adb_path: PathBuf,
    serial: String,
    local_port: u16,
    command_timeout: Duration,
    cleaned: bool,
}

impl AdbForward {
    fn new(adb_path: PathBuf, serial: String, local_port: u16, command_timeout: Duration) -> Self {
        Self {
            adb_path,
            serial,
            local_port,
            command_timeout,
            cleaned: false,
        }
    }

    pub async fn cleanup(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        run_adb(
            &self.adb_path,
            &[
                "-s",
                &self.serial,
                "forward",
                "--remove",
                &format!("tcp:{}", self.local_port),
            ],
            self.command_timeout,
        )
        .await?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for AdbForward {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let child = std::process::Command::new(&self.adb_path)
            .args([
                "-s",
                &self.serial,
                "forward",
                "--remove",
                &format!("tcp:{}", self.local_port),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            let deadline = std::time::Instant::now() + self.command_timeout;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if std::time::Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
        self.cleaned = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn selects_only_online_device() {
        let selected = select_device(
            vec![AdbDevice {
                serial: "abc".into(),
                state: "device".into(),
                product: None,
                model: None,
                device: None,
            }],
            None,
        )
        .expect("selected");
        assert_eq!(selected.serial, "abc");
    }

    #[test]
    fn refuses_ambiguous_device_selection() {
        let device = |serial: &str| AdbDevice {
            serial: serial.into(),
            state: "device".into(),
            product: None,
            model: None,
            device: None,
        };
        assert!(select_device(vec![device("a"), device("b")], None).is_err());
    }

    #[test]
    fn parses_adb_devices_long_output() {
        let output = "List of devices attached\nABC123 device product:osborn model:DE106 device:osborn transport_id:1\nOFF offline transport_id:2\n\n";
        let devices = parse_devices(output);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].serial, "ABC123");
        assert_eq!(devices[0].model.as_deref(), Some("DE106"));
        assert_eq!(devices[1].state, "offline");
    }

    #[test]
    fn parses_only_matching_ssp_forward_ports() {
        let output =
            "ABC123 tcp:41001 tcp:10086\nOTHER tcp:41002 tcp:10086\nABC123 tcp:41003 tcp:5555\n";
        assert_eq!(
            parse_forward_ports(output, "ABC123"),
            HashSet::from([41001])
        );
    }

    #[test]
    fn ambiguous_new_forwards_are_never_selected_for_cleanup() {
        let before = HashSet::from([41000]);
        let after = HashSet::from([41000, 41001, 41002]);
        assert!(unique_added_forward(&before, &after).is_err());
        assert_eq!(
            unique_added_forward(&before, &HashSet::from([41000, 41001])).unwrap(),
            Some(41001)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_adb_dynamic_forward_is_cleaned_exactly() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let port = listener.local_addr().expect("address").port();
        let temp = tempfile::tempdir().expect("temporary directory");
        let adb = temp.path().join("adb");
        let log = temp.path().join("calls.log");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n  'devices -l') printf 'List of devices attached\\nABC123 device product:osborn model:DE106 device:osborn\\n' ;;\n  *'forward tcp:0 tcp:10086') printf '{}\\n' ;;\nesac\n",
            log.display(),
            port,
        );
        std::fs::write(&adb, script).expect("fake adb");
        let mut permissions = std::fs::metadata(&adb).expect("metadata").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&adb, permissions).expect("permissions");

        let accept = tokio::spawn(async move { listener.accept().await.expect("accept") });
        let connector = AdbConnector::new(adb, None, Duration::from_secs(5));
        let mut connected = connector.connect().await.expect("connect");
        assert_eq!(connected.device.serial, "ABC123");
        let _ = accept.await.expect("accept task");
        connected.cleanup.cleanup().await.expect("cleanup");
        drop(connected.stream);

        let calls = std::fs::read_to_string(log).expect("calls");
        assert!(calls.contains("-s ABC123 shell am startservice --user 0 -n com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService --ei ADB_PORT 10086"));
        assert!(calls.contains("-s ABC123 forward tcp:0 tcp:10086"));
        assert!(calls.contains(&format!("-s ABC123 forward --remove tcp:{port}")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_dynamic_port_output_cleans_only_new_forward() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let adb = temp.path().join("adb");
        let log = temp.path().join("calls.log");
        let marker = temp.path().join("forward-created");
        std::fs::write(&log, "").expect("call log");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n  'devices -l') printf 'List of devices attached\\nABC123 device product:osborn model:DE106 device:osborn\\n' ;;\n  *'forward --list') if [ -f '{}' ]; then printf 'ABC123 tcp:45678 tcp:10086\\n'; fi ;;\n  *'forward tcp:0 tcp:10086') touch '{}'; printf 'not-a-port\\n' ;;\nesac\n",
            log.display(),
            marker.display(),
            marker.display(),
        );
        std::fs::write(&adb, script).expect("fake adb");
        let mut permissions = std::fs::metadata(&adb).expect("metadata").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&adb, permissions).expect("permissions");

        let connector = AdbConnector::new(adb, None, Duration::from_secs(5));
        assert!(connector.connect().await.is_err());
        let calls = std::fs::read_to_string(log).expect("calls");
        assert!(calls.contains("-s ABC123 forward --remove tcp:45678"));
        assert!(!calls.contains("forward --remove-all"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adb_subprocess_respects_timeout() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let adb = temp.path().join("adb");
        std::fs::write(&adb, "#!/bin/sh\nexec sleep 2\n").expect("fake adb");
        let mut permissions = std::fs::metadata(&adb).expect("metadata").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&adb, permissions).expect("permissions");

        let started = std::time::Instant::now();
        let error = run_adb(&adb, &["devices", "-l"], Duration::from_millis(50))
            .await
            .expect_err("timeout");
        assert!(matches!(error, Error::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adb_output_collection_uses_same_deadline() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let adb = temp.path().join("adb");
        std::fs::write(&adb, "#!/bin/sh\nsleep 2 &\nexit 0\n").expect("fake adb");
        let mut permissions = std::fs::metadata(&adb).expect("metadata").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&adb, permissions).expect("permissions");

        let started = std::time::Instant::now();
        let error = run_adb(&adb, &["devices", "-l"], Duration::from_millis(50))
            .await
            .expect_err("output timeout");
        assert!(matches!(error, Error::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
