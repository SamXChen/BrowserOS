import { cn } from '@/lib/utils'

export interface SidebarBrandingProps {
  expanded?: boolean
}

/**
 * Compact NEOB mark at the top of the sidebar: the tile stays visible when
 * collapsed, the wordmark fades in as the rail expands (fading rather than
 * sliding keeps the layout from shifting mid-animation).
 *
 * ONE ASSET, BOTH THEMES: the mark is a brand chip, not a themed surface —
 * same call the palette makes for --mcp-endpoint, which holds #0454ec in
 * both themes. Third-party logos in components/harness/harness-marks.tsx
 * keep their dark:hidden variants; this rule is only about our own mark.
 */
export function SidebarBranding({ expanded = false }: SidebarBrandingProps) {
  return (
    <div className="flex h-14 shrink-0 items-center gap-3 px-3">
      <img
        src="/icons/neob.svg"
        alt="NEOB"
        className="size-8 shrink-0 rounded-md shadow-card"
      />
      <span
        className={cn(
          'truncate font-extrabold text-base tracking-tight transition-opacity duration-200',
          expanded ? 'opacity-100' : 'opacity-0',
        )}
      >
        NEOB
      </span>
    </div>
  )
}
