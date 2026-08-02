use serde::Serialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Usage,
    Configuration,
    DeviceSelection,
    AdbUnavailable,
    Transport,
    Handshake,
    Timeout,
    Protocol,
    RemoteIo,
    LocalIo,
    ConfirmationRequired,
    Interrupted,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("参数错误：{0}")]
    Usage(String),
    #[error("配置错误：{0}")]
    Configuration(String),
    #[error("设备选择失败：{0}")]
    DeviceSelection(String),
    #[error("无法运行 adb：{0}")]
    AdbUnavailable(String),
    #[error("连接失败：{0}")]
    Transport(String),
    #[error("握手失败：{0}")]
    Handshake(String),
    #[error("操作超时：{0}")]
    Timeout(String),
    #[error("协议错误：{0}")]
    Protocol(String),
    #[error("手机端操作失败（代码 {code:?}）：{message}")]
    RemoteIo { code: Option<i32>, message: String },
    #[error("本地 I/O 错误：{0}")]
    LocalIo(String),
    #[error("此操作需要明确确认：{0}")]
    ConfirmationRequired(String),
    #[error("操作已中断")]
    Interrupted,
}

impl Error {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Usage(_) => ErrorCode::Usage,
            Self::Configuration(_) => ErrorCode::Configuration,
            Self::DeviceSelection(_) => ErrorCode::DeviceSelection,
            Self::AdbUnavailable(_) => ErrorCode::AdbUnavailable,
            Self::Transport(_) => ErrorCode::Transport,
            Self::Handshake(_) => ErrorCode::Handshake,
            Self::Timeout(_) => ErrorCode::Timeout,
            Self::Protocol(_) => ErrorCode::Protocol,
            Self::RemoteIo { .. } => ErrorCode::RemoteIo,
            Self::LocalIo(_) => ErrorCode::LocalIo,
            Self::ConfirmationRequired(_) => ErrorCode::ConfirmationRequired,
            Self::Interrupted => ErrorCode::Interrupted,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Configuration(_) | Self::DeviceSelection(_) | Self::AdbUnavailable(_) => 3,
            Self::Transport(_) | Self::Handshake(_) | Self::Timeout(_) => 4,
            Self::Protocol(_) => 5,
            Self::RemoteIo { .. } => 6,
            Self::LocalIo(_) => 7,
            Self::ConfirmationRequired(_) => 8,
            Self::Interrupted => 130,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::LocalIo(value.to_string())
    }
}

impl From<prost::DecodeError> for Error {
    fn from(value: prost::DecodeError) -> Self {
        Self::Protocol(format!("protobuf 解码失败：{value}"))
    }
}
