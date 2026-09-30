export type ApiKeyStatus = 'active' | 'disabled';

export interface ApiKeyRecord {
  id: string;
  name: string;
  prefix: string;
  last4: string;
  maskedSecret: string;
  scope: string;
  createdAt: string;
  lastUsedAt: string;
  status: ApiKeyStatus;
}
