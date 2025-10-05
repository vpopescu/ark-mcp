import React, { useMemo } from "react"
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { useSharedMetrics } from "@/lib/metrics-bus"

export function McpRequestsCard() {
  const { samples } = useSharedMetrics()
  const total = useMemo(() => {
    let sum = 0
    for (const s of samples) {
      // Sum all tool calls across plugins/tools; accept both counter name forms
      if (s.name === 'ark_tool_calls_total' || s.name === 'ark_tool_calls') {
        sum += s.value
      }
    }
    return Math.round(sum)
  }, [samples])
  return (
    <Card className="ark-numeric-stat ark-metrics-card ark-metrics-card-4">
      <CardHeader>
        <CardTitle className="ark-metrics-card-title">Number of tool requests</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="text-3xl font-semibold tracking-tight ark-numeric-state-value">{total.toLocaleString()}</div>
      </CardContent>
      <CardFooter className="ark-metrics-card-footer">
        requests since startup
      </CardFooter>
    </Card>
  )
}

export default McpRequestsCard
