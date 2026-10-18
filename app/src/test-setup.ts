import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// jsdom has no layout engine: stub the few browser APIs the UI relies on.
Element.prototype.scrollIntoView = vi.fn();
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn()
  })
});
