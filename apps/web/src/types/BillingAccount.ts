export interface InvoiceRecord {
  id: string;
  label: string;
  amountUsd: number;
  status: 'paid' | 'processing';
  issuedAt: string;
}

export interface TransactionRecord {
  id: string;
  label: string;
  amountUsd: number;
  kind: 'top-up' | 'usage' | 'refund';
  at: string;
}

export interface BillingAccount {
  subscriptionPlanId: string;
  billingCycle: 'monthly' | 'annual';
  nextRenewal: string;
  paymentMethod: string;
  walletBalanceUsd: number;
  autoRechargeEnabled: boolean;
  autoRechargeThresholdUsd: number;
  autoRechargeAmountUsd: number;
  invoices: InvoiceRecord[];
  transactions: TransactionRecord[];
}
