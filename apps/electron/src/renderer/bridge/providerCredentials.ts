export async function persistProviderCredentialInput(
  providerId: string,
  input: string,
  persist: (providerId: string, credential: string) => Promise<unknown>,
): Promise<boolean> {
  const credential = input.trim();
  if (!credential) return false;
  await persist(providerId, credential);
  return true;
}

/** Async credential work may finish after Settings has been closed. */
export function isCurrentCredentialTransaction(
  mounted: boolean,
  transactionGeneration: number,
  currentGeneration: number,
): boolean {
  return mounted && transactionGeneration === currentGeneration;
}

/** Persist a credential, clear the renderer secret, then apply a deferred model. */
export async function persistProviderCredentialAndApplyModel(
  providerId: string,
  input: string,
  persist: (providerId: string, credential: string) => Promise<unknown>,
  clearSecret: () => void,
  pendingModelReference: string | null | undefined,
  applyModel: (reference: string) => Promise<unknown>,
): Promise<boolean> {
  const saved = await persistProviderCredentialInput(providerId, input, persist);
  if (!saved) return false;
  clearSecret();
  if (pendingModelReference) await applyModel(pendingModelReference);
  return true;
}
