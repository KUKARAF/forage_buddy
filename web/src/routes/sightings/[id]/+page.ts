// Dynamic sighting-detail route. This cannot be prerendered (the id is only
// known at runtime), so we disable prerendering here — the adapter-static
// `200.html` fallback serves it and the page resolves client-side. SSR stays
// off, matching the rest of this SPA.
export const prerender = false;
export const ssr = false;
