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
import { sourceRoute } from '../inbox/route';
import type { Locale } from '../utils/locale';
import '../teams/teams-web.css';

function SignedIn({ user, locale, focusKey }: { user: User; locale: Locale; focusKey?: string }) {
  const [controller, setController] = useState<InboxController>();
  const [receipts, setReceipts] = useState<boolean>(),
    [preferenceError, setError] = useState('');
  useEffect(() => {
    let live = true;
    const request = webInboxRequest(user);
    const db = teamsDatabase(auth.app);
    const model = new InboxController({
      uid: user.uid,
      request,
      watch: (...args) => watchInbox(sdk, db, user.uid, ...args),
    });
    setController(model);
    void model.start();
    if (focusKey)
      void model
        .get(focusKey)
        .then((row) => {
          if (!live) return;
          const path = sourceRoute(row);
          if (path) location.replace(path);
          else setError('SOURCE_UNAVAILABLE');
        })
        .catch((e) => {
          if (live) setError(e.message);
        });
    void request<{ dmReceipts: boolean }>({ action: 'accountPreferences' })
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
  }, [user, focusKey]);
  return controller ? (
    <>
      <InboxList
        controller={controller}
        locale={locale}
        onOpen={(row) => {
          const route = sourceRoute(row);
          if (!route) throw new Error('SOURCE_UNAVAILABLE');
          location.assign(route);
        }}
      />
      <label>
        <input
          type="checkbox"
          disabled={receipts === undefined}
          checked={receipts ?? false}
          onChange={(e) => {
            const enabled = e.currentTarget.checked;
            setReceipts(undefined);
            const request = webInboxRequest(user);
            void request<{ dmReceipts: boolean }>({ action: 'accountPreferences', dmReceipts: enabled })
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
      </label>
      {preferenceError ? <p role="alert">{inboxErrorText(preferenceError, locale === 'zh')}</p> : null}
    </>
  ) : null;
}

export default function InboxPage({ locale, focusKey }: { locale: Locale; focusKey?: string }) {
  const [user, setUser] = useState<User | null>();
  useEffect(() => onAuthStateChanged(auth, setUser), []);
  return (
    <main className="teams-web">
      <header className="teams-web-nav">
        <a href="/">LingXi</a>
        <h1>{locale === 'zh' ? '收件箱' : 'Inbox'}</h1>
        <a href="/teams">{locale === 'zh' ? '团队' : 'Teams'}</a>
        <a href="/friends">{locale === 'zh' ? '好友' : 'Friends'}</a>
        <a href="/remote">{locale === 'zh' ? '设备' : 'Devices'}</a>
      </header>
      <div style={{ height: 'min(75dvh, 860px)', padding: '1rem' }}>
        {user ? (
          <SignedIn focusKey={focusKey} key={user.uid} user={user} locale={locale} />
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
