/** Small windows (the minimum is 800 x 600) get the compact Home layout. */
export const viewport = $state({ compact: false, narrow: false });

/** Track the media queries; returns a cleanup function. */
export function watchViewport(): () => void {
  const compact = window.matchMedia('(max-width: 899px), (max-height: 659px)');
  const narrow = window.matchMedia('(max-width: 599px)');
  const sync = () => {
    viewport.compact = compact.matches;
    viewport.narrow = narrow.matches;
  };
  sync();
  compact.addEventListener('change', sync);
  narrow.addEventListener('change', sync);
  return () => {
    compact.removeEventListener('change', sync);
    narrow.removeEventListener('change', sync);
  };
}
