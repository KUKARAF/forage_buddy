// This app is a pure client-side SPA (adapter-static): it is served as static
// files both from a plain web deployment and from inside a Tauri-wrapped
// Android build, and talks to a remote Rust backend over HTTP. There is no
// SSR and no Node/edge server, so we prerender the shell and disable SSR
// entirely; dynamic data is fetched client-side (fallback: '200.html'
// resolves unknown paths in the browser).
export const prerender = true;
export const ssr = false;
