import { useMemo } from 'react'
import { Bar, BarChart, CartesianGrid, XAxis, YAxis, Cell } from 'recharts'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { useSharedMetrics } from '@/lib/metrics-bus'
import { useChartColors } from '@/lib/chart-colors'
import { cn } from '@/lib/utils'

type Row = { name: string; value: number; plugin: string; tool: string }

function ToolCallsChart({ topN = 10 }: { topN?: number }) {
  const { samples, toolCallTotals } = useSharedMetrics()
  // Resolve CSS variables to concrete color strings so SVG "fill" works in all browsers
  const resolvedColors = useChartColors()
  const getColor = (i: number) => resolvedColors[i % resolvedColors.length]

  const data = useMemo<Row[]>(() => {
    // Prefer persisted totals if present
    const totalsEntries = Object.entries(toolCallTotals || {})
    if (totalsEntries.length > 0) {
      const rows: Row[] = totalsEntries.map(([key, v]) => ({
        name: `${v.plugin}/${v.tool}`,
        plugin: v.plugin,
        tool: v.tool,
        value: Math.round(v.total || 0),
      }))
      const top = rows.sort((a, b) => b.value - a.value).slice(0, topN)
      return top.sort((a, b) => a.name.localeCompare(b.name))
    }

    if (!samples.length) return []
    const agg = new Map<string, { plugin: string; tool: string; value: number }>()
    for (const s of samples) {
      // Prometheus exporter appends _total to counter names; accept both just in case
      if (!(s.name === 'ark_tool_calls_total' || s.name === 'ark_tool_calls')) continue
      const plugin = s.labels.plugin || 'unknown'
      const tool = s.labels.tool || 'unknown'
      const key = `${plugin}::${tool}`
      const prev = agg.get(key) || { plugin, tool, value: 0 }
      prev.value += s.value
      agg.set(key, prev)
    }
    const top = Array.from(agg.values())
      .sort((a, b) => b.value - a.value)
      .slice(0, topN)
      .map(({ plugin, tool, value }) => ({
        name: `${plugin}/${tool}`,
        plugin,
        tool,
        value: Math.round(value),
      }))
    // Order alphabetically by name for display
    return top.sort((a, b) => a.name.localeCompare(b.name))
  }, [samples, topN, toolCallTotals])

  return (
    <ChartContainer
      config={{ calls: { label: 'tool calls', color: 'hsl(var(--chart-6, var(--primary)))' } }}
      className="h-[260px]"
    >
      <BarChart data={data} margin={{ left: 6, right: 6, top: 6, bottom: 6 }}>
        <CartesianGrid vertical={false} />
        <XAxis dataKey="name" tickLine={false} axisLine={false} interval={0} angle={-20} textAnchor="end" height={60} />
        <YAxis width={48} tickLine={false} axisLine={false} tickFormatter={(v) => String(Math.round(v))} />
        <ChartTooltip cursor={false} content={<ChartTooltipContent hideLabel nameKey="name" />} />
        <Bar dataKey="value" radius={4} name="calls">
          {data.map((_, idx) => (
            <Cell key={`cell-${idx}`} fill={getColor(idx)} />
          ))}
        </Bar>
      </BarChart>
    </ChartContainer>
  )
}

export function ToolCallsCard({ className }: { className?: string }) {
  return (
    <Card className={cn(className)}>
      <CardHeader>
        <CardTitle className="ark-metrics-card-title">Number of calls by tool</CardTitle>
        <CardDescription className="ark-metrics-card-subtitle">Total calls since service start</CardDescription>
      </CardHeader>
      <CardContent>
        <ToolCallsChart />
      </CardContent>
    </Card>
  )
}

export default ToolCallsCard
