import { useMemo } from 'react'
import { Bar, BarChart, CartesianGrid, XAxis, YAxis, Cell } from 'recharts'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { ChartContainer, ChartTooltip, ChartTooltipContent } from '@/components/ui/chart'
import { useSharedMetrics } from '@/lib/metrics-bus'
import { useChartColors } from '@/lib/chart-colors'
import { cn } from '@/lib/utils'

type Row = { name: string; value: number; plugin: string; tool: string }

function ToolLatencyChart({ topN = 10 }: { topN?: number }) {
  const { samples } = useSharedMetrics()

  const resolvedColors = useChartColors()
  const getColor = (i: number) => resolvedColors[i % resolvedColors.length]

  const data = useMemo<Row[]>(() => {
    if (!samples.length) return []
    const agg = new Map<string, { plugin: string; tool: string; sum: number; count: number }>()
    for (const s of samples) {
      if (s.name !== 'ark_tool_latency_ms_sum' && s.name !== 'ark_tool_latency_ms_count') continue
      const plugin = s.labels.plugin || 'unknown'
      const tool = s.labels.tool || 'unknown'
      const key = `${plugin}::${tool}`
      const prev = agg.get(key) || { plugin, tool, sum: 0, count: 0 }
      if (s.name === 'ark_tool_latency_ms_sum') prev.sum += s.value
      else if (s.name === 'ark_tool_latency_ms_count') prev.count += s.value
      agg.set(key, prev)
    }
    const rows: Row[] = []
    for (const { plugin, tool, sum, count } of agg.values()) {
      if (count <= 0) continue
      const avg = sum / count
      rows.push({ name: `${plugin}/${tool}`, plugin, tool, value: Math.round(avg) })
    }
    const top = rows
      .sort((a, b) => b.value - a.value)
      .slice(0, topN)
    // Order alphabetically by name for display
    return top.sort((a, b) => a.name.localeCompare(b.name))
  }, [samples, topN])

  return (
    <ChartContainer
      config={{ latency: { label: 'latency (ms)', color: 'hsl(var(--chart-6, var(--primary)))' } }}
      className="h-[260px]"
    >
      <BarChart data={data} margin={{ left: 6, right: 6, top: 6, bottom: 6 }}>
        <CartesianGrid vertical={false} />
        <XAxis dataKey="name" tickLine={false} axisLine={false} interval={0} angle={-20} textAnchor="end" height={60} />
        <YAxis width={48} tickLine={false} axisLine={false} tickFormatter={(v) => String(Math.round(v))} />
        <ChartTooltip cursor={false} content={<ChartTooltipContent hideLabel nameKey="name" />} />
        <Bar dataKey="value" radius={4} name="latency (ms)">
          {data.map((_, idx) => (
            <Cell key={`cell-lat-${idx}`} fill={getColor(idx)} />
          ))}
        </Bar>
      </BarChart>
    </ChartContainer>
  )
}

export function ToolLatencyCard({ className }: { className?: string }) {
  return (
    <Card className={cn(className)}>
      <CardHeader>
        <CardTitle className="ark-metrics-card-title">Tool Latency</CardTitle>
        <CardDescription className="ark-metrics-card-subtitle">Average latency (ms) by plugin/tool</CardDescription>
      </CardHeader>
      <CardContent>
        <ToolLatencyChart />
      </CardContent>
    </Card>
  )
}

export default ToolLatencyCard
