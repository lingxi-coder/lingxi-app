export const CH_APPLICATION_ICON = 'lingxi:application-icon';

/** Native icons are resolved by bundle identity, never by renderer file paths. */
export interface ApplicationIconApi {
  get(bundleId: string): Promise<string | null>;
}

export function validBundleId(value: unknown): value is string {
  return typeof value === 'string' && value.length <= 255
    && /^[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+$/.test(value);
}
