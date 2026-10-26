/** Device-owned diagnostics policy. Workspace settings never own an exporter. */
export interface DesktopTelemetryPreferences {
  telemetryEnabled?: boolean;
  telemetryEndpoint?: string;
}

export interface DesktopTelemetryConfig {
  enabled: boolean;
  endpoint: string;
  headers: Record<string, string>;
}

const PRIVACY_FLAGS = ['DISABLE_TELEMETRY', 'DO_NOT_TRACK', 'LINGXI_DISABLE_NONESSENTIAL_TRAFFIC', 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC'] as const;
const CONTENT_FLAGS = ['USER_PROMPTS', 'TOOL_DETAILS', 'TOOL_CONTENT', 'ASSISTANT_RESPONSES', 'RAW_API_BODIES'] as const;
const truthy = (value: string | undefined): boolean => value !== undefined && ['1', 'true', 'yes', 'on'].includes(value.trim().toLowerCase());

export function telemetryPrivacyDisabled(environment: NodeJS.ProcessEnv): boolean {
  return truthy(environment.DO_NOT_TRACK) || PRIVACY_FLAGS.some(name => name !== 'DO_NOT_TRACK' && Boolean(environment[name]));
}

export function parseTelemetryEndpoint(value: unknown): string | undefined {
  if (typeof value !== 'string' || value.length > 2048) return undefined;
  try {
    const url = new URL(value);
    const loopback = ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
    if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) return undefined;
    if (url.username || url.password || url.search || url.hash) return undefined;
    return url.toString().replace(/\/$/, '');
  } catch { return undefined; }
}

export function resolveDesktopTelemetry(
  source: NodeJS.ProcessEnv,
  preferences: DesktopTelemetryPreferences = {},
): DesktopTelemetryConfig {
  const requestedEndpoint = preferences.telemetryEndpoint ?? source.OTEL_EXPORTER_OTLP_ENDPOINT ?? 'http://127.0.0.1:4318';
  const endpoint = parseTelemetryEndpoint(requestedEndpoint);
  const headers: Record<string, string> = {};
  for (const entry of (source.OTEL_EXPORTER_OTLP_HEADERS ?? '').split(',').slice(0, 16)) {
    const equals = entry.indexOf('=');
    if (equals < 1) continue;
    const key = entry.slice(0, equals).trim();
    try {
      const value = decodeURIComponent(entry.slice(equals + 1).trim());
      if (/^[a-zA-Z0-9-]{1,64}$/.test(key) && value.length <= 4096 && !/[\r\n]/.test(value)) headers[key] = value;
    } catch { /* An invalid optional header cannot break application startup. */ }
  }
  return {
    enabled: Boolean(endpoint) && !telemetryPrivacyDisabled(source)
      && (preferences.telemetryEnabled ?? truthy(source.LINGXI_ENABLE_TELEMETRY)),
    endpoint: endpoint ?? 'http://127.0.0.1:4318',
    headers,
  };
}

/** Apply after merging project environment; no project can change destination or consent. */
export function protectBridgeTelemetryEnvironment(
  environment: NodeJS.ProcessEnv,
  source: NodeJS.ProcessEnv,
  config: DesktopTelemetryConfig,
): NodeJS.ProcessEnv {
  const result = { ...environment };
  const disabled = telemetryPrivacyDisabled(source) || telemetryPrivacyDisabled(environment);
  for (const key of Object.keys(result)) {
    if (/^(?:OTEL_|LINGXI_OTEL_|CLAUDE_CODE_OTEL_)/.test(key)
      || key === 'LINGXI_ENABLE_TELEMETRY' || key === 'CLAUDE_CODE_ENABLE_TELEMETRY'
      || key === 'CLAUDE_CODE_ENHANCED_TELEMETRY_BETA') delete result[key];
  }
  for (const flag of PRIVACY_FLAGS) {
    if (flag === 'DO_NOT_TRACK' ? truthy(source[flag]) : Boolean(source[flag])) result[flag] = 'true';
  }
  result.LINGXI_ENABLE_TELEMETRY = String(config.enabled && !disabled);
  result.OTEL_SERVICE_NAME = 'lingxi-desktop-bridge';
  result.OTEL_EXPORTER_OTLP_ENDPOINT = config.endpoint;
  result.OTEL_EXPORTER_OTLP_PROTOCOL = 'http/protobuf';
  result.OTEL_EXPORTER_OTLP_HEADERS = Object.entries(config.headers).map(([key, value]) => `${key}=${encodeURIComponent(value)}`).join(',');
  result.OTEL_METRICS_EXPORTER = 'otlp';
  result.OTEL_LOGS_EXPORTER = 'otlp';
  result.OTEL_TRACES_EXPORTER = 'otlp';
  result.OTEL_METRICS_INCLUDE_SESSION_ID = 'false';
  result.OTEL_METRICS_INCLUDE_ACCOUNT_UUID = 'false';
  for (const flag of CONTENT_FLAGS) result[`OTEL_LOG_${flag}`] = 'false';
  return result;
}
