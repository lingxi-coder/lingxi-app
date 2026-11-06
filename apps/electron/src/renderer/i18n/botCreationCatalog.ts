export const botCreationCatalog: Record<string, Record<string, string>> = {
  bots_assist_title: {
    'zh-Hans': '想让 Bot 帮你做什么？', 'zh-Hant': '想讓 Bot 幫你做什麼？',
    en: 'What would you like your Bot to do?', ja: 'Bot に何を手伝ってほしいですか？', ko: 'Bot이 어떤 일을 도와주면 좋을까요?',
  },
  bots_assist_body: {
    'zh-Hans': '说说你的想法，一起完善 Bot 的职责、人设和工作方式。', 'zh-Hant': '說說你的想法，一起完善 Bot 的職責、人設和工作方式。',
    en: 'Describe your idea, then shape your Bot’s role, persona and workflow together.',
    ja: 'アイデアを伝えて、Bot の役割、ペルソナ、作業の進め方を一緒に考えましょう。', ko: '아이디어를 설명하고 Bot의 역할, 페르소나, 작업 방식을 함께 다듬어 보세요.',
  },
  bots_assist_request: {
    'zh-Hans': '你的需求', 'zh-Hant': '你的需求', en: 'Your idea', ja: '作りたい Bot', ko: '만들고 싶은 Bot',
  },
  bots_assist_placeholder: {
    'zh-Hans': '例如：一个帮我修改中文文章的写作伙伴，保留我的语气，给出简洁的修改建议。',
    'zh-Hant': '例如：一個幫我修改中文文章的寫作夥伴，保留我的語氣，給出簡潔的修改建議。',
    en: 'For example: a writing partner that edits my articles, preserves my voice and gives concise feedback.',
    ja: '例：自分らしい文体を残しながら、記事を推敲して簡潔な改善案をくれる執筆パートナー。',
    ko: '예: 내 문체를 유지하면서 글을 다듬고 간결한 피드백을 주는 글쓰기 파트너.',
  },
  bots_assist_chat_hint: {
    'zh-Hans': '需求会填入新对话，发送后即可开始完善 Bot。', 'zh-Hant': '需求會填入新對話，傳送後即可開始完善 Bot。',
    en: 'Your idea will be ready in a new chat. Send it to start shaping your Bot.',
    ja: '新しいチャットに入力されます。送信すると Bot の相談を始められます。', ko: '새 채팅에 요청이 입력됩니다. 전송하면 Bot 만들기를 시작할 수 있습니다.',
  },
  bots_assist_examples: {
    'zh-Hans': '试试这些想法', 'zh-Hant': '試試這些想法', en: 'Try an idea', ja: 'アイデアの例', ko: '예시 아이디어',
  },
  bots_assist_example_writing_label: {
    'zh-Hans': '写作伙伴', 'zh-Hant': '寫作夥伴', en: 'Writing partner', ja: '執筆パートナー', ko: '글쓰기 파트너',
  },
  bots_assist_example_writing: {
    'zh-Hans': '我想创建一个写作伙伴，帮我梳理想法和修改文章，保留我的语气，给出简洁的修改建议。',
    'zh-Hant': '我想建立一個寫作夥伴，幫我整理想法和修改文章，保留我的語氣，給出簡潔的修改建議。',
    en: 'Create a writing partner that helps organize my ideas and edit articles, preserving my voice with concise feedback.',
    ja: 'アイデアの整理と記事の推敲を手伝い、自分らしい文体を残して簡潔な改善案をくれる執筆パートナーを作りたいです。',
    ko: '아이디어 정리와 글 수정을 도와주고, 내 문체를 유지하면서 간결한 피드백을 주는 글쓰기 파트너를 만들고 싶어요.',
  },
  bots_assist_example_research_label: {
    'zh-Hans': '研究助手', 'zh-Hant': '研究助手', en: 'Research helper', ja: 'リサーチ助手', ko: '리서치 도우미',
  },
  bots_assist_example_research: {
    'zh-Hans': '我想创建一个研究助手，帮我拆解问题、比较资料、整理有来源的结论，并明确指出不确定之处。',
    'zh-Hant': '我想建立一個研究助手，幫我拆解問題、比較資料、整理有來源的結論，並明確指出不確定之處。',
    en: 'Create a research helper that breaks down questions, compares sources and summarizes findings with citations and clear uncertainty.',
    ja: '問いを整理し、資料を比較して、出典と不確かな点を明示した結論にまとめるリサーチ助手を作りたいです。',
    ko: '질문을 나누어 살펴보고 자료를 비교하며, 출처와 불확실한 점을 명확히 밝혀 결과를 정리하는 리서치 도우미를 만들고 싶어요.',
  },
  bots_assist_example_code_label: {
    'zh-Hans': '代码审查伙伴', 'zh-Hant': '程式碼審查夥伴', en: 'Code reviewer', ja: 'コードレビュー', ko: '코드 리뷰어',
  },
  bots_assist_example_code: {
    'zh-Hans': '我想创建一个代码审查伙伴，优先找出真实缺陷和回归风险，结合代码解释原因，并给出可操作的修改建议。',
    'zh-Hant': '我想建立一個程式碼審查夥伴，優先找出實際缺陷和回歸風險，結合程式碼解釋原因，並給出可執行的修改建議。',
    en: 'Create a code reviewer that prioritizes real bugs and regressions, explains findings with code context and suggests practical fixes.',
    ja: '実際のバグやリグレッションを優先し、コードの文脈から原因を説明して具体的な修正案を出すレビュー助手を作りたいです。',
    ko: '실제 버그와 회귀 위험을 우선적으로 찾고, 코드 맥락과 함께 원인을 설명하며 실행 가능한 수정안을 제시하는 코드 리뷰어를 만들고 싶어요.',
  },
  bots_assist_steps: {
    'zh-Hans': '描述需求 → 对话完善 → 预览并创建', 'zh-Hant': '描述需求 → 對話完善 → 預覽並建立',
    en: 'Describe → Refine in chat → Review & create', ja: '説明 → チャットで調整 → 確認して作成', ko: '설명 → 채팅으로 다듬기 → 검토 후 만들기',
  },
  bots_assist_continue: {
    'zh-Hans': '继续到对话', 'zh-Hant': '繼續到對話', en: 'Continue in chat', ja: 'チャットで続ける', ko: '채팅에서 계속',
  },
  bots_assist_opening: {
    'zh-Hans': '正在打开对话…', 'zh-Hant': '正在開啟對話…', en: 'Opening chat…', ja: 'チャットを開いています…', ko: '채팅 여는 중…',
  },
  bots_assist_open_failed: {
    'zh-Hans': '暂时无法打开创建对话，请重试。', 'zh-Hant': '暫時無法開啟建立對話，請重試。',
    en: 'The setup chat could not be opened. Please try again.', ja: '作成用のチャットを開けませんでした。もう一度お試しください。', ko: '설정 채팅을 열 수 없습니다. 다시 시도해 주세요.',
  },
  bots_assist_method: {
    'zh-Hans': '创建方式', 'zh-Hant': '建立方式', en: 'Setup method', ja: '作成方法', ko: '설정 방법',
  },
  bots_assist_mode: {
    'zh-Hans': 'AI 辅助创建', 'zh-Hant': 'AI 輔助建立', en: 'Create with AI', ja: 'AI と作成', ko: 'AI와 만들기',
  },
  bots_assist_manual: {
    'zh-Hans': '手动设置', 'zh-Hant': '手動設定', en: 'Set up manually', ja: '手動で設定', ko: '직접 설정',
  },
  bots_assist_proposal: {
    'zh-Hans': 'Bot 方案', 'zh-Hant': 'Bot 方案', en: 'Bot proposal', ja: 'Bot の提案', ko: 'Bot 제안',
  },
  bots_assist_proposal_hint: {
    'zh-Hans': '预览后可编辑，点击「创建 Bot」才会保存。', 'zh-Hant': '預覽後可編輯，點選「建立 Bot」才會儲存。',
    en: 'Review and edit first. Saved when you choose Create Bot.', ja: '確認画面で編集できます。「Bot を作成」を選ぶと保存されます。', ko: '먼저 검토하고 수정하세요. Bot 만들기를 선택하면 저장됩니다.',
  },
  bots_assist_desktop_hint: {
    'zh-Hans': '可在 Desktop 的「Bot → 新建 Bot → 手动设置」中使用此方案。', 'zh-Hant': '可在 Desktop 的「Bot → 新增 Bot → 手動設定」中使用此方案。',
    en: 'Use this proposal in Desktop → Bots → New Bot → Set up manually.', ja: 'Desktop → Bot → 新しい Bot → 手動で設定でこの提案を使用できます。', ko: 'Desktop → Bot → 새 Bot → 직접 설정에서 이 제안을 사용하세요.',
  },
  bots_assist_review: {
    'zh-Hans': '预览并创建', 'zh-Hant': '預覽並建立', en: 'Review & create', ja: '確認して作成', ko: '검토 후 만들기',
  },
  bots_assist_review_title: {
    'zh-Hans': '预览 Bot', 'zh-Hant': '預覽 Bot', en: 'Review your Bot', ja: 'Bot を確認', ko: 'Bot 검토',
  },
  bots_assist_review_hint: {
    'zh-Hans': '检查名称和人设，按需选择模型、文件夹和权限，然后创建。', 'zh-Hant': '檢查名稱和人設，依需求選擇模型、資料夾和權限，然後建立。',
    en: 'Check the name and persona, choose any model, folders and permissions you need, then create your Bot.',
    ja: '名前とペルソナを確認し、必要に応じてモデル、フォルダ、権限を選んで作成してください。', ko: '이름과 페르소나를 확인하고 필요한 모델, 폴더, 권한을 선택한 뒤 Bot을 만드세요.',
  },
};
