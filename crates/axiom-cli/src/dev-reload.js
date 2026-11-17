
// Local development uses polling instead of a WebSocket so the dev server stays dependency-free.
let __devVersion = null;
setInterval(async () => {
  try {
    const response = await fetch('/__synthetix/status', { cache: 'no-store' });
    if (!response.ok) return;
    const { version, error } = await response.json();
    if (__devVersion === null) __devVersion = version;
    else if (version !== __devVersion) location.reload();
    // Application errors stay underneath and return once the build error clears.
    if ((error ?? null) !== __buildError) {
      __buildError = error ?? null;
      renderErrorOverlay();
    }
  } catch (_) {}
}, 500);
