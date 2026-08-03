# HandShaker_Rust Agent 协作指南

本文件适用于仓库根目录及其全部子目录。开始工作前先阅读本文件；若子目录后续出现更具体的
`AGENTS.md`，则以离目标文件最近的规则为准。

## 1. 项目定位

本仓库实现兼容原版 Smartisan HandShaker 的跨平台 Rust 后端与 `handshaker` CLI，并保存 SSP
（SmartSync Protocol）的逆向研究和真实抓包证据。

当前首版范围：

- ADB 通道；
- 设备信息和连通性；
- 文件查询、创建、重命名和删除；
- 单文件上传、下载；
- 剪贴板；
- 一次性命令和常驻 `shell`。

当前不在首版范围：WiFi、USB AOA、媒体库、目录监控、递归传输和照片同步。除非任务明确要求，
不要提前实现这些功能或为其引入依赖和复杂抽象。

## 2. 事实来源与验证等级

修改协议实现前，按以下优先级确认事实：

1. `docs/14-capture-validation.md` 中的真实设备抓包结果；
2. `proto/smartsync.proto` 和 APK 内的权威 proto2 schema；
3. `docs/13-verification-status.md` 的验证等级与源码索引；
4. 项目目录内的反编译源码：`Android_jadx/`、`android_smali/`、`original_smali_1.2.0/`、`macos/`；
5. 推断。

推断不能覆盖真实抓包结论。若实现与文档冲突，应先判断文档是否过时，并在同一变更中更新相关
文档；不要静默改变线上协议行为。

接口或实现细节不确定时（类名、方法签名、字段含义、调用顺序、返回格式等），必须先到上述反编译
源码中交叉验证原版行为，再据此进行 Rust 重写，切勿猜测、臆造行为或凭空补全接口。交叉验证时用
`rg` 搜索相关符号并通读其上下文，确认找到的是真实调用路径，而不是残留或无关代码。

开始协议任务时建议依次阅读：

1. `docs/01-overview.md`
2. `docs/05-message-framing.md`
3. `docs/04-handshake-trust.md`
4. `docs/06-protobuf-schema.md`
5. `docs/07-command-reference.md`
6. `docs/14-capture-validation.md`

## 3. 工程结构

- `src/lib.rs`：公开 library 入口，只导出稳定领域模型、配置、错误和客户端 API。
- `src/client.rs`：面向业务的 `HandShakerClient`，负责设备、文件、传输和剪贴板操作。
- `src/session.rs`：sid 路由、请求状态、心跳、读写任务、下载裸流和统一关闭。
- `src/transport/`：传输连接与清理；当前只实现 ADB。
- `src/protocol/`：封帧、密码学、握手和内部生成的 protobuf 类型。
- `src/domain.rs`：不依赖 Prost 的公开领域类型。
- `src/error.rs`：稳定错误分类和退出码。
- `src/i18n.rs`：语言资源加载与稳定消息 key。
- `src/cli.rs`：Clap 命令树、共享命令执行、确认策略和 REPL。
- `src/output.rs`：human、JSON、JSONL 渲染。
- `src/state.rs`：主机 UUID 和未来信任记录，权限必须安全。
- `proto/smartsync.proto`：完整 proto2 schema。
- `locales/zh-CN.json`：当前全部用户可见中文文案。
- `tests/`：CLI、集成和架构约束测试。
- `tools/capture/`：协议抓包与复现工具，不是正式客户端运行时依赖。

数据流保持为：

```text
CLI / REPL
  -> CommandExecutor
  -> HandShakerClient
  -> Session
  -> HandshakeStrategy
  -> TransportConnector
  -> TCP / ADB forward
```

`src/main.rs` 只负责启动、顶层信号处理和退出，不要把业务逻辑重新放回其中。

## 4. 不可破坏的协议不变量

- ADB 使用已抓包验证的 USB-style 裸公钥交换；未来 WiFi 的 REQUEST_01/02 与持久化信任是另一套
  握手流程，不能合并。
- RSA 为 1024 位，签名为 SHA-256 with RSA；每次 ADB 连接生成临时密钥。
- `enckey` 使用协议固定 AES-256-CBC 表；修改常量前必须有抓包向量和真机证据。
- 上行帧是 `[sid:u32 BE][flag:u8][len:u32 BE][payload]`，payload 上限 4 MiB。
- 下行帧是 `[sid:u32 BE][chunkLen:u16 BE][chunk]`，单块上限 32761 字节。
- 普通响应以 8 字节大端总长度重组；下载数据面是无长度包络的裸流，并以响应头声明长度为准。
- 响应类型以当前请求状态和预期消息为主，不能依赖 protobuf field 1；线上响应可能省略默认字段。
- 待处理请求必须在发送前注册 sid；完成后立即从 pending map 删除。
- 下载裸流出现无法可靠区分的同 sid 普通消息时，必须报协议错误，不能把可疑字节写入文件。
- Ready 会话在长时间上传和下载期间也必须维持心跳。
- QUIT、超时、EOF、取消和帧错误必须走统一、幂等的关闭流程。

手机端会检查主机兼容版本。`GET_DEVICE_INFO` 报告的兼容身份固定为：

```text
host_app_version = 2.5.6
host_app_version_code = 408
```

这不是 Cargo package 版本。没有新的真机证据时，绝对不要把它改成 `Cargo.toml` 的版本。

## 5. ADB 连接与设备安全

- 服务组件固定使用已经验证的
  `com.smartisanos.smartfolder.aoa/.service.ConnectionManagerService`。
- 通过 `adb forward tcp:0 tcp:10086` 获取动态本地端口。
- 只删除本进程明确创建的 forward；无法唯一识别时宁可返回错误，不猜测和误删。
- 默认不执行 `force-stop`，不杀手机应用，不尝试未经验证的备用包名、服务或端口。
- 未指定 `--serial` 时，只在恰好一台在线设备的情况下自动选择；零台或多台都明确报错。
- `device list` 只能读取 `adb devices -l`，不能启动服务或建立 forward。
- 真机测试只在用户已授权且测试目标明确时进行。创建唯一测试目录，不触碰用户文件；结束后验证测试
  文件和 adb forward 均已清理。

## 6. CLI 与输出兼容性

- 可执行文件名固定为 `handshaker`，Cargo package 名保持 `handshaker_rust`。
- 一次性命令与 REPL 必须共用同一套 Clap 命令解析和业务执行逻辑。
- 默认 human 输出为中文；JSON 字段、命令名、事件名和错误 code 固定使用英文，不参与本地化。
- JSON envelope 的 `schema_version` 当前固定为 `1`。变更字段属于兼容性变化，必须同时更新测试和文档。
- `json` 只输出一个最终对象；`jsonl` 可输出进度和事件流。不要向 stdout 混入日志或提示。
- human 日志、警告和交互提示写入正确的 stdout/stderr 通道。
- 删除、清空、覆盖等危险操作统一使用确认规则：非 TTY 或 JSON 模式除操作开关外还必须带
  `--yes`。
- 下载期间 Ctrl-C 通过关闭连接停止，因为手机不会中断已开始的下载流；不要伪造“取消成功”。
- 退出码保持稳定：参数 `2`，配置/设备选择 `3`，连接/握手 `4`，协议 `5`，手机端 `6`，本地
  I/O `7`，缺少确认 `8`，用户中断 `130`。

## 7. 文案和国际化

- 所有用户可见文本必须存放在 `locales/<language>.json`，Rust 源码只引用稳定英文消息 key。
- 当前只提供 `zh-CN`；新增语言时保持 key 集合和占位符一致。
- CLI 帮助、错误、确认提示、进度、REPL、普通日志、wire log 注释都属于用户可见文本。
- 不要在 `src/`、`build.rs` 或测试快照中硬编码中文。`tests/localization.rs` 会扫描并拒绝 CJK 文案。
- 协议常量、JSON key、错误 code、命令名、文件路径和第三方原始错误不是可翻译 key。
- 修改现有文案时优先复用 key；语义不同才新增 key，不要让一个 key 承担不兼容的多个含义。

## 8. protobuf、公开 API 与依赖

- 使用 `prost-build` 和 vendored `protoc` 从 `proto/smartsync.proto` 生成到 `OUT_DIR`。
- 不提交生成的 Rust protobuf 文件，不手改 `OUT_DIR` 内容。
- Prost 类型、线路帧和密码学细节保持 crate 内部可见；CLI 和未来 GUI 只使用公开领域类型。
- 公开类型变更需考虑未来 GUI 与自动化调用的兼容性，并补充序列化测试。
- 引入新依赖前先确认标准库和现有依赖无法完成任务。不要为未来功能加入暂时不用的框架。
- 修复问题时不要增加掩盖根因的备用流程、吞错或自动重试。协议异常应返回准确错误。

## 9. 状态与敏感数据

- 状态文件位于平台配置目录的 `handshaker/state.json`，schema version 当前为 `1`。
- 文件权限必须为 `0600`；配置目录在 Unix 上保持 `0700`。
- host UUID 必须稳定。ADB 当前不依赖持久化信任记录。
- `--wire-log` 只能显式开启，文件权限为 `0600`，并提示其可能包含文件和剪贴板内容。
- 普通日志只能记录 sid、flag、长度和状态，不输出 payload、剪贴板或文件正文。
- 不在提交、测试输出或回复中泄露真实设备隐私、信任密钥、抓包正文或用户文件内容。

## 10. 版本策略

- 每次软件代码更新根据变更类型自动调整版本。
- Bug 修复和简单小功能默认只递增修订号 Z，例如 `0.1.1 -> 0.1.2`。
- 纯文档、注释、格式、测试说明和其他不改变软件行为的更新不递增版本。
- 如果上一轮修复未成功或功能未完成，后续修正继续沿用上一轮已经设置的版本，不重复递增。
- 较大或不兼容变更不要自行决定 X/Y 版本；先说明影响并等待用户决定。
- 版本变更时同步更新 `Cargo.toml`、`Cargo.lock` 和 README 中项目版本引用。
- 不要把项目版本与手机兼容身份 `2.5.6 / 408` 混淆。

## 11. 实施工作流

1. 先阅读任务涉及的文档和模块，使用 `rg` 搜索现有实现、测试与反编译源码。
2. 检查工作树，保留用户已有修改，不覆盖无关变更。
3. 对协议任务先写清已经确认的线路事实和待验证假设。
4. 接口或实现不确定时，先到反编译源码中交叉验证原版行为再动手，不要猜测或凭空补全。
5. 做最小、直接的根因修复；不要顺手扩展未要求的功能。
6. 使用 `apply_patch` 修改文件；复制或移动文件时使用系统命令，不手工重新输出文件内容。
7. 为行为变更补充对应层级的单元、状态机、集成或 CLI 测试。
8. 更新受影响的协议文档、README、语言资源和版本。
9. 完成验证并检查最终 diff，确认没有生成物、敏感信息或残留 forward。

## 12. 必跑检查

代码变更提交前至少执行：

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
git diff --check
```

协议、会话或传输层变更还应运行相关定向测试，并确认以下覆盖没有退化：

- 32761 字节下行分块和 4 MiB 上行限制；
- 8 字节长度前缀跨帧；
- 下载裸流；
- field 1 缺失；
- sid 递增、碰撞和主动推送；
- 超时、EOF、取消和重复关闭；
- 假 adb 的动态端口解析与精确清理；
- JSON envelope、中文帮助和非 TTY 确认策略。

仅文档变更至少执行：

```sh
git diff --check
```

## 13. 真机验收

涉及 ADB、握手、Session、文件传输或剪贴板的高风险变更，在自动化测试通过后应建议或执行一次受控
真机验收：

1. 指定设备连接并读取设备信息；
2. ping 和列根目录；
3. 在唯一测试目录内创建、上传、下载并校验 MD5；
4. 重命名、剪贴板读写和删除测试目录；
5. 发送 QUIT；
6. 确认没有残留 adb forward。

未经用户授权不要扩大真机操作范围。若本轮没有真机条件，明确写明“自动化测试通过，尚未进行真机
验证”，不要把静态检查描述成互通验收。

## 14. 完成标准

交付前确认：

- 请求功能已经完成，而不是只搭接口或增加兜底；
- 协议行为有文档或抓包依据；
- public API 未泄露 Prost 类型；
- 用户文案全部来自语言文件；
- JSON schema、错误 code 和退出码保持兼容；
- 危险操作与日志不泄露数据；
- 自动化检查通过；
- 版本策略执行正确；
- 最终说明列出实际变更、验证结果和仍未验证的事项。
