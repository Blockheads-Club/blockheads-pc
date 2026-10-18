export interface ToastItem {
  id: number;
  message: string;
  tone: 'ok' | 'bad';
  actionLabel?: string;
  action?: () => void;
}

let seq = 0;
export const toasts = $state<{ items: ToastItem[] }>({ items: [] });

/** Show a short message; it disappears after 6 seconds (errors after 10). */
export function toast(message: string, action?: { label: string; run: () => void }, tone: 'ok' | 'bad' = 'ok') {
  const id = ++seq;
  toasts.items = [...toasts.items, { id, message, tone, actionLabel: action?.label, action: action?.run }];
  setTimeout(() => dismiss(id), tone === 'bad' ? 10_000 : 6000);
  return id;
}

export function dismiss(id: number) {
  toasts.items = toasts.items.filter((x) => x.id !== id);
}
