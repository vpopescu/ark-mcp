import { Github } from "lucide-react"

export function Footer() {
  return (
    <footer className="ark-footer">
      <div className="ark-footer-inner">
        <Github className="ark-footer-icon" aria-hidden="true" />
        <span className="ark-footer-text">github.com/vpopescu/ark-mcp</span>
      </div>
    </footer>
  )
}
