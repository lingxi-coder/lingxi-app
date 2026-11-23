import { providerBrandMark } from '../../../../../../../assets/brand/provider-icons/marks';

export function ProviderBrandIcon({ provider, className }: {
  provider: { id: string; label: string };
  className: string;
}) {
  const mark = providerBrandMark(provider.id);
  return <span className={className} aria-hidden="true">
    {mark?.color ? <img className="provider-brand-mark" src={mark.source} alt="" />
      : mark ? <span className="provider-brand-mark provider-brand-mask" style={{ maskImage: `url("${mark.source}")` }} />
      : provider.label.slice(0, 1)}
  </span>;
}
