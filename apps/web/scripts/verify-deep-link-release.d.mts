/** Preflight association entries against supplied public production certificates. */
export function validateAndroidReleaseAssociations(entries: unknown, verifiedCertificates: Map<string, Set<string>>, variant?: 'all' | 'direct' | 'play'): void;
export function verifyDeepLinkRelease(argv: string[], root?: string): 'all' | 'direct' | 'play';
