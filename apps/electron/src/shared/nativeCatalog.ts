const rows: Record<string, readonly [string, string, string, string, string]> = {
  settings: ['设置…', '設定…', 'Settings…', '設定…', '설정…'],
  file: ['文件', '檔案', 'File', 'ファイル', '파일'],
  links: ['链接', '連結', 'Links', 'リンク', '링크'],
  open_link: ['打开剪贴板中的链接', '開啟剪貼簿中的連結', 'Open Link from Clipboard', 'クリップボードのリンクを開く', '클립보드에서 링크 열기'],
  view: ['视图', '檢視', 'View', '表示', '보기'],
  activity: ['活动', '活動', 'Activity', 'アクティビティ', '활동'],
  trust_title: ['信任目录', '信任目錄', 'Trust directory', 'ディレクトリを信頼', '디렉터리 신뢰'],
  trust_message: ['是否信任 %1$s 中的文件？', '是否信任 %1$s 中的檔案？', 'Do you trust the files in %1$s?', '%1$s 内のファイルを信頼しますか？', '%1$s의 파일을 신뢰하시겠습니까?'],
  trust_detail: ['此会话将使用该目录的设置、Hooks、MCP 服务器、技能和记忆。', '此對話將使用該目錄的設定、Hooks、MCP 伺服器、技能和記憶。', 'This chat will use this directory’s settings, hooks, MCP servers, skills and memory.', 'この会話では、このディレクトリの設定、フック、MCP サーバー、スキル、メモリを使用します。', '이 대화는 이 디렉터리의 설정, 훅, MCP 서버, 스킬, 메모리를 사용합니다.'],
  trust_continue: ['信任并继续', '信任並繼續', 'Trust and continue', '信頼して続行', '신뢰하고 계속하기'],
  export_diagnostics: ['导出已脱敏的灵犀诊断信息', '匯出已去識別的靈犀診斷資訊', 'Export sanitized LingXi diagnostics', '機密情報を除いた LingXi 診断情報をエクスポート', '민감 정보가 제거된 LingXi 진단 내보내기'],
  bypass_accept: ['是的，我接受', '是的，我接受', 'Yes, I accept', 'はい、同意します', '예, 동의합니다'],
  bypass_title: ['免确认模式', '免確認模式', 'Bypass Permissions mode', '権限確認を省略するモード', '권한 확인 생략 모드'],
  bypass_message: ['在免确认模式下，灵犀运行可能危险的命令前不会征求你的批准。', '在免確認模式下，靈犀執行可能危險的命令前不會徵求你的核准。', 'In Bypass Permissions mode, LingXi will not ask for your approval before running potentially dangerous commands.', '権限確認を省略するモードでは、LingXi は危険な可能性のあるコマンドを実行する前に承認を求めません。', '권한 확인 생략 모드에서 LingXi는 위험할 수 있는 명령을 실행하기 전에 승인을 요청하지 않습니다.'],
  bypass_detail: ['此模式仅应在限制了互联网访问、受损后可轻松恢复的沙盒容器或虚拟机中使用。\n\n继续即表示你承担在免确认模式下执行操作的全部责任。', '此模式僅應在限制了網際網路存取、受損後可輕鬆復原的沙箱容器或虛擬機中使用。\n\n繼續即表示你承擔在免確認模式下執行操作的全部責任。', 'This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.\n\nBy proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.', 'このモードは、インターネットアクセスが制限され、破損時に簡単に復元できるサンドボックスコンテナまたは仮想マシンでのみ使用してください。\n\n続行すると、このモードで実行される操作のすべての責任を負うことに同意したものとみなされます。', '이 모드는 인터넷 접근이 제한되고 손상 시 쉽게 복원할 수 있는 샌드박스 컨테이너 또는 가상 머신에서만 사용해야 합니다.\n\n계속하면 권한 확인 생략 모드에서 수행되는 모든 작업에 대한 책임을 수락하게 됩니다.'],
  link_queue_full: ['待处理的灵犀链接过多。请先打开或关闭队列中的链接，然后重试。', '待處理的靈犀連結過多。請先開啟或關閉佇列中的連結，然後重試。', 'Too many pending LingXi links. Open or dismiss the queued links, then try again.', '保留中の LingXi リンクが多すぎます。キュー内のリンクを開くか閉じてから再試行してください。', '대기 중인 LingXi 링크가 너무 많습니다. 대기 중인 링크를 열거나 닫은 후 다시 시도하세요.'],
  link_unsupported: ['此灵犀链接的目标不受支持。', '此靈犀連結的目標不受支援。', 'This LingXi link destination is not supported.', 'この LingXi リンクの移動先はサポートされていません。', '이 LingXi 링크의 대상은 지원되지 않습니다.'],
  link_invalid: ['此灵犀链接无效，请复制完整链接后重试。', '此靈犀連結無效，請複製完整連結後重試。', 'This LingXi link is invalid. Copy the complete link and try again.', 'この LingXi リンクは無効です。リンク全体をコピーして再試行してください。', '이 LingXi 링크가 올바르지 않습니다. 전체 링크를 복사한 후 다시 시도하세요.'],
  link_unavailable: ['无法打开此灵犀链接', '無法開啟此靈犀連結', 'Unable to open this LingXi link', 'この LingXi リンクを開けません', '이 LingXi 링크를 열 수 없음'],
  link_workspace_missing: ['此链接所指的工作区在此设备上不可用。请先在本机打开工作区，再重试链接。', '此連結所指的工作區在此裝置上不可用。請先在本機開啟工作區，再重試連結。', 'The workspace referenced by this link is not available on this device. Open the workspace locally, then try the link again.', 'リンク先のワークスペースはこのデバイスでは利用できません。ローカルでワークスペースを開いてから再試行してください。', '이 링크의 작업 공간을 이 기기에서 사용할 수 없습니다. 작업 공간을 로컬에서 연 후 링크를 다시 시도하세요.'],
  delete_app: ['删除 %1$s？', '刪除 %1$s？', 'Delete %1$s?', '%1$s を削除しますか？', '%1$s을(를) 삭제하시겠습니까?'],
  delete_app_detail: ['这将删除应用及其保存的数据。', '這將刪除應用程式及其儲存的資料。', 'This removes the application and its saved data.', 'アプリケーションと保存されたデータを削除します。', '앱과 저장된 데이터가 삭제됩니다.'],
  allow_capabilities: ['允许 %1$s 使用以下能力？', '允許 %1$s 使用以下能力？', 'Allow %1$s to use these capabilities?', '%1$s にこれらの機能の使用を許可しますか？', '%1$s에 다음 기능을 허용하시겠습니까?'],
  capability_process: ['以你的用户身份运行本地程序和安装命令', '以你的使用者身分執行本機程式和安裝命令', 'Run local programs and installation commands as your user', 'あなたのユーザー権限でローカルプログラムとインストールコマンドを実行', '사용자 권한으로 로컬 프로그램 및 설치 명령 실행'],
  capability_files: ['选择要读取或写入的文件和文件夹', '選擇要讀取或寫入的檔案和資料夾', 'Choose files and folders to read or write', '読み書きするファイルやフォルダーを選択', '읽거나 쓸 파일 및 폴더 선택'],
  capability_clipboard: ['读取和写入剪贴板', '讀取和寫入剪貼簿', 'Read and write the clipboard', 'クリップボードの読み書き', '클립보드 읽기 및 쓰기'],
  capability_notifications: ['显示通知', '顯示通知', 'Show notifications', '通知を表示', '알림 표시'],
  capability_camera: ['使用相机', '使用相機', 'Use the camera', 'カメラを使用', '카메라 사용'],
  capability_microphone: ['使用麦克风', '使用麥克風', 'Use the microphone', 'マイクを使用', '마이크 사용'],
  revoke_hint: ['可在应用列表中撤销访问权限。', '可在應用程式列表中撤銷存取權限。', 'You can revoke access in Apps.', 'アプリ一覧でアクセス権を取り消せます。', '앱 목록에서 접근 권한을 취소할 수 있습니다.'],
  remote_title: ['灵犀 · 远程控制', '靈犀 · 遠端控制', 'LingXi · Remote Control', 'LingXi · リモート操作', 'LingXi · 원격 제어'],
  remote_allow: ['允许“%1$s”远程控制灵犀？', '允許「%1$s」遠端控制靈犀？', 'Allow “%1$s” to control LingXi remotely?', '「%1$s」に LingXi のリモート操作を許可しますか？', '“%1$s”의 LingXi 원격 제어를 허용하시겠습니까?'],
  remote_device: ['设备：%1$s（%2$s）', '裝置：%1$s（%2$s）', 'Device: %1$s (%2$s)', 'デバイス：%1$s（%2$s）', '기기: %1$s (%2$s)'],
  remote_code: ['配对码：%1$s', '配對碼：%1$s', 'Pairing code: %1$s', 'ペアリングコード：%1$s', '페어링 코드: %1$s'],
  remote_browser: ['网页浏览器', '網頁瀏覽器', 'Web browser', 'Web ブラウザー', '웹 브라우저'],
  remote_detail: ['请确认这台设备上显示的配对码与此处一致。允许后，该设备可以查看并操作你已开启远程的对话，包括批准权限请求。如果这不是你的设备，请选择“拒绝”。', '請確認這台裝置上顯示的配對碼與此處一致。允許後，該裝置可以檢視並操作你已開啟遠端的對話，包括核准權限請求。如果這不是你的裝置，請選擇「拒絕」。', 'Confirm the pairing code matches the one on this device. Once allowed, the device can view and control remotely enabled chats, including approving permission requests. Choose Deny if this is not your device.', 'このデバイスのペアリングコードと一致することを確認してください。許可すると、権限リクエストの承認を含め、リモート操作を有効にした会話をこのデバイスから表示・操作できます。自分のデバイスでなければ「拒否」を選んでください。', '이 기기의 페어링 코드와 일치하는지 확인하세요. 허용하면 이 기기에서 원격 기능을 켠 대화를 보고 제어할 수 있으며 권한 요청도 승인할 수 있습니다. 본인 기기가 아니면 거부를 선택하세요.'],
};
const locales = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'];
// Only these four canonical labels are needed by native dialogs. Their values
// are checked against resources/translations so the main bundle stays small.
const commonRows: Record<string, readonly string[]> = {
  common_cancel: ['取消', '取消', 'Cancel', 'キャンセル', '취소'],
  common_delete: ['删除', '刪除', 'Delete', '削除', '삭제'],
  permission_allow: ['允许', '允許', 'Allow', '許可', '허용'],
  permission_deny: ['拒绝', '拒絕', 'Deny', '拒否', '거부'],
};
export const nativeCatalog: Record<string, Record<string, string>> = Object.fromEntries(
  [...Object.entries(rows).map(([key, values]) => [`native_${key}`, values] as const), ...Object.entries(commonRows)]
    .map(([key, values]) => [key, Object.fromEntries(locales.map((locale, index) => [locale, values[index]]))]),
);
