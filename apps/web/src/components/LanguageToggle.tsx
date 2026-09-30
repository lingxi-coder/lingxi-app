import { Locale, Region } from '../utils/locale';
import { IconGlobe } from './Icons';

interface LanguageToggleProps {
  locale: Locale;
  onLocaleChange: (locale: Locale) => void;
  region: Region;
  onRegionChange: (region: Region) => void;
}

export function LanguageToggle({
  locale,
  onLocaleChange,
  region,
  onRegionChange,
}: LanguageToggleProps) {
  return (
    <div className="control-group" role="group" aria-label="Language and region">
      <button className="control-chip" type="button" onClick={() => onLocaleChange(locale === 'en' ? 'zh' : 'en')}>
        <IconGlobe width={16} height={16} />
        <span>{locale === 'en' ? '中文' : 'EN'}</span>
      </button>
      <div className="segmented-control" aria-label="Region switch">
        <button
          type="button"
          className={region === 'global' ? 'active' : ''}
          onClick={() => onRegionChange('global')}
        >
          Global
        </button>
        <button
          type="button"
          className={region === 'china' ? 'active' : ''}
          onClick={() => onRegionChange('china')}
        >
          中国
        </button>
      </div>
    </div>
  );
}
