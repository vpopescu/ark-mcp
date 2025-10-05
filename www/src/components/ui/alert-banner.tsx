import type { ReactNode } from "react"
import { cn } from "@/lib/utils"
import { X } from "lucide-react"

type Variant = "destructive" | "warning" | "info" | "success"

export function AlertBanner({
  variant = "destructive",
  className,
  children,
  role = "alert",
  onClose,
  closeLabel = "Dismiss",
}: {
  variant?: Variant
  className?: string
  children: ReactNode
  role?: string
  onClose?: () => void
  closeLabel?: string
}) {
  return (
    <div role={role} className={cn("ark-alert-banner", `ark-alert--${variant}`, className)}>
      <div className="ark-alert-inner">
        <div className="ark-alert-message">{children}</div>
        {onClose ? (
          <button
            type="button"
            aria-label={closeLabel}
            onClick={onClose}
            className="ark-alert-close"
          >
            <X className="ark-alert-close-icon" />
          </button>
        ) : null}
      </div>
    </div>
  )
}

export default AlertBanner
