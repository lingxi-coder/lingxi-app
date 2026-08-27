import { UsagePoint } from '../../types/UsageSummary';

interface LineChartProps {
  data: UsagePoint[];
}

export function LineChart({ data }: LineChartProps) {
  const width = 640;
  const height = 220;
  const padding = 28;
  const maxSpend = Math.max(...data.map((point) => point.spendUsd), 1);
  const stepX = data.length > 1 ? (width - padding * 2) / (data.length - 1) : width - padding * 2;

  const path = data
    .map((point, index) => {
      const x = padding + stepX * index;
      const y = height - padding - (point.spendUsd / maxSpend) * (height - padding * 2);
      return `${index === 0 ? 'M' : 'L'} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(' ');

  const areaPath = `${path} L ${width - padding} ${height - padding} L ${padding} ${height - padding} Z`;

  return (
    <div className="chart-shell">
      <svg viewBox={`0 0 ${width} ${height}`} className="line-chart" role="img" aria-label="Usage trend chart">
        <defs>
          <linearGradient id="lingxi-chart-fill" x1="0%" y1="0%" x2="0%" y2="100%">
            <stop offset="0%" stopColor="rgba(16, 148, 133, 0.3)" />
            <stop offset="100%" stopColor="rgba(16, 148, 133, 0.02)" />
          </linearGradient>
        </defs>
        {[0, 0.25, 0.5, 0.75, 1].map((ratio) => {
          const y = padding + (height - padding * 2) * ratio;
          return <line key={ratio} x1={padding} y1={y} x2={width - padding} y2={y} className="chart-grid-line" />;
        })}
        <path d={areaPath} fill="url(#lingxi-chart-fill)" />
        <path d={path} className="chart-line" />
        {data.map((point, index) => {
          const x = padding + stepX * index;
          const y = height - padding - (point.spendUsd / maxSpend) * (height - padding * 2);
          return (
            <g key={point.day}>
              <circle cx={x} cy={y} r="4.5" className="chart-point" />
              <text x={x} y={height - 10} textAnchor="middle" className="chart-label">
                {point.day}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}
