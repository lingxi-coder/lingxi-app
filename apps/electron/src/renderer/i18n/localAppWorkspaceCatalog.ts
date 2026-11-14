const entries: Record<string, [string, string, string, string, string]> = {
  apps_refresh: ['刷新', '重新整理', 'Refresh', '更新', '새로고침'],
  apps_loading: ['正在载入 App…', '正在載入 App…', 'Loading apps…', 'アプリを読み込み中…', '앱 불러오는 중…'],
  apps_new_session_named: ['为 %s 新建开发会话', '為 %s 建立開發對話', 'New development chat for %s', '%s の開発チャットを作成', '%s 새 개발 채팅'],
  apps_workspace_title: ['应用与开发会话', '應用與開發對話', 'Apps and development chats', 'アプリと開発チャット', '앱 및 개발 채팅'],
  apps_create: ['创建 App', '建立 App', 'Create app', 'アプリを作成', '앱 만들기'],
  apps_library: ['应用库', '應用程式庫', 'App library', 'アプリライブラリ', '앱 라이브러리'],
  apps_your_apps: ['你的 App', '你的 App', 'Your apps', '自分のアプリ', '내 앱'],
  apps_in_progress: ['创建中', '建立中', 'In progress', '作成中', '만드는 중'],
  apps_sidebar_empty: ['从一个想法开始。App 和它的开发对话都会保留在这里。', '從一個想法開始。App 和它的開發對話都會保留在這裡。', 'Start with an idea. Your apps and their development chats live here.', 'アイデアから始めましょう。アプリと開発チャットがここに並びます。', '아이디어로 시작하세요. 앱과 개발 채팅이 여기에 모입니다.'],
  apps_development_chats: ['开发会话', '開發對話', 'Development chats', '開発チャット', '개발 채팅'],
  apps_development_chat: ['开发会话', '開發對話', 'Development chat', '開発チャット', '개발 채팅'],
  apps_no_development_chats: ['选择编辑，继续完善这个 App。', '選擇編輯，繼續完善這個 App。', 'Choose Edit to work on this app.', '編集を選んでアプリを改良しましょう。', '편집을 선택해 앱을 개선하세요.'],
  apps_edit_named: ['编辑 %s', '編輯 %s', 'Edit %s', '%s を編集', '%s 편집'],
  apps_create_hint: ['每个 App 从独立的开发对话开始。已有会话和草稿会保留。', '每個 App 從獨立的開發對話開始。現有對話和草稿會保留。', 'Each app starts in its own development chat. Your existing chats and drafts stay in place.', 'アプリごとに専用の開発チャットを開始します。既存のチャットと下書きは保持されます。', '앱마다 별도의 개발 채팅을 시작합니다. 기존 채팅과 초안은 유지됩니다.'],
};
export const localAppWorkspaceCatalog = Object.fromEntries(Object.entries(entries).map(([key, values]) =>
  [key, Object.fromEntries(['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'].map((locale, index) => [locale, values[index]]))]));
