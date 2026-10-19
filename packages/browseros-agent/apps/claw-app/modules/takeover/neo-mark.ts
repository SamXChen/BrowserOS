/**
 * @license
 * Copyright 2025 BrowserOS
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

// The NEOB mark, inlined so the in-page bar renders it inside its shadow root
// without a web-accessible-resource round trip or the page CSP blocking it.
// Kept in sync with public/icons/neob.svg, which the sidebar and the new tab
// favicon load.

export const NEO_MARK_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" role="img" aria-label="NEOB"><title>NEOB</title><rect width="1024" height="1024" rx="224" fill="#175ce0"/><text x="512" y="512" text-anchor="middle" dominant-baseline="central" font-family="Helvetica Neue, Helvetica, Arial, sans-serif" font-size="440" font-weight="700" fill="#ffffff">NB</text></svg>`
