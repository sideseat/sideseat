export function prefetchOnIdle(...loaders: Array<() => Promise<unknown>>): () => void {
  const run = () => loaders.forEach((l) => l().catch(() => {}));
  if (typeof requestIdleCallback !== "undefined") {
    const id = requestIdleCallback(run);
    return () => cancelIdleCallback(id);
  }
  const id = setTimeout(run, 1);
  return () => clearTimeout(id);
}
