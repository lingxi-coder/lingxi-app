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
