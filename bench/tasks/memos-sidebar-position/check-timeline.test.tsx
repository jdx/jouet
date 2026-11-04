import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import MemoDisplaySettingMenu from '@/components/MemoDisplaySettingMenu';
import { ViewProvider } from '@/contexts/ViewContext';

vi.mock('@/utils/i18n', () => ({
  useTranslate: () => (key: string) => key,
}));

it('keeps the actual sidebar Timeline menu fixed during document scrolling', async () => {
  document.body.style.height = '3000px';
  render(
    <div style={{ position: 'fixed', top: '100px', left: '40px' }}>
      <ViewProvider><MemoDisplaySettingMenu /></ViewProvider>
    </div>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'common.timeline · memo.view-options' }));
  const popup = await screen.findByRole('dialog', { name: 'common.timeline · memo.view-options' });
  const positioner = popup.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('fixed');
  const before = positioner.getBoundingClientRect().top;
  window.scrollTo(0, 300);
  expect(window.scrollY).toBeGreaterThan(0);
  expect(Math.abs(positioner.getBoundingClientRect().top - before)).toBeLessThan(2);
});
