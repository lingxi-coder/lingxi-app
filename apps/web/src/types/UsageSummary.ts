export interface UsagePoint {
  day: string;
  requests: number;
  spendUsd: number;
}

export interface UsageBreakdownItem {
  label: string;
  value: number;
  share: number;
}

export interface UsageSummary {
  periodLabel: string;
  requests: number;
  tokensIn: number;
  tokensOut: number;
  latencyP50Ms: number;
  spendUsd: number;
  walletBalanceUsd: number;
  trend: UsagePoint[];
  byModel: UsageBreakdownItem[];
  byKey: UsageBreakdownItem[];
  bySource: UsageBreakdownItem[];
}
