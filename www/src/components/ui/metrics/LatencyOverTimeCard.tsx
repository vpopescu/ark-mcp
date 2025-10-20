import { useEffect, useMemo, useState } from 'react'
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from 'recharts'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { ChartConfig, ChartContainer, ChartLegend, ChartLegendContent, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { useSharedMetrics, type LatencyPoint } from '@/lib/metrics-bus'
import { cn } from '@/lib/utils'
type TimeRange = '5m' | '1h' | '1d' | '1w' | '1mo'

const TIME_RANGES: { value: TimeRange; label: string; ms: number }[] = [
  { value: '5m', label: 'Last 5 minutes', ms: 5 * 60_000 },
  { value: '1h', label: 'Last hour', ms: 60 * 60_000 },
  { value: '1d', label: 'Last day', ms: 24 * 60 * 60_000 },
  { value: '1w', label: 'Last week', ms: 7 * 24 * 60 * 60_000 },
  { value: '1mo', label: 'Last month', ms: 30 * 24 * 60 * 60_000 },
]

function ToolLatencyHistoryChart() {
  const { latencyHistory, perToolLatencyHistory } = useSharedMetrics()
  const [range, setRange] = useState<TimeRange>(() => {
    try {
      const v = localStorage.getItem('ark.ui.metrics.latencyRange') as TimeRange | null
      if (v && TIME_RANGES.some(r => r.value === v)) return v
    } catch { }
    return '5m'
  })
  useEffect(() => { try { localStorage.setItem('ark.ui.metrics.latencyRange', range) } catch { } }, [range])

  // Build tool options from reported series
  const toolOptions = useMemo(() => {
    const out: Array<{ key: string; label: string }> = []
    for (const [key, s] of Object.entries(perToolLatencyHistory || {})) {
      out.push({ key, label: `${s.plugin}/${s.tool}` })
    }
    return out.sort((a, b) => a.label.localeCompare(b.label))
  }, [perToolLatencyHistory])

  const [selectedTool, setSelectedTool] = useState<string>('')
  useEffect(() => {
    if (!toolOptions.length) { setSelectedTool(''); return }
    if (!selectedTool || !toolOptions.find(o => o.key === selectedTool)) {
      setSelectedTool(toolOptions[0].key)
    }
  }, [toolOptions, selectedTool])

  const now = Date.now()
  const start = now - (TIME_RANGES.find(r => r.value === range)?.ms || 5 * 60_000)

  // Build merged dataset with t, overall, toolAvg
  const data = useMemo(() => {
    const overallPts = (latencyHistory || []).filter(p => p.t >= start)
    const toolPts: LatencyPoint[] = selectedTool && perToolLatencyHistory[selectedTool]
      ? (perToolLatencyHistory[selectedTool].points || []).filter(p => p.t >= start)
      : []
    const ts = new Set<number>()
    overallPts.forEach(p => ts.add(p.t))
    toolPts.forEach(p => ts.add(p.t))
    const times = Array.from(ts).sort((a, b) => a - b).filter(t => Number.isFinite(t))
    // If only one point, synth an earlier one to render area nicely
    if (times.length === 1) times.unshift(times[0] - 1000)
    const omap = new Map(overallPts.map(p => [p.t, Math.round(p.avgMs)]))
    const tmap = new Map(toolPts.map(p => [p.t, Math.round(p.avgMs)]))
    return times.map(t => ({
      t,
      overall: omap.get(t),
      toolAvg: tmap.get(t),
    })).filter(d => Number.isFinite(d.t))
  }, [latencyHistory, perToolLatencyHistory, selectedTool, start])

  const xTickFormatter = (ts: number) => {
    const d = new Date(ts)
    const span = (TIME_RANGES.find(r => r.value === range)?.ms || 0)
    if (span <= 24 * 60 * 60_000) {
      return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
    }
    return d.toLocaleDateString([], { month: 'short', day: 'numeric' })
  }

  // Chart config: overall -> chart-1, tool -> chart-2
  const selectedLabel = toolOptions.find(o => o.key === selectedTool)?.label || 'Selected tool'
  const chartConfig: ChartConfig = {
    overall: { label: 'All tools', color: 'var(--chart-1)' },
    tool: { label: selectedLabel, color: 'var(--chart-2)' },
  }

  return (
    <div className="w-full">
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-2">
          <div className="text-sm font-medium">Latency over time</div>
          <select className="text-xs border rounded px-2 py-1 bg-background" value={range} onChange={(e) => setRange(e.target.value as TimeRange)}>
            {TIME_RANGES.map(r => (<option key={r.value} value={r.value}>{r.label}</option>))}
          </select>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">Tool</span>
          <select className="text-xs border rounded px-2 py-1 bg-background" value={selectedTool} onChange={(e) => setSelectedTool(e.target.value)}>
            {toolOptions.map(o => (<option key={o.key} value={o.key}>{o.label}</option>))}
          </select>
        </div>
      </div>
      <ChartContainer config={chartConfig} className="w-full h-[320px] aspect-auto">
        {(!data.length || !toolOptions.length) ? (
          <div className="flex items-center justify-center text-xs text-muted-foreground">No data for selected interval.</div>
        ) : (
          <AreaChart data={data}>
            <defs>
              <linearGradient id="fill-overall" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="var(--color-overall)" stopOpacity={0.8} />
                <stop offset="95%" stopColor="var(--color-overall)" stopOpacity={0.1} />
              </linearGradient>
              <linearGradient id="fill-tool" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="var(--color-tool)" stopOpacity={0.8} />
                <stop offset="95%" stopColor="var(--color-tool)" stopOpacity={0.1} />
              </linearGradient>
            </defs>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="t"
              type="number"
              domain={[start, now]}
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              minTickGap={32}
              tickFormatter={(value) => xTickFormatter(Number(value))}
            />
            <YAxis width={48} tickLine={false} axisLine={false} domain={[0, 'auto']} tickCount={5} tickFormatter={(v) => String(Number(v).toFixed(1))} />
            <ChartTooltip
              cursor={false}
              content={({ active, payload, label }) => {
                if (active && payload && payload.length && typeof label === 'number') {
                  return <div className="text-sm">{xTickFormatter(label)}</div>
                }
                return null
              }}
            />
            <Area dataKey="overall" type="monotone" fill="url(#fill-overall)" stroke="var(--color-overall)" connectNulls strokeWidth={2} />
            <Area dataKey="toolAvg" type="monotone" fill="url(#fill-tool)" stroke="var(--color-tool)" connectNulls strokeWidth={2} />
            <ChartLegend content={<ChartLegendContent />} />
          </AreaChart>
        )}
      </ChartContainer>
    </div>
  )
}

export function LatencyOverTimeCard({ className }: { className?: string }) {
  return (
    <Card className={cn('w-full', className)}>
      <CardHeader>
        <CardTitle className="ark-metrics-card-title">Latency over time</CardTitle>
        <CardDescription className="ark-metrics-card-subtitle">Average tool-call latency for the selected period</CardDescription>
      </CardHeader>
      <CardContent>
        <ToolLatencyHistoryChart />
      </CardContent>
    </Card>
  )
}

export default LatencyOverTimeCard
