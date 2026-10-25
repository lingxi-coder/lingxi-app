const rows: Record<string, readonly [string, string, string, string, string]> = {
  "idle": [
    "轻点 Orb 开始聆听",
    "輕點 Orb 開始聆聽",
    "Tap the orb to start listening",
    "オーブをタップして聴き取りを開始",
    "오브를 눌러 듣기 시작"
  ],
  "preparing_audio": [
    "正在准备本机音频服务…",
    "正在準備本機音訊服務…",
    "Preparing native audio…",
    "ネイティブ音声を準備中…",
    "기기 오디오 준비 중…"
  ],
  "replacement_question": [
    "请说出新的问题…",
    "請說出新的問題…",
    "Say your new question…",
    "新しい質問を話してください…",
    "새 질문을 말씀하세요…"
  ],
  "recognition_failed": [
    "本机语音识别失败。",
    "本機語音辨識失敗。",
    "Native speech recognition failed.",
    "ネイティブ音声認識に失敗しました。",
    "기기 음성 인식에 실패했습니다."
  ],
  "recognition_invalid": [
    "本机语音服务返回了无效的识别结果。",
    "本機語音服務傳回無效的辨識結果。",
    "Native audio returned an invalid recognition result.",
    "ネイティブ音声から無効な認識結果が返されました。",
    "기기 오디오가 잘못된 인식 결과를 반환했습니다."
  ],
  "wait_interruption": [
    "正在等待插话识别完成…",
    "正在等待插話辨識完成…",
    "Waiting for interruption recognition to finish…",
    "割り込み音声の認識完了を待っています…",
    "끼어든 음성의 인식 완료를 기다리는 중…"
  ],
  "finish_failed": [
    "无法结束当前本机语音识别。",
    "無法結束目前的本機語音辨識。",
    "Could not finish the current native recognition.",
    "現在のネイティブ音声認識を終了できませんでした。",
    "현재 기기 음성 인식을 끝낼 수 없습니다."
  ],
  "wait_interruption_failed": [
    "结束插话识别失败：%1$s；仍在等待识别结果…",
    "結束插話辨識失敗：%1$s；仍在等待辨識結果…",
    "Could not finish interruption recognition: %1$s; still waiting for the result…",
    "割り込み音声の認識を終了できませんでした：%1$s。結果を待っています…",
    "끼어든 음성 인식을 끝내지 못했습니다: %1$s. 결과를 기다리는 중…"
  ],
  "wait_recognition_failed": [
    "结束识别失败：%1$s；仍在等待识别结果…",
    "結束辨識失敗：%1$s；仍在等待辨識結果…",
    "Could not finish recognition: %1$s; still waiting for the result…",
    "認識を終了できませんでした：%1$s。結果を待っています…",
    "인식을 끝내지 못했습니다: %1$s. 결과를 기다리는 중…"
  ],
  "send_unavailable": [
    "无法发送当前心流问题。",
    "無法傳送目前的心流問題。",
    "Could not send the current Flow question.",
    "現在のフローモードの質問を送信できませんでした。",
    "현재 플로우 질문을 보낼 수 없습니다."
  ],
  "send_failed": [
    "心流问题发送失败。",
    "心流問題傳送失敗。",
    "The Flow question could not be sent.",
    "フローモードの質問を送信できませんでした。",
    "플로우 질문 전송에 실패했습니다."
  ],
  "playback_unconfirmed": [
    "本机语音服务未确认播放完成。",
    "本機語音服務未確認播放完成。",
    "Native audio did not confirm playback completion.",
    "ネイティブ音声で再生完了を確認できませんでした。",
    "기기 오디오가 재생 완료를 확인하지 않았습니다."
  ],
  "continuing_reply": [
    "正在继续生成回复…",
    "正在繼續產生回覆…",
    "Continuing the reply…",
    "回答の生成を続けています…",
    "답변을 계속 생성하는 중…"
  ],
  "playback_failed": [
    "语音播放失败。",
    "語音播放失敗。",
    "Speech playback failed.",
    "音声の再生に失敗しました。",
    "음성 재생에 실패했습니다."
  ],
  "resume_reply": [
    "继续当前回复…",
    "繼續目前的回覆…",
    "Resuming the current reply…",
    "現在の回答を再開しています…",
    "현재 답변을 계속하는 중…"
  ],
  "no_interruption": [
    "没有检测到新的插话，重新开始聆听…",
    "未偵測到新的插話，重新開始聆聽…",
    "No new interruption detected. Listening again…",
    "新しい割り込み音声はありませんでした。聴き取りを再開します…",
    "새로운 음성이 감지되지 않았습니다. 다시 듣는 중…"
  ],
  "stop_failed": [
    "无法停止上一条心流问题。",
    "無法停止上一個心流問題。",
    "Could not stop the previous Flow question.",
    "前のフローモードの質問を停止できませんでした。",
    "이전 플로우 질문을 중지할 수 없습니다."
  ],
  "stop_previous": [
    "停止上一条心流问题失败：%1$s",
    "停止上一個心流問題失敗：%1$s",
    "Could not stop the previous Flow question: %1$s",
    "前のフローモードの質問を停止できませんでした：%1$s",
    "이전 플로우 질문을 중지하지 못했습니다: %1$s"
  ],
  "stop_previous_with_detail": [
    "%1$s；停止上一条心流问题失败：%2$s",
    "%1$s；停止上一個心流問題失敗：%2$s",
    "%1$s; could not stop the previous Flow question: %2$s",
    "%1$s。前のフローモードの質問を停止できませんでした：%2$s",
    "%1$s. 이전 플로우 질문을 중지하지 못했습니다: %2$s"
  ],
  "phase_interrupting": [
    "准备插话",
    "準備插話",
    "Preparing to interrupt",
    "割り込みの準備中",
    "끼어들기 준비 중"
  ],
  "phase_paused": [
    "待命",
    "待命",
    "Standing by",
    "待機中",
    "대기 중"
  ],
  "phase_retry": [
    "需要重试",
    "需要重試",
    "Retry needed",
    "再試行が必要です",
    "다시 시도 필요"
  ],
  "orb_finish": [
    "结束当前聆听",
    "結束目前的聆聽",
    "Finish listening",
    "聴き取りを終了",
    "듣기 종료"
  ],
  "orb_interrupt": [
    "打断当前回复并说出新问题",
    "打斷目前的回覆並說出新問題",
    "Interrupt the reply and ask a new question",
    "回答を中断して新しい質問を話す",
    "답변을 중단하고 새 질문하기"
  ],
  "orb_retry": [
    "重试心流模式",
    "重試心流模式",
    "Retry Flow mode",
    "フローモードを再試行",
    "플로우 모드 다시 시도"
  ],
  "orb_start": [
    "开始心流模式",
    "開始心流模式",
    "Start Flow mode",
    "フローモードを開始",
    "플로우 모드 시작"
  ],
  "footer_speaking": [
    "点击 Orb 可以打断并改问新的问题",
    "點擊 Orb 可打斷並改問新的問題",
    "Tap the orb to interrupt and ask a new question",
    "オーブをタップすると回答を中断して新しい質問ができます",
    "오브를 눌러 중단하고 새 질문을 하세요"
  ],
  "footer_thinking": [
    "回复生成中，点击 Orb 可以直接插话",
    "回覆產生中，點擊 Orb 可直接插話",
    "A reply is being generated. Tap the orb to interrupt",
    "回答を生成中です。オーブをタップして割り込めます",
    "답변 생성 중입니다. 오브를 눌러 끼어들 수 있습니다"
  ],
  "footer_interrupting": [
    "说完后会只取消当前心流回合并发送替代问题",
    "說完後只會取消目前的心流回合並傳送替代問題",
    "After you finish, only this Flow turn is cancelled and replaced with your new question",
    "話し終えると現在のフローターンだけを取り消し、新しい質問を送信します",
    "말씀을 마치면 현재 플로우 턴만 취소하고 새 질문으로 바꿉니다"
  ],
  "footer_configuration": [
    "先修复权限或模型问题，再继续完整闭环",
    "請先修正權限或模型問題，再繼續對話流程",
    "Resolve permission or model issues to continue the conversation",
    "権限またはモデルの問題を解決して会話を続けてください",
    "대화를 계속하려면 권한 또는 모델 문제를 해결하세요"
  ],
  "footer_failed": [
    "当前回合已停止，可重试或打开语音设置",
    "目前回合已停止，可重試或開啟語音設定",
    "This turn has stopped. Retry or open voice settings",
    "このターンは停止しました。再試行するか音声設定を開いてください",
    "현재 턴이 중지되었습니다. 다시 시도하거나 음성 설정을 여세요"
  ],
  "footer_relisten": [
    "心流会在播报结束后自动重新聆听",
    "心流會在播報結束後自動重新聆聽",
    "Flow listens again automatically after speaking",
    "読み上げが終わると自動的に聴き取りを再開します",
    "말하기가 끝나면 플로우가 자동으로 다시 듣습니다"
  ],
  "turn_based_processing": [
    "轮流说话模式正在处理回复",
    "輪流說話模式正在處理回覆",
    "Turn-based mode is processing the reply",
    "交互会話モードで回答を処理中です",
    "순서대로 말하기 모드에서 답변 처리 중"
  ],
  "turn_based_hint": [
    "轮流说话 · 轻点 Orb 结束聆听，播报后自动重新聆听",
    "輪流說話 · 輕點 Orb 結束聆聽，播報後自動重新聆聽",
    "Take turns speaking · Tap the orb to finish listening; listening resumes after playback",
    "交互に会話 · オーブをタップして聴き取りを終了。読み上げ後に自動で聴き取りを再開",
    "번갈아 말하기 · 오브를 눌러 듣기 종료, 재생 후 자동으로 다시 듣기"
  ]
};

const locales = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'];
const flowMessages: Record<string, Record<string, string>> = Object.fromEntries(
  Object.entries(rows).map(([key, values]) => [`desktop_flow_${key}`, Object.fromEntries(locales.map((locale, index) => [locale, values[index]]))]),
);

const audioGateCatalog: Record<string, Record<string, string>> = {
  desktop_audio_gate_busy: { 'zh-Hans': '音频设备正在使用中', 'zh-Hant': '音訊裝置正在使用中', en: 'Audio device in use', ja: '音声デバイスを使用中です', ko: '오디오 장치가 사용 중입니다' },
  desktop_audio_gate_missing_model: { 'zh-Hans': '当前语言缺少可用的识别模型，请在语音设置中下载模型。', 'zh-Hant': '目前語言缺少可用的辨識模型，請在語音設定中下載模型。', en: 'No recognition model is available for this language. Download one in voice settings.', ja: 'この言語の認識モデルがありません。音声設定でダウンロードしてください。', ko: '이 언어에 사용할 수 있는 인식 모델이 없습니다. 음성 설정에서 다운로드하세요.' },
  desktop_audio_gate_unavailable: { 'zh-Hans': '当前语言的音频服务不可用，请检查语音设置。', 'zh-Hant': '目前語言的音訊服務無法使用，請檢查語音設定。', en: 'Audio is unavailable for this language. Check voice settings.', ja: 'この言語の音声サービスを利用できません。音声設定を確認してください。', ko: '이 언어의 오디오 서비스를 사용할 수 없습니다. 음성 설정을 확인하세요.' },
  desktop_audio_gate_microphone: { 'zh-Hans': '需要麦克风授权，请在语音设置中授权。', 'zh-Hant': '需要麥克風授權，請在語音設定中授權。', en: 'Microphone permission is required. Grant access in voice settings.', ja: 'マイクの権限が必要です。音声設定で許可してください。', ko: '마이크 권한이 필요합니다. 음성 설정에서 허용하세요.' },
  desktop_audio_guide_permission_detail: { 'zh-Hans': '在语音设置中检查麦克风和语音识别授权。', 'zh-Hant': '在語音設定中檢查麥克風和語音辨識授權。', en: 'Check microphone and speech recognition permissions in voice settings.', ja: '音声設定でマイクと音声認識の権限を確認してください。', ko: '음성 설정에서 마이크 및 음성 인식 권한을 확인하세요.' },
  desktop_audio_guide_foreground_title: { 'zh-Hans': '请在前台使用麦克风', 'zh-Hant': '請在前景使用麥克風', en: 'Use the microphone in the foreground', ja: 'マイクは前面で使用してください', ko: '마이크는 앱이 전면에 있을 때 사용하세요' },
  desktop_audio_guide_foreground_detail: { 'zh-Hans': '返回此窗口后重试；也可在语音设置中检查输入设备和识别来源。', 'zh-Hant': '返回此視窗後重試；也可在語音設定中檢查輸入裝置和辨識來源。', en: 'Return to this window and retry. You can also check the input device and recognition source in voice settings.', ja: 'このウインドウに戻って再試行してください。音声設定で入力デバイスと認識ソースも確認できます。', ko: '이 창으로 돌아온 후 다시 시도하세요. 음성 설정에서 입력 장치와 인식 소스도 확인할 수 있습니다.' },
  desktop_audio_guide_quota_detail: { 'zh-Hans': '服务返回 HTTP 429。请检查账号额度，或在语音设置中切换到系统或离线来源。', 'zh-Hant': '服務回傳 HTTP 429。請檢查帳號額度，或在語音設定中切換到系統或離線來源。', en: 'The service returned HTTP 429. Check your account quota, or choose a system or offline source in voice settings.', ja: 'サービスが HTTP 429 を返しました。アカウントの利用枠を確認するか、音声設定でシステムまたはオフラインのソースに切り替えてください。', ko: '서비스가 HTTP 429를 반환했습니다. 계정 할당량을 확인하거나 음성 설정에서 시스템 또는 오프라인 소스로 전환하세요.' },
  desktop_audio_guide_no_speech_detail: { 'zh-Hans': '请确认麦克风能收到声音，并在语音设置中选择与你说话一致的语言。', 'zh-Hant': '請確認麥克風能收到聲音，並在語音設定中選擇與你說話一致的語言。', en: 'Check that your microphone receives sound, and select the language you are speaking in voice settings.', ja: 'マイクが音を拾っていることを確認し、音声設定で話している言語を選択してください。', ko: '마이크에 소리가 입력되는지 확인하고 음성 설정에서 말하는 언어를 선택하세요.' },
  desktop_audio_guide_unsupported_detail: { 'zh-Hans': '在语音设置中选择系统、离线模型或支持该操作的云端服务商。', 'zh-Hant': '在語音設定中選擇系統、離線模型或支援該操作的雲端服務商。', en: 'In voice settings, select the system, an offline model, or a cloud provider that supports this operation.', ja: '音声設定でシステム、オフラインモデル、またはこの操作に対応するクラウドプロバイダーを選択してください。', ko: '음성 설정에서 시스템, 오프라인 모델 또는 이 작업을 지원하는 클라우드 제공업체를 선택하세요.' },
  desktop_audio_guide_model_detail: { 'zh-Hans': '当前语言的识别服务不可用。可在语音设置中更改语言、下载离线模型或选择云端来源。', 'zh-Hant': '目前語言的辨識服務無法使用。可在語音設定中更改語言、下載離線模型或選擇雲端來源。', en: 'Recognition is unavailable for this language. Change the language, download an offline model, or choose a cloud source in voice settings.', ja: 'この言語の認識サービスを利用できません。音声設定で言語の変更、オフラインモデルのダウンロード、クラウドソースの選択を行ってください。', ko: '이 언어의 인식 서비스를 사용할 수 없습니다. 음성 설정에서 언어를 바꾸거나 오프라인 모델을 다운로드하거나 클라우드 소스를 선택하세요.' },
  desktop_audio_guide_busy_detail: { 'zh-Hans': '结束当前录音或播放后重试；可在语音设置中检查所选来源。', 'zh-Hant': '結束目前錄音或播放後重試；可在語音設定中檢查所選來源。', en: 'Finish the current recording or playback, then retry. You can check the selected source in voice settings.', ja: '現在の録音や再生を終了して再試行してください。音声設定で選択中のソースを確認できます。', ko: '현재 녹음 또는 재생을 끝낸 후 다시 시도하세요. 음성 설정에서 선택한 소스를 확인할 수 있습니다.' },
  desktop_audio_guide_dismiss: { 'zh-Hans': '关闭语音设置引导', 'zh-Hant': '關閉語音設定引導', en: 'Dismiss voice setup guide', ja: '音声設定ガイドを閉じる', ko: '음성 설정 안내 닫기' },
};

export const flowCatalog = { ...flowMessages, ...audioGateCatalog };
