import { useEffect, useState } from 'react';
import type { User } from 'firebase/auth';

/** `../app/useAuthUser` over a page fixture's fake `onAuthStateChanged`, so the real page renders against the fixture's sign-in. */
export const fakeAuthHook = (authModule: { onAuthStateChanged(auth: unknown, next: (user: User | null) => void): () => void }) => ({
  useAuthUser(): User | null | undefined {
    const [user, setUser] = useState<User | null | undefined>(undefined);
    useEffect(() => authModule.onAuthStateChanged({}, setUser), []);
    return user;
  },
});
