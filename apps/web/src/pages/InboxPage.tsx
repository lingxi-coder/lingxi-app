import { useEffect, useState } from 'react';
import { onAuthStateChanged, type User } from 'firebase/auth';
import * as sdk from 'firebase/firestore';
import { InboxList, inboxErrorText } from '../../../../packages/inbox/InboxList';
import { webInboxRequest } from '../inbox/session';
import { InboxController } from '../../../../packages/inbox/controller';
import { watchInbox } from '../../../../packages/inbox/firestore';
import { auth } from '../app/firebase';
import { teamsDatabase } from '../teams/firestore';
import { SignInCard } from '../teams/SignInCard';
import {type InboxNotice} from '../../../../packages/inbox/contracts';
import { noticeTargetFromSearch,sourceRoute } from '../inbox/route';
import type { Locale } from '../utils/locale';
import '../teams/teams-web.css';

function SignedIn({ user, locale, focusKey, projects }: { user: User; locale: Locale; focusKey?: string; projects: boolean }) {
  const [controller, setController] = useState<InboxController>();
  const [receipts, setReceipts] = useState<boolean>(),
    [preferenceError, setError] = useState('');
  useEffect(() => {
    let live = true;
    const request = webInboxRequest(user);
    const db = teamsDatabase(auth.app);
    const model = new InboxController({
      uid: user.uid,
      scope: projects ? 'assistant' : 'messages',
      request,
      watch: (...args) => watchInbox(sdk, db, user.uid, ...args),
    });
    setController(model);
    void model.start();
    if(focusKey){
      const resolve=async()=>{
        const id=new URLSearchParams(location.search).get('notificationId');
        const target=id?(await request<{item:InboxNotice}>({action:'notice',id})).item:noticeTargetFromSearch(location.search);
        if(target?.key && target.key!==focusKey)throw new Error('SOURCE_ACCESS_REQUIRED');
        const row=await model.get(focusKey,target?.generation);
        if(!live)return;
        const path=sourceRoute(row,target??{through:-1,key:row.key,generation:row.ref.generation});if(path)location.replace(path);else setError('SOURCE_UNAVAILABLE');
      };
      void resolve().catch(e=>{if(live)setError(e.message);});
    }
    if (!projects) void request<{ dmReceipts: boolean }>({ action: 'accountPreferences' })
      .then((result) => {
        if (live) setReceipts(result.dmReceipts);
      })
      .catch((e) => {
        if (live) setError(e.message);
      });
    return () => {
      live = false;
      model.close();
    };
  }, [user, focusKey, projects]);
  return controller ? (
    <>
      <InboxList
        controller={controller}
        locale={locale}
        onOpen={(row,target) => {
          const route = sourceRoute(row,target);
          if (!route) throw new Error('SOURCE_UNAVAILABLE');
          location.assign(route);
        }}
      />
      {!projects && <label>
        <input
          type="checkbox"
          disabled={receipts === undefined}
          checked={receipts ?? false}
          onChange={(e) => {
            const enabled = e.currentTarget.checked;
            setReceipts(undefined);
            const request = webInboxRequest(user);
            if (!projects) void request<{ dmReceipts: boolean }>({ action: 'accountPreferences', dmReceipts: enabled })
              .then((result) => {
                setReceipts(result.dmReceipts);
                setError('');
              })
              .catch((e) => {
                setReceipts(!enabled);
                setError(e.message);
              });
          }}
        />
        {locale === 'zh'
          ? '双方开启时共享私聊已读回执'
          : 'Share direct-message read receipts when both people enable them'}
      </label>}
      {preferenceError ? <p role="alert">{inboxErrorText(preferenceError, locale === 'zh')}</p> : null}
    </>
  ) : null;
}

export default function InboxPage({ locale, focusKey, projects = false }: { locale: Locale; focusKey?: string; projects?: boolean }) {
  const [user, setUser] = useState<User | null>();
  useEffect(() => onAuthStateChanged(auth, setUser), []);
  return (
    <main className="teams-web">
      <header className="teams-web-nav">
        <a href="/">LingXi</a>
        <h1>{projects ? (locale === 'zh' ? '项目' : 'Projects') : (locale === 'zh' ? '消息' : 'Messages')}</h1>
        {projects ? <a href="/inbox">{locale === 'zh' ? '消息' : 'Messages'}</a> : <a href="/projects">{locale === 'zh' ? '项目' : 'Projects'}</a>}
        <a href="/teams">{locale === 'zh' ? '团队' : 'Teams'}</a>
        <a href="/friends">{locale === 'zh' ? '好友' : 'Friends'}</a>
        <a href="/remote">{locale === 'zh' ? '设备' : 'Devices'}</a>
      </header>
      <div style={{ height: 'min(75dvh, 860px)', padding: '1rem' }}>
        {user ? (
          <SignedIn projects={projects} focusKey={focusKey} key={user.uid} user={user} locale={locale} />
        ) : user === null ? (
          <SignInCard
            auth={auth}
            locale={locale}
            note={locale === 'zh' ? '登录查看会话' : 'Sign in to view your conversations.'}
          />
        ) : null}
      </div>
    </main>
  );
}
