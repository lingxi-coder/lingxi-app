import type { User } from 'firebase/auth';
import { auth } from '../app/firebase';
import { createInboxRequest } from '../../../../packages/inbox/client';
export function webInboxRequest(user: User) {
  return createInboxRequest('/v1/inbox', async () => {
    if (auth.currentUser?.uid !== user.uid) throw new Error('INVALID_SESSION');
    const token = await user.getIdToken();
    if (auth.currentUser?.uid !== user.uid) throw new Error('INVALID_SESSION');
    return token;
  });
}
