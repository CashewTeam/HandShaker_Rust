# 06 protobuf 模式（SmartSyncProtocol.proto）

以下模式**逐字段**与 APK 内置权威源文件一致：
`Reference/Android_jadx/resources/main/proto/SmartSyncProtocol.proto`（与
`Reference/android_smali/unknown/main/proto/SmartSyncProtocol.proto` 相同）。
同时已与 macOS 端 `SmartFinderCore.h` 中 66 个 `SSP*` GPB 消息逐字段比对通过。

- 序列化：**protobuf proto2**，整数 varint（64 位），大端无关（varint 无字节序）。
- 每个消息 **field 1 = `SSPRequestType`**（命令/消息类型，见 §07），用作消息判别器。
- 时间戳均为 **Unix 秒**；文件大小单位 **byte**。
- 注意已删除字段的编号空洞不可占用：`SSPFile` 无 field 5；`SSPDeleteFileResponse` 无 field 3。
- `SSPVideoFile.created/modified_timestamp` 为 `uint32`，其余文件类为 `uint64`（真实 proto 如此）。

## 6.1 完整模式

```proto
syntax = "proto2";
package smartsync;

option java_package = "com.smartisanos.smartfolder.protocol";
option java_outer_classname = "SmartSyncProtocolProtos";

//文件
message SSPFile {
    optional string path = 1;
    optional uint64 file_size = 2;
    optional uint64 created_timestamp = 3;
    optional uint64 modified_timestamp = 4;
    optional bool isDirectory = 6;
    optional string checksum = 7;
    optional SSPFileType file_type = 8;
    optional string prefixMd5 = 9;
    optional string ext_data = 10;
    optional bool is_trash = 11;    //是否移动到回收站，用于文件删除请求
    optional bool succeed = 12;     //删除是否成功，用于响应文件删除请求
    optional SSPFileIOError error_code = 13; //删除错误码，用于响应文件删除请求
    optional uint64 id = 14;        //在手机媒体库中的文件id
}

message SSPImageFile {
    optional string path = 1;
    optional uint64 file_size = 2;
    optional uint64 created_timestamp = 3;
    optional uint64 modified_timestamp = 4;
    optional uint32 width = 5;
    optional uint32 height = 6;
    optional uint32 orientation = 7; //Exif Orientation Value
    optional uint64 media_id = 8;
    optional uint64 album_id = 9;
    optional string mime_type = 10;
    optional bytes thumbnail = 11;
    optional string album_name = 12;
    optional uint64 date_taken = 13;
    optional string latitude = 14;
    optional string longitude = 15;
    optional string mini_thumb_magic = 16;
    optional string title = 17;
    optional bool get_thumbnail_error = 18;
    optional bool starred = 19; // 是否为加星图片
}

message SSPImageAlbum {
    optional string album_path = 1;     //_data 去掉文件名
    optional uint64 album_id = 2;       //bucket_id
    optional string album_name = 3;     //bucket_display_name
    optional SSPImageFile cover_image = 4;
}

message SSPAudioFile {
    optional string path = 1;
    optional uint64 file_size = 2;
    optional uint64 created_timestamp = 3;
    optional uint64 modified_timestamp = 4;
    optional uint64 media_id = 5;
    optional uint64 album_id = 6;
    optional string title = 7;
    optional string mime_type = 8;
    optional uint64 artist_id = 9;
    optional string artist = 10;
    optional string composer = 11;
    optional uint32 genre = 12; //ID3v1 genre 列表
    optional string comment = 13;
    optional string copyright = 14;
    optional string audio_codec = 15;
    optional uint32 track = 16;
    optional double duration = 17; //second. apk 返回 ms，需 /1000.0
    optional double start_offset = 18; //APE 音轨 start offset；0=普通单文件
    optional uint32 year = 19;
    optional uint32 bitrate = 20; //kbps
    optional double sample_rate = 21; //kHz
    optional uint32 play_count = 22;
    optional double rating = 23;
    optional uint32 total_frames = 24;
    optional uint32 bitspersample = 25;
    optional uint32 channels = 26;
    optional string genre_name = 27;
}

message SSPAudioAlbum {
    optional string album_path = 1;
    optional uint64 album_id = 2;
    optional string album_name = 3;
    optional uint64 artist_id = 4;
    optional string artist = 5;
    optional uint32 year = 6;
    optional bytes thumbnail = 7;
    optional bool get_thumbnail_error = 8;
}

message SSPVideoFile {
    optional string path = 1;
    optional uint64 file_size = 2;
    optional uint32 created_timestamp = 3;  //注意：uint32
    optional uint32 modified_timestamp = 4; //注意：uint32
    optional uint32 width = 5;
    optional uint32 height = 6;
    optional uint32 orientation = 7;
    optional uint64 media_id = 8;
    optional uint64 album_id = 9;
    optional string mime_type = 10;
    optional bytes thumbnail = 11;
    optional bool get_thumbnail_error = 12;
    optional double duration = 13; //second
}

message SSPVideoAlbum {
    optional string album_path = 1;
    optional uint64 album_id = 2;
    optional string album_name = 3;
}

message SSPDataRange {
    optional uint64 offset = 1;
    optional uint64 length = 2;
}

//文件变更回调的对象
message SSPFileEvent {
    optional SSPFile file = 1; //事件主体
    optional SSPFileEventType event = 2; //事件
}

//请求命令类型定义
enum SSPRequestType {
    HEART_BEAT_REQUEST = 1;          //心跳请求
    GET_DEVICE_INFO_REQUEST = 2;     //获取固定设备信息
    GET_THUMBNAIL_REQUEST = 3;       //获取缩略图
    GET_PHOTO_LIB_REQUEST = 4;       //获取图库
    GET_VIDEO_LIB_REQUEST = 5;       //获取视频库
    GET_AUDIO_LIB_REQUEST = 6;       //获取乐库
    GET_DIR_FILES_REQUEST = 7;       //获取文件
    GET_FILE_COUNT_REQUEST = 8;      //目录下文件数量
    GET_FILE_EXIST_REQUEST = 9;      //检测文件是否存在
    GET_CREATE_FOLDER_REQUEST = 10;  //建立目录
    GET_RENAME_FILE_REQUEST = 11;    //重命名文件
    GET_DOWNLOAD_FILE_REQUEST = 12;  //下载请求
    GET_DOWNLOAD_FILE_RESPONSE_HEADER = 13; //下载响应头
    GET_DOWNLOAD_FILE_RESPONSE_BODY = 14;   //下载响应主体（未使用）
    GET_UPLOAD_FILE_REQUEST_HEADER = 15;    //上传请求头
    GET_UPLOAD_FILE_RESPONSE_HEADER = 16;   //上传头响应
    GET_UPLOAD_FILE_REQUEST_BODY = 17;      //上传主体（未使用）
    GET_UPLOAD_FILE_RESPONSE = 18;          //上传主体响应
    GET_DELETE_FILE_REQUEST = 19;           //删除文件
    PHOTO_LIB_CHANGE = 20;                  //图片库变更
    AUDIO_LIB_CHANGE = 21;                  //音频库变更
    VIDEO_LIB_CHANGE = 22;                  //视频库变更
    MONITOR_FOLDER_REQUEST = 23;            //目录监控请求
    MONITOR_FOLDER_RESPONSE_HEADER = 24;    //目录监控确认结果
    MONITOR_FOLDER_RESPONSE = 25;           //目录监控回调（文件变更）
    GET_CLIPBOARD_REQUEST = 26;             //获取剪切板
    POST_CLIPBOARD_REQUEST = 27;            //发送剪切板
    CLEAR_CLIPBOARD_REQUEST = 28;           //清空剪切板
    DELETE_CLIPBOARD_REQUEST = 29;          //删除剪切板
    CLIPBOARD_CHANGE = 30;                  //剪切板变更
    HANDSHAKE_REQUEST_01 = 31;              //握手请求 01
    HANDSHAKE_RESPONSE_01 = 32;             //握手响应 01
    HANDSHAKE_REQUEST_02 = 33;              //握手请求 02
    HANDSHAKE_RESPONSE_02 = 34;             //握手响应 02
    QUIT_REQUEST = 35;                      //退出
    CANCEL_REQUEST = 36;                    //取消请求
    PHOTO_SYNC_REQUEST = 37;                //照片同步请求
    FILE_CHANGE = 38;                       //文件变更
    SYNC_MONITOR_REQUEST = 39;              //实时同步请求
    UPDATE_FILE_INFO = 40;                  //更新文件信息
    UPDATE_FILE_INFO_RESPONSE = 41;         //更新文件信息响应
}

//文件变更事件类型（对应 Android FileObserver/inotify）
enum SSPFileEventType {
    FILE_EVENT_CREATE = 1;
    FILE_EVENT_DELETE = 2;
    FILE_EVENT_CLOSE_WRITE = 3;
    FILE_EVENT_MOVED_FROM = 4;
    FILE_EVENT_MOVED_TO = 5;
    FILE_EVENT_DELETE_SELF = 6;
    FILE_EVENT_MOVE_SELF = 7;
    FILE_EVENT_DIR_CHANGED = 8;
}

//文件操作错误类型
enum SSPFileIOError {
    FILE_IO_UNKNOW_ERROR = 1;                  //未知文件 IO 失败
    FILE_IO_INVALID_NAME = 2;                  //无效文件名
    FILE_IO_INVALID_SOURCE = 3;                //操作目标无效（如重命名的文件不存在）
    FILE_IO_TARGET_ALREADY_EXIST = 4;          //同名文件已存在
    FILE_IO_PERMISSION_ERROR = 5;              //权限错误
    FILE_IO_INSUFFICIENT_DISK_SPACE_ERROR = 6; //磁盘空间不足
    FILE_IO_MD5_CHECK_ERROR = 7;               //MD5 校验失败
    FILE_IO_SYSTEM_FILE = 8;                   //系统文件，无法修改
    FILE_IO_SDCARD_REMOVED = 9;                //SD卡被拔出
    FILE_IO_SDCARD_NO_PERMISSION = 10;         //Android<5.0 读写 SD 卡
    FILE_IO_PATH_OR_NAME_TOO_LONG = 11;        //文件名>255 / 路径>4096
    FILE_IO_CANCEL_ACTION = 12;                //取消相关操作
}

enum SSPFileIOPermission {
    ALLOW_NONE = 0;
    ALLOW_READ = 1;
    ALLOW_WRITE = 2;
    ALLOW_READ_WRITE = 3;
}

message SSPRequest {
    optional SSPRequestType type = 1;
}

//握手
enum SSPHandShakeTrustType {
    TRUST_WAITING = 1;
    TRUST_UNKNOW = 2;
    TRUST_NO = 3;
    TRUST_ONCE = 4;
    TRUST_ALWAYS = 5;
    TRUST_REMOVE = 6;
}

message SSPHandShakeRequest01 {
    optional SSPRequestType type = 1 [default = HANDSHAKE_REQUEST_01];
    optional string host_uuid = 2;                       //主机端 uuid（如 MAC 地址）
    optional string host_name = 3;                       //主机端电脑名称
    optional uint64 host_timestamp = 4;
    optional string host_smart_sync_protocol_version = 5;//协议版本号
    optional string host_app_version = 6;                //软件版本
    optional string host_min_client_version = 7;         //需要的最小 APK 版本
    optional bytes md5 = 8;                              //MD5 of ENCKEY
    optional bytes enckey = 9;                           //ENCKEY
    optional string host_model = 10;                     //机型，如 iMac12,2
    optional uint64 heartbeat_timeout_second = 11;       //心跳超时上限（秒）
}

message SSPHandShakeResponse01 {
    optional SSPRequestType type = 1 [default = HANDSHAKE_RESPONSE_01];
    optional string apk_version = 2;                     //apk version code
    optional string apk_version_name = 3;
    optional uint64 client_timestamp = 4;
    optional string client_smart_sync_protocol_version = 5;
    optional string client_min_host_version = 6;
    optional string device_uuid = 7;
    optional string device_name = 8;
    optional string usb_serial = 9;                      //ro.serialno / ro.boot.serialno
    optional bool is_smartisan_device = 10;
    optional uint64 client_min_host_version_code = 11;
}

message SSPHandShakeRequest02 {
    optional SSPRequestType type = 1 [default = HANDSHAKE_REQUEST_02];
    optional string host_uuid = 2;
    optional bytes derived_key = 3;                      //设备端生成 derived key
    optional SSPHandShakeTrustType trust_type = 4;
}

message SSPHandShakeResponse02 {
    optional SSPRequestType type = 1 [default = HANDSHAKE_RESPONSE_02];
    optional SSPHandShakeTrustType trust_type = 2;
    optional string device_uuid = 3;
    optional string device_name = 4;
    optional bytes derived_key = 5;
    optional string result = 6;  //'failed'/'locked'/'needauth'/base64(RSA_enc('ok'))
}

//基本请求
message SSPHeartBeatRequest {
    optional SSPRequestType type = 1 [default = HEART_BEAT_REQUEST];
    optional uint64 host_timestamp = 2;
}
message SSPHeartBeatResponse {
    optional SSPRequestType type = 1 [default = HEART_BEAT_REQUEST];
    optional uint64 host_timestamp = 2;
    optional uint64 client_timestamp = 3;
}

message SSPQuitRequest {
    optional SSPRequestType type = 1 [default = QUIT_REQUEST];
}

message SSPGetDeviceInfoRequest {
    optional SSPRequestType type = 1 [default = GET_DEVICE_INFO_REQUEST];
    optional uint64 host_timestamp = 2;
    optional string host_smart_sync_protocol_version = 3;
    optional bool need_device_info_callback = 4;  //是否需要 DeviceInfo 监控回调
    optional bool need_photo_library_callback = 5; //是否需要图片库监控回调
    optional bool need_audio_library_callback = 6; //是否需要音频库监控回调
    optional bool need_video_library_callback = 7; //是否需要视频库监控回调
    optional string host_app_version = 8;
    optional string host_min_client_version = 9;
    optional uint32 host_type = 10 [default = 1];  //mac(1) / windows(2)
    optional uint32 host_app_version_code = 11;
}

message SSPGetDeviceInfoResponse {
    optional SSPRequestType type = 1 [default = GET_DEVICE_INFO_REQUEST];
    optional uint64 host_timestamp = 2;    //回显
    optional string host_smart_sync_protocol_version = 3; //回显
    optional string apk_version = 4;       //apk version code
    optional uint64 client_timestamp = 5;
    optional string client_smart_sync_protocol_version = 6;
    optional string host_app_version = 7;  //回显
    optional string host_min_client_version = 8; //回显
    optional string phone_model = 9;       //ro.product.name
    optional string phone_color = 10;
    optional uint64 disk_size = 11;
    optional uint64 ram_size = 12;
    optional double battery_capacity = 13;
    optional uint32 battery_percentage = 14;
    optional string phone_name = 15;
    optional uint64 used_disk_size = 16;
    optional string root_path = 17;        //根目录
    optional string product_brand = 18;    //ro.product.brand
    optional string product_manufacturer = 19; //ro.product.manufacturer
    optional string smartisan_version = 20;//ro.smartisan.version
    optional bool phone_locked = 21;
    optional string client_min_host_version = 22;
    optional string apk_version_name = 23;
    optional string external_storage_path = 24;    //附加 sdcard 目录
    optional SSPFileIOPermission external_storage_permission = 25;
    optional uint64 ext_disk_size = 26;
    optional uint64 ext_used_disk_size = 27;
    optional string phone_id = 28;
    optional int64 audio_size = 29;      //音频占用空间
    optional int64 pic_video_size = 30;  //图片、视频占用空间
    optional int64 download_size = 31;   //下载占用空间
    optional int64 other_size = 32;
    optional int64 app_size = 33;
    optional int64 cache_size = 34;
    optional string debug_build_time = 35; //仅 debug 版
    optional int64 client_min_host_version_code = 36;
}

//文件与目录
message SSPGetDirFilesRequest {
    optional SSPRequestType type = 1 [default = GET_DIR_FILES_REQUEST];
    optional SSPFile dir = 2;
    optional uint32 maxdepth = 3; //递归深度；1=仅当前目录
}
message SSPGetDirFilesResponse {
    optional SSPRequestType type = 1 [default = GET_DIR_FILES_REQUEST];
    optional SSPFile dir = 2;
    optional uint32 maxdepth = 3;
    optional uint32 timecost = 4; //耗时 ms
    repeated SSPFile file = 5;
}

message SSPGetFileCountRequest {
    optional SSPRequestType type = 1 [default = GET_FILE_COUNT_REQUEST];
    optional SSPFile dir = 2;
    optional uint32 maxdepth = 3;
    repeated string exclusion_pattern = 4; //排除正则
}
message SSPGetFileCountResponse {
    optional SSPRequestType type = 1 [default = GET_FILE_COUNT_REQUEST];
    optional SSPFile dir = 2;
    optional uint32 maxdepth = 3;
    repeated string exclusion_pattern = 4;
    optional uint64 count = 5;
}

message SSPFileExistRequest {
    optional SSPRequestType type = 1 [default = GET_FILE_EXIST_REQUEST];
    optional SSPFile file = 2;
}
message SSPFileExistResponse {
    optional SSPRequestType type = 1 [default = GET_FILE_EXIST_REQUEST];
    optional SSPFile file = 2;
    optional bool exist = 3;
}

message SSPCreateFolderRequest {
    optional SSPRequestType type = 1 [default = GET_CREATE_FOLDER_REQUEST];
    optional SSPFile file = 2;
}
message SSPCreateFolderResponse {
    optional SSPRequestType type = 1 [default = GET_CREATE_FOLDER_REQUEST];
    optional SSPFile file = 2;
    optional bool succeed = 3;
    optional SSPFileIOError error_code = 4;
    optional string error_message = 5;
}

message SSPRenameFileRequest {
    optional SSPRequestType type = 1 [default = GET_RENAME_FILE_REQUEST];
    optional SSPFile source_file = 2;
    optional SSPFile target_file = 3;
}
message SSPRenameFileResponse {
    optional SSPRequestType type = 1 [default = GET_RENAME_FILE_REQUEST];
    optional SSPFile source_file = 2;
    optional SSPFile target_file = 3;
    optional bool succeed = 4;
    optional SSPFileIOError error_code = 5;
    optional string error_message = 6;
}

message SSPDeleteFileRequest {
    optional SSPRequestType type = 1 [default = GET_DELETE_FILE_REQUEST];
    repeated SSPFile file = 2;  //列表，可为目录（递归删除）
    optional bool is_sync = 3;  //是否维护同步状态
    optional bool is_trash = 4; //是否移动到回收站
}
message SSPDeleteFileResponse {
    optional SSPRequestType type = 1 [default = GET_DELETE_FILE_REQUEST];
    repeated SSPFile file = 2;
    optional bool succeed = 4;
    optional SSPFileIOError error_code = 5;
    optional string error_message = 6;
}

message SSPMonitorFolderRequest {
    optional SSPRequestType type = 1 [default = MONITOR_FOLDER_REQUEST];
    optional SSPFile file = 2;      //要监控的目录
    optional bool register = 3;     //true=监控 false=取消监控
}
message SSPMonitorFolderResponseHeader {
    optional SSPRequestType type = 1 [default = MONITOR_FOLDER_RESPONSE_HEADER];
    optional bool succeed = 2;
    optional string error_message = 3;
}
message SSPMonitorFolderResponse {
    optional SSPRequestType type = 1 [default = MONITOR_FOLDER_RESPONSE];
    repeated SSPFileEvent event = 2; //实际文件变更回调
}

//下载（手机端 → 电脑端）
message SSPDownloadFileRequest {
    optional SSPRequestType type = 1 [default = GET_DOWNLOAD_FILE_REQUEST];
    optional SSPFile file = 2;           //被下载文件，不含目录
    optional SSPDataRange range = 3;     //起始字节+长度；length=0=全量
    optional bool need_md5 = 4;          //是否计算 md5
    optional bool gzip = 5;              //数据是否 gzip 压缩
    optional bool is_sync = 6;           //是否维护同步状态
}
message SSPDownloadFileResponseHeader {
    optional SSPRequestType type = 1 [default = GET_DOWNLOAD_FILE_RESPONSE_HEADER];
    optional SSPFile file = 2;
    optional SSPDataRange range = 3;     //请求 offset + 实际返回长度
    optional bool need_md5 = 4;
    optional string data_md5 = 5;        //数据区 MD5（need_md5=1 时）
    optional bool ready = 6;             //是否可以继续下载
    optional SSPFileIOError error_code = 7;
}
// SSPDownloadFileResponseBody 已注释：大文件走 session_id + binary

//上传（电脑端 → 手机端）
message SSPUploadFileRequest {
    optional SSPRequestType type = 1 [default = GET_UPLOAD_FILE_REQUEST_HEADER];
    optional SSPFile file = 2;      //上传位置；存在则覆盖（总是覆盖）；含大小
    optional string data_md5 = 3;   //整体 md5，空则不校验
    optional bool gzip = 4;         //数据是否 gzip 压缩
    optional bool is_sync = 5;      //是否维护同步状态
}
message SSPUploadFileResponseHeader {
    optional SSPRequestType type = 1 [default = GET_UPLOAD_FILE_RESPONSE];
    optional SSPFile file = 2;
    optional bool ready = 3;        //空间已准备，可以接收
    optional SSPFileIOError error_code = 4;
}
message SSPUploadFileResponse {
    optional SSPRequestType type = 1 [default = GET_UPLOAD_FILE_RESPONSE];
    optional SSPFile file = 2;
    optional bool canceled = 3;     //是否被取消
    optional bool succeed = 4;      //是否成功
    optional SSPFileIOError error_code = 5;
}

//媒体库与缩略图
message SSPGetThumbnailRequest {
    optional SSPRequestType type = 1 [default = GET_THUMBNAIL_REQUEST];
    repeated SSPImageFile image = 2;       //先按 media_id，无则按 path
    repeated SSPVideoFile video = 3;
    repeated SSPAudioAlbum audio_album = 4;
}
message SSPGetThumbnailResponse {
    optional SSPRequestType type = 1 [default = GET_THUMBNAIL_REQUEST];
    repeated SSPImageFile image = 2;
    repeated SSPVideoFile video = 3;
    repeated SSPAudioAlbum audio_album = 4;
}

message SSPGetPhotoLibraryRequest {
    optional SSPRequestType type = 1 [default = GET_PHOTO_LIB_REQUEST];
}
message SSPGetPhotoLibraryResponse {
    optional SSPRequestType type = 1 [default = GET_PHOTO_LIB_REQUEST];
    repeated SSPImageFile image = 2;
    repeated SSPImageAlbum album = 3;
    optional uint64 camera_album_id = 4; //相机相册 id
}

message SSPGetVideoLibraryRequest {
    optional SSPRequestType type = 1 [default = GET_VIDEO_LIB_REQUEST];
}
message SSPGetVideoLibraryResponse {
    optional SSPRequestType type = 1 [default = GET_VIDEO_LIB_REQUEST];
    repeated SSPVideoFile video = 2;
    repeated SSPVideoAlbum album = 3;
}

message SSPGetAudioLibraryRequest {
    optional SSPRequestType type = 1 [default = GET_AUDIO_LIB_REQUEST];
}
message SSPGetAudioLibraryResponse {
    optional SSPRequestType type = 1 [default = GET_AUDIO_LIB_REQUEST];
    repeated SSPAudioFile audio = 2;
    repeated SSPAudioAlbum album = 3;
}

//媒体库变更回调
message SSPPhotoLibraryChange {
    optional SSPRequestType type = 1 [default = PHOTO_LIB_CHANGE];
    repeated SSPImageFile added_image = 2;
    repeated SSPImageFile deleted_image = 3;
}
message SSPVideoLibraryChange {
    optional SSPRequestType type = 1 [default = VIDEO_LIB_CHANGE];
    repeated SSPVideoFile added_video = 2;
    repeated SSPVideoFile deleted_video = 3;
    repeated SSPVideoFile updated_video = 4; //部分手机视频先建后改
}
message SSPAudioLibraryChange {
    optional SSPRequestType type = 1 [default = AUDIO_LIB_CHANGE];
    repeated SSPAudioFile added_audio = 2;
    repeated SSPAudioFile deleted_audio = 3;
    repeated SSPAudioAlbum added_album = 4;
}

//剪切板
message SSPClipboard {
    optional bytes content = 1;     // gzip 压缩过的剪切板内容
    optional int64 mstimestamp = 2;
}
message SSPGetClipboardRequest {
    optional SSPRequestType type = 1 [default = GET_CLIPBOARD_REQUEST];
}
message SSPGetClipboardResponse {
    optional SSPRequestType type = 1 [default = GET_CLIPBOARD_REQUEST];
    repeated SSPClipboard clipboard = 2;
}
message SSPPostClipboardRequest {
    optional SSPRequestType type = 1 [default = POST_CLIPBOARD_REQUEST];
    required SSPClipboard clipboard = 2;
}
message SSPPostClipboardResponse {
    optional SSPRequestType type = 1 [default = POST_CLIPBOARD_REQUEST];
    optional bool succeed = 2;
}
message SSPClearClipboardRequest {
    optional SSPRequestType type = 1 [default = CLEAR_CLIPBOARD_REQUEST];
}
message SSPClearClipboardResponse {
    optional SSPRequestType type = 1 [default = CLEAR_CLIPBOARD_REQUEST];
    optional bool succeed = 2;
}
message SSPDeleteClipboardRequest {
    optional SSPRequestType type = 1 [default = DELETE_CLIPBOARD_REQUEST];
    required SSPClipboard clipboard = 2;
}
message SSPDeleteClipboardResponse {
    optional SSPRequestType type = 1 [default = DELETE_CLIPBOARD_REQUEST];
    optional bool succeed = 2;
}
message SSPClipboardChange {
    optional SSPRequestType type = 1 [default = CLIPBOARD_CHANGE];
    repeated SSPClipboard clipboard = 2;
}

//取消
message SSPCancelRequest {
    optional SSPRequestType type = 1 [default = CANCEL_REQUEST];
    optional uint64 session_id = 2;
    optional SSPCancelErrorCode error_code = 3;
}
enum SSPCancelErrorCode {
    ERROR_CODE_UNKNOWN = 1;
    ERROR_CODE_SDCARD_REMOVED = 2;
}

//同步
message SSPPhotoSyncRequest {
    optional SSPRequestType type = 1 [default = PHOTO_SYNC_REQUEST];
    optional string pc_id = 2;           //pc 端唯一标志
    repeated SSPFile files = 3;          //上一次同步的快照列表
}
message SSPPhotoSyncResponse {
    optional SSPRequestType type = 1 [default = PHOTO_SYNC_REQUEST];
    optional bool is_first = 2;          //是否首次同步
    repeated SSPFile files = 3;          //当前手机端文件状态列表
    optional bool is_success = 4;
}

enum SSPFileType {
    NORMAL = 0;
    DATA = 1;
}

message SSPFileChange {
    optional SSPRequestType type = 1 [default = FILE_CHANGE];
    repeated SSPFileChangeItem file_change_items = 2;
}
message SSPFileChangeItem {
    optional SSPFile file = 1;
    optional SSPFileChangeStatus status = 2;
}
enum SSPFileChangeStatus {
    None = 0;
    Added = 1;
    Deleted = 2;
    Modified = 3;
    InfoModified = 4;      //修改文件附属信息
    FileAndInfoModified = 5;
}

message SSPSyncMonitorRequest {
    optional SSPRequestType type = 1 [default = SYNC_MONITOR_REQUEST];
    optional bool is_sync_monitor = 2;  //是否开启实时同步
}
message SSPSyncMonitorResponse {
    optional SSPRequestType type = 1 [default = SYNC_MONITOR_REQUEST];
    optional bool is_success = 2;
}

message SSPUpdateFileRequest {
    optional SSPRequestType type = 1 [default = UPDATE_FILE_INFO];
    repeated SSPFile files = 2;
    optional bool is_sync = 3;
}
message SSPUpdateFileResponse {
    optional SSPRequestType type = 1 [default = UPDATE_FILE_INFO_RESPONSE];
    optional bool is_success = 2;
}
```

## 6.2 反编译类 ↔ 消息对照表（Android `com.smartisanos.smartfolder.a.a`）

| 反编译类 | 消息 | 反编译类 | 消息 |
|---|---|---|---|
| `aj` | SSPFile | `bm` | SSPGetDirFilesResponse |
| `cp` | SSPImageFile | `bo` | SSPGetFileCountRequest |
| `cn` | SSPImageAlbum | `bq` | SSPGetFileCountResponse |
| `c` | SSPAudioFile | `as` | SSPFileExistRequest |
| `C0028a` | SSPAudioAlbum | `cr` | SSPFileExistResponse |
| `eg` | SSPVideoFile | `r` | SSPCreateFolderRequest |
| `ee` | SSPVideoAlbum | `t` | SSPCreateFolderResponse |
| `v` | SSPDataRange | `dj` | SSPRenameFileRequest |
| `ap` | SSPFileEvent | `dl` | SSPRenameFileResponse |
| `dn` | SSPRequest | `ab` | SSPDeleteFileRequest |
| `ca` | SSPHandShakeRequest01 | `ad` | SSPDeleteFileResponse |
| `ce` | SSPHandShakeResponse01 | `au` | SSPMonitorFolderRequest |
| `cc` | SSPHandShakeRequest02 | `cu` | SSPMonitorFolderResponseHeader |
| `cg` | SSPHandShakeResponse02 | `ct` | SSPMonitorFolderResponse |
| `cj` | SSPHeartBeatRequest | `af` | SSPDownloadFileRequest |
| `cl` | SSPHeartBeatResponse | `ah` | SSPDownloadFileResponseHeader |
| `dh` | SSPQuitRequest | `dy` | SSPUploadFileRequest |
| `bg` | SSPGetDeviceInfoRequest | `eb` | SSPUploadFileResponseHeader |
| `bi` | SSPGetDeviceInfoResponse | `ea` | SSPUploadFileResponse |
| `bk` | SSPGetDirFilesRequest | `bu` | SSPGetThumbnailRequest |
| `bw` | SSPGetThumbnailResponse | `bs` | SSPGetPhotoLibraryResponse |
| `by` | SSPGetVideoLibraryResponse | `ba` | SSPGetAudioLibraryResponse |
| `cx` | SSPPhotoLibraryChange | `ei` | SSPVideoLibraryChange |
| `e` | SSPAudioLibraryChange | `n` | SSPClipboard |
| `bc` | SSPGetClipboardRequest | `be` | SSPGetClipboardResponse |
| `dd` | SSPPostClipboardRequest | `df` | SSPPostClipboardResponse |
| `j` | SSPClearClipboardRequest | `l` | SSPClearClipboardResponse |
| `x` | SSPDeleteClipboardRequest | `z` | SSPDeleteClipboardResponse |
| `o` | SSPClipboardChange | `h` | SSPCancelRequest |
| `cz` | SSPPhotoSyncRequest | `db` | SSPPhotoSyncResponse |
| `ak` | SSPFileChange | `al` | SSPFileChangeItem |
| `dq` | SSPSyncMonitorRequest | `ds` | SSPSyncMonitorResponse |
| `du` | SSPUpdateFileRequest | `dw` | SSPUpdateFileResponse |

## 6.3 枚举数值总表

| 枚举 | 值 |
|---|---|
| SSPRequestType | 见 §6.1（1..41） |
| SSPFileEventType | CREATE=1 DELETE=2 CLOSE_WRITE=3 MOVED_FROM=4 MOVED_TO=5 DELETE_SELF=6 MOVE_SELF=7 DIR_CHANGED=8 |
| SSPFileIOError | 1..12 |
| SSPFileIOPermission | ALLOW_NONE=0 ALLOW_READ=1 ALLOW_WRITE=2 ALLOW_READ_WRITE=3 |
| SSPHandShakeTrustType | WAITING=1 UNKNOW=2 NO=3 ONCE=4 ALWAYS=5 REMOVE=6 |
| SSPCancelErrorCode | UNKNOWN=1 SDCARD_REMOVED=2 |
| SSPFileType | NORMAL=0 DATA=1 |
| SSPFileChangeStatus | None=0 Added=1 Deleted=2 Modified=3 InfoModified=4 FileAndInfoModified=5 |

## 6.4 互通注意

1. 每个请求/响应 field 1 默认值即该消息类型；实现时以 field 1 作为判别器即可。
   > ⚠️ **抓包实测（2026-08）**：field 1 在等于默认值时**可能被线上省略**（如 `SSPUploadFileResponse`
   > 只有 field 2/3/4，无 field 1）。解析器应视 field 1 为提示、不得假定必然存在。
2. varint 有符号/无符号不影响线格式（同为 varint64）。`SSPClipboard.mstimestamp`、device info 的
   `*_size` 用 int64 读也无妨。
3. `SSPGetPhotoLibraryRequest/SSPGetVideoLibraryRequest/SSPGetAudioLibraryRequest` 仅主机→手机，
   APK 未内置其类；实现互通时仍需构造（内容只有 type 字段）。
4. `SSPDownloadFileResponseBody` / `SSPUploadFileRequestBody` 已注释，线上不用。
5. 上传响应头 type 线上回显 **15**（请求类型），与 proto 默认注释（16/18）不一致，以线上为准。
