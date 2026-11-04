import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import ViewsSection from '@/components/AppSidebar/ViewsSection';
import { MemoFilterProvider } from '@/contexts/MemoFilterContext';

vi.mock('@/utils/i18n', () => ({
  useTranslate: () => (key: string) => key,
  getLocaleDirection: () => 'ltr', getLocaleWithFallback: () => 'en',
  loadLocale: vi.fn(), subscribeToLocaleDirection: () => () => {},
}));
vi.mock('@/contexts/AppSidebarContext', () => ({ useAppSidebar: () => ({ setMobileOpen: vi.fn() }) }));
vi.mock('@/hooks/useCurrentUser', () => ({ default: () => ({ name: 'users/alice' }) }));
vi.mock('@/hooks/useUserQueries', () => ({
  useMemoViews: () => ({ data: [{ name: 'users/alice/memoViews/one', title: 'Board' }], isPending: false, isError: false }),
  userKeys: { memoViews: () => [] },
}));

it('keeps the actual sidebar Views menu fixed during document scrolling', async () => {
  document.body.style.height = '3000px';
  render(<MemoryRouter><QueryClientProvider client={new QueryClient()}><MemoFilterProvider>
    <div style={{ position: 'fixed', top: '100px', left: '40px' }}><ViewsSection /></div>
  </MemoFilterProvider></QueryClientProvider></MemoryRouter>);
  fireEvent.click(screen.getByRole('button', { name: 'common.edit Board' }));
  const menu = await screen.findByRole('menu');
  const positioner = menu.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('fixed');
  const before = positioner.getBoundingClientRect().top;
  window.scrollTo(0, 300);
  expect(window.scrollY).toBeGreaterThan(0);
  expect(Math.abs(positioner.getBoundingClientRect().top - before)).toBeLessThan(2);
});
