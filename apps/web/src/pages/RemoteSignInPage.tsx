import { useEffect, useState } from 'react';
import { GoogleAuthProvider, onAuthStateChanged, signInWithEmailAndPassword, signInWithPopup } from 'firebase/auth';
import googleIcon from '../assets/google-g.png';
import { auth } from '../app/firebase';
import type { Locale } from '../utils/locale';
import { safeNext } from '../remote/signInNext';
import { t, type StringKey } from '../remote/strings';
import '../remote/remote.css';

function failureKey(error: unknown): StringKey {
  const code = typeof error === 'object' && error !== null && 'code' in error ? String((error as { code: unknown }).code) : '';
  if (code === 'auth/popup-blocked') return 'signInPopupBlocked';
  if (code === 'auth/popup-closed-by-user' || code === 'auth/cancelled-popup-request') return 'signInCancelled';
  return 'signInFailed';
}

/**
 * Signs in for Remote Control and goes back to where the visitor came from.
 * It is its own document (`/login/remote`) because the Google popup needs
 * more than the strict policy of the Remote Control pages allows.
 */
export default function RemoteSignInPage({ locale }: { locale: Locale }) {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<StringKey | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    let leaving = false;
    return onAuthStateChanged(auth, (user) => {
      if (user === null || leaving) return;
      leaving = true;
      setDone(true);
      window.location.assign(safeNext(window.location.search, window.location.origin));
    });
  }, []);

  const attempt = async (action: () => Promise<unknown>): Promise<void> => {
    setBusy(true);
    setProblem(null);
    try {
      await action();
    } catch (error) {
      setProblem(failureKey(error));
      setBusy(false);
    }
  };

  return (
    <div className="rc-page">
      <main className="rc-main rc-signin">
        <section className="rc-panel">
          <h1>{t(locale, 'title')}</h1>
          <p>{t(locale, 'signedOutBody')}</p>
          <p className="rc-note">{t(locale, 'signInDoneHint')}</p>
          <button type="button" className="button secondary rc-google" disabled={busy || done} onClick={() => void attempt(() => signInWithPopup(auth, new GoogleAuthProvider()))}>
            <img src={googleIcon} alt="" width={20} height={20} />
            {t(locale, 'signInGoogle')}
          </button>
          <p className="rc-divider">{t(locale, 'signInOr')}</p>
          <form
            className="rc-form"
            onSubmit={(event) => {
              event.preventDefault();
              void attempt(() => signInWithEmailAndPassword(auth, email, password));
            }}
          >
            <label>
              {t(locale, 'email')}
              <input type="email" autoComplete="email" value={email} onChange={(event) => setEmail(event.target.value)} required />
            </label>
            <label>
              {t(locale, 'password')}
              <input type="password" autoComplete="current-password" value={password} onChange={(event) => setPassword(event.target.value)} required />
            </label>
            <button type="submit" className="button primary" disabled={busy || done}>{t(locale, 'signIn')}</button>
          </form>
          {problem ? <p className="rc-note rc-danger" role="alert">{t(locale, problem)}</p> : null}
          {done ? <p className="rc-note" role="status">{t(locale, 'signInDone')}</p> : null}
          <p><a className="rc-link" href="/">{t(locale, 'backToSite')}</a></p>
        </section>
      </main>
    </div>
  );
}
