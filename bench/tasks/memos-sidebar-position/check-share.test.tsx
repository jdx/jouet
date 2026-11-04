import { create } from '@bufbuild/protobuf';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import MemoDetailSidebar from '@/components/MemoDetailSidebar/MemoDetailSidebar';
import { MemoSchema } from '@/types/proto/api/memo_service_pb';

vi.mock('copy-to-clipboard', () => ({ default: vi.fn() }));
vi.mock('react-hot-toast', () => ({ default: { success: vi.fn() } }));
vi.mock('@/components/MemoDetailSidebar/MemoOutline', () => ({ default: () => null }));
vi.mock('@/components/MemoDetailSidebar/MemoSharePanel', () => ({ default: () => null }));
vi.mock('@/components/MemoMetadata/Relation/useResolvedRelationMemos', () => ({ useResolvedRelationMemos: () => ({}) }));
vi.mock('@/contexts/InstanceContext', () => ({ useInstance: () => ({ profile: { instanceUrl: 'https://memos.example/' } }) }));
vi.mock('@/hooks/useCurrentUser', () => ({ default: () => ({ name: 'users/alice' }) }));
vi.mock('@/utils/i18n', () => ({
  useTranslate: () => (key: string) => key,
  getLocaleDirection: () => 'ltr',
  getLocaleWithFallback: () => 'en',
  loadLocale: vi.fn(),
  subscribeToLocaleDirection: () => () => {},
}));


it('keeps the actual memo-detail Share menu fixed during document scrolling', async () => {
  document.body.style.height = '3000px';
  const memo = create(MemoSchema, { name: 'memos/detail', creator: 'users/alice', content: 'Body' });
  render(
    <MemoryRouter><div style={{ position: 'fixed', top: '100px', left: '40px' }}>
      <MemoDetailSidebar memo={memo} parentPage="/" />
    </div></MemoryRouter>,
  );
  fireEvent.click(screen.getByRole('button', { name: /share/i }));
  const menu = await screen.findByRole('menu');
  const positioner = menu.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('fixed');
  const before = positioner.getBoundingClientRect().top;
  window.scrollTo(0, 300);
  expect(window.scrollY).toBeGreaterThan(0);
  expect(Math.abs(positioner.getBoundingClientRect().top - before)).toBeLessThan(2);
});
