export interface SubscriptionPrice {
  monthlyUsd: number;
  annualMonthlyUsd: number;
}

export interface SubscriptionPlan {
  id: string;
  nameEn: string;
  nameZh: string;
  taglineEn: string;
  taglineZh: string;
  featured?: boolean;
  price: SubscriptionPrice;
  featuresEn: string[];
  featuresZh: string[];
}
