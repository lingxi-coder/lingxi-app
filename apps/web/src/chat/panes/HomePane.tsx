/** The workspace's home before Agent chat lands: a greeting and the ways into people's chats. */
export function HomePane({ locale, name }: { locale: string; name: string }) {
  const zh = locale === 'zh';
  return <div className="chat-notice chat-home">
    <h2>{zh ? `${name}，今天想聊点什么？` : `What's on your mind, ${name}?`}</h2>
    <p>{zh ? '从左侧选择团队、好友或 Bot 的对话。' : 'Pick a chat with a team, a friend or a Bot on the left.'}</p>
  </div>;
}
