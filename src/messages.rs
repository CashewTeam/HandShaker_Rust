#[derive(Debug, Clone, Copy)]
pub(crate) enum MessageKey {
    WireLogWarning,
    ConfirmPrompt,
    NoDevices,
    DeviceListHeader,
    FileListHeader,
    ClipboardHeader,
    ShellWelcome,
    ShellBye,
    Yes,
    No,
    ShellOnlyHuman,
    PingResult,
    FileCount,
    Exists,
    Missing,
    DirectoryCreated,
    RenameDone,
    DeletedCount,
    DownloadDone,
    UploadDone,
    ClipboardWritten,
    ClipboardDeleted,
    ClipboardCleared,
    ShellNoStdin,
    ClipboardSetRequired,
    ShellNested,
    ShellHelp,
    Error,
    CommandParseError,
    RemoteNotDirectory,
    LocalNotDirectory,
    ConfirmationRequired,
    UserNotConfirmed,
    Download,
    Upload,
    Progress,
    Directory,
    File,
    DeviceInfo,
    RemoteMissing,
    DeleteRecursiveRequired,
    DeleteAction,
    LocalTargetExists,
    OverwriteLocalAction,
    RemoteTargetExists,
    OverwriteRemoteAction,
    DeleteClipboardAction,
    ClearClipboardAction,
    RemoteNameMissing,
    InvalidDuration,
}

pub(crate) trait Localizer {
    fn text(&self, key: MessageKey) -> &'static str;

    fn format(&self, key: MessageKey, arguments: &[&str]) -> String {
        let mut message = self.text(key).to_string();
        for (index, argument) in arguments.iter().enumerate() {
            message = message.replace(&format!("{{{index}}}"), argument);
        }
        message
    }
}

pub(crate) struct ZhCn;

impl Localizer for ZhCn {
    fn text(&self, key: MessageKey) -> &'static str {
        match key {
            MessageKey::WireLogWarning => "警告：线路日志可能包含文件内容和剪贴板数据。",
            MessageKey::ConfirmPrompt => "确认执行？[y/N] ",
            MessageKey::NoDevices => "未发现 ADB 设备。",
            MessageKey::DeviceListHeader => "序列号\t状态\t型号\t设备",
            MessageKey::FileListHeader => "类型\t大小\t修改时间\t路径",
            MessageKey::ClipboardHeader => "时间戳(ms)\t内容",
            MessageKey::ShellWelcome => "已进入 HandShaker 会话；输入 help 查看命令，Ctrl-D 退出。",
            MessageKey::ShellBye => "连接已关闭。",
            MessageKey::Yes => "是",
            MessageKey::No => "否",
            MessageKey::ShellOnlyHuman => "shell 仅支持 human 输出",
            MessageKey::PingResult => "往返延迟：{0} ms",
            MessageKey::FileCount => "文件数量：{0}",
            MessageKey::Exists => "存在",
            MessageKey::Missing => "不存在",
            MessageKey::DirectoryCreated => "已创建目录：{0}",
            MessageKey::RenameDone => "重命名完成",
            MessageKey::DeletedCount => "已处理 {0} 个路径",
            MessageKey::DownloadDone => "下载完成：{0} 字节",
            MessageKey::UploadDone => "上传完成：{0} 字节",
            MessageKey::ClipboardWritten => "剪贴板已写入",
            MessageKey::ClipboardDeleted => "剪贴板条目已删除",
            MessageKey::ClipboardCleared => "剪贴板已清空",
            MessageKey::ShellNoStdin => "shell 中不能使用 clipboard set --stdin",
            MessageKey::ClipboardSetRequired => "clipboard set 需要 TEXT 或 --stdin",
            MessageKey::ShellNested => "shell 中不能再次进入 shell",
            MessageKey::ShellHelp => {
                "device/fs/clipboard 命令与一次性模式相同；内建命令：pwd cd lpwd lcd help exit"
            }
            MessageKey::Error => "错误：{0}",
            MessageKey::CommandParseError => "错误：无法解析命令：{0}",
            MessageKey::RemoteNotDirectory => "错误：{0} 不是目录或不存在",
            MessageKey::LocalNotDirectory => "错误：{0} 不是本地目录",
            MessageKey::ConfirmationRequired => "{0}；请添加 --yes",
            MessageKey::UserNotConfirmed => "用户未确认操作",
            MessageKey::Download => "下载",
            MessageKey::Upload => "上传",
            MessageKey::Progress => "{0}进度：{1} / {2} 字节（{3}%）",
            MessageKey::Directory => "目录",
            MessageKey::File => "文件",
            MessageKey::DeviceInfo => {
                "序列号：{0}\n名称：{1}\n型号：{2}\n品牌：{3}\n系统版本：{4}\nAPK：{5}\n根目录：{6}\n电量：{7}\n锁屏：{8}"
            }
            MessageKey::RemoteMissing => "远端路径 {0} 不存在",
            MessageKey::DeleteRecursiveRequired => "删除目录 {0} 需要 --recursive",
            MessageKey::DeleteAction => "将删除 {0} 个远端路径",
            MessageKey::LocalTargetExists => "本地目标 {0} 已存在，请使用 --overwrite",
            MessageKey::OverwriteLocalAction => "将覆盖本地文件 {0}",
            MessageKey::RemoteTargetExists => "远端目标 {0} 已存在，请使用 --overwrite",
            MessageKey::OverwriteRemoteAction => "将覆盖远端文件 {0}",
            MessageKey::DeleteClipboardAction => "将删除剪贴板条目 {0}",
            MessageKey::ClearClipboardAction => "将清空手机剪贴板",
            MessageKey::RemoteNameMissing => "远端路径缺少文件名：{0}",
            MessageKey::InvalidDuration => "无效时长 {0}：{1}",
        }
    }
}
