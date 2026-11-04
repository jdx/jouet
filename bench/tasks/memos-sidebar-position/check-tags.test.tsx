import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import TagsSection from '@/components/AppSidebar/TagsSection';
import { MemoFilterProvider } from '@/contexts/MemoFilterContext';

vi.mock('@/utils/i18n', () => ({
  useTranslate: () => (key: string) => key,
  getLocaleDirection: () => 'ltr', getLocaleWithFallback: () => 'en',
  loadLocale: vi.fn(), subscribeToLocaleDirection: () => () => {},
}));

it('keeps the actual sidebar Tags menu fixed during document scrolling', async () => {
  document.body.style.height = '3000px';
  render(<MemoryRouter><MemoFilterProvider><div style={{ position: 'fixed', top: '100px', left: '40px' }}>
    <TagsSection tagCount={{ alpha: 1 }} scope="home" />
  </div></MemoFilterProvider></MemoryRouter>);
  fireEvent.click(screen.getByRole('button', { name: 'common.tags: common.more' }));
  const menu = await screen.findByRole('menu');
  const positioner = menu.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('fixed');
  const before = positioner.getBoundingClientRect().top;
  window.scrollTo(0, 300);
  expect(window.scrollY).toBeGreaterThan(0);
  expect(Math.abs(positioner.getBoundingClientRect().top - before)).toBeLessThan(2);
});
