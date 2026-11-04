import { create } from '@bufbuild/protobuf';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import AppSidebar from '@/components/AppSidebar';
import { MemoFilterProvider } from '@/contexts/MemoFilterContext';
import { ViewProvider } from '@/contexts/ViewContext';
import { type Memo, MemoSchema } from '@/types/proto/api/memo_service_pb';

const sidebarState = vi.hoisted(() => ({ memoDetail: undefined as { memo: Memo; from: string } | undefined }));

vi.mock('@/components/MemosLogo', () => ({ default: () => null }));
vi.mock('@/components/UserMenu', () => ({ default: () => null }));
vi.mock('@/components/CreateSpaceDialog', () => ({ default: () => null }));
vi.mock('@/components/StatisticsView', () => ({ default: () => null }));
vi.mock('@/contexts/AppSidebarContext', () => ({ useAppSidebar: () => ({
  attachmentSection: 'all', setAttachmentSection: vi.fn(), inboxFilter: 'all', setInboxFilter: vi.fn(),
  memoDetail: sidebarState.memoDetail, setMemoDetail: vi.fn(), mobileOpen: false, setMobileOpen: vi.fn(),
  quickFindOpen: false, setQuickFindOpen: vi.fn(),
}) }));
vi.mock('@/contexts/AuthContext', () => ({ useAuth: () => ({ isInitialized: true }) }));
vi.mock('@/contexts/GlobalMemoEditorContext', () => ({ useGlobalMemoEditor: () => ({ canOpen: false }) }));
vi.mock('@/contexts/InstanceContext', () => ({ useInstance: () => ({ isInitialized: true, profile: { instanceUrl: 'https://memos.example/' } }) }));
vi.mock('copy-to-clipboard', () => ({ default: vi.fn() }));
vi.mock('react-hot-toast', () => ({ default: { success: vi.fn() }, toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock('@/components/MemoDetailSidebar/MemoOutline', () => ({ default: () => null }));
vi.mock('@/components/MemoDetailSidebar/MemoSharePanel', () => ({ default: () => null }));
vi.mock('@/components/MemoMetadata/Relation/useResolvedRelationMemos', () => ({ useResolvedRelationMemos: () => ({}) }));
vi.mock('@/contexts/SpaceContext', () => ({ useSpaceContext: () => ({ spaces: [], duplicateSpaceTitles: new Set(), isLoadingSpaces: false, isSpacesError: false }) }));
vi.mock('@/hooks/useCurrentUser', () => ({ default: () => ({ name: 'users/alice' }) }));
vi.mock('@/hooks/useFilteredMemoStats', () => ({ useFilteredMemoStats: () => ({ statistics: { activityStats: {}, timeBasis: 'create_time' }, tags: { alpha: 1 } }) }));
vi.mock('@/hooks/useAttachmentLibrary', () => ({ useAttachmentLibraryStats: () => ({ stats: {} }) }));
vi.mock('@/hooks/useMediaQuery', () => ({ default: () => true }));
vi.mock('@/hooks/useUserQueries', () => ({
  userKeys: { memoViews: () => [] },
  useMemoViews: () => ({ data: [{ name: 'users/alice/memoViews/one', title: 'Board' }], isPending: false, isError: false }),
  useNotifications: () => ({ data: [] }), useUser: () => ({ data: undefined }),
  useDeleteUser: vi.fn(), useUpdateUser: vi.fn(), useUserSettings: vi.fn(), useUserStats: vi.fn(),
  useUserUsage: vi.fn(), useAllUserStats: vi.fn(), useArchiveNotification: vi.fn(), useDeleteNotification: vi.fn(),
  useTagCounts: vi.fn(), userDetailQueryOptions: vi.fn(), useListUsers: vi.fn(), useUpdateUserSetting: vi.fn(), useUpdateUserGeneralSetting: vi.fn(), useUsersByNames: vi.fn(), useUsersByUsernames: vi.fn(),
}));
vi.mock('@/i18n', () => ({ default: { language: 'en' }, locales: [] }));
vi.mock('@/utils/i18n', () => ({ useTranslate: () => (key: string) => key,
  getLocaleDirection: () => 'ltr', getLocaleWithFallback: () => 'en', loadLocale: vi.fn(), subscribeToLocaleDirection: () => () => {},
  getLocaleDisplayName: (locale: string) => locale, normalizeLocaleSearchText: (value: string) => value,
  localeMatchesSearch: () => true, getLocaleSearchLabels: () => [], isValidLocale: () => true, findNearestMatchedLanguage: () => 'en', applyDocumentLocale: vi.fn(), applyLocaleEarly: vi.fn() }));

function renderSidebar(path = '/') {
  document.body.style.height = '3000px';
  window.scrollTo(0, 0);
  render(<MemoryRouter initialEntries={[path]}><QueryClientProvider client={new QueryClient()}><MemoFilterProvider><ViewProvider>
    <div style={{ position: 'fixed', top: '100px', left: '40px' }}><AppSidebar /></div>
  </ViewProvider></MemoFilterProvider></QueryClientProvider></MemoryRouter>);
}

async function remainsFixed(popup: HTMLElement) {
  const positioner = popup.parentElement as HTMLElement;
  await waitFor(() => expect(positioner.style.opacity).not.toBe('0'));
  expect(positioner.style.position).toBe('fixed');
  const before = positioner.getBoundingClientRect().top;
  window.scrollTo(0, 300);
  expect(window.scrollY).toBeGreaterThan(0);
  expect(Math.abs(positioner.getBoundingClientRect().top - before)).toBeLessThan(2);
}

it('keeps sidebar Timeline settings fixed while scrolling', async () => {
  renderSidebar();
  fireEvent.click(screen.getByRole('button', { name: 'common.timeline · memo.view-options' }));
  await remainsFixed(await screen.findByRole('dialog', { name: 'common.timeline · memo.view-options' }));
});

it('keeps sidebar Tags fixed while scrolling', async () => {
  renderSidebar();
  fireEvent.click(screen.getByRole('button', { name: 'common.tags: common.more' }));
  await remainsFixed(await screen.findByRole('menu'));
});

it('keeps sidebar Views fixed while scrolling', async () => {
  renderSidebar();
  fireEvent.click(screen.getByRole('button', { name: 'common.edit Board' }));
  await remainsFixed(await screen.findByRole('menu'));
});

it('keeps sidebar memo Share fixed while scrolling', async () => {
  sidebarState.memoDetail = { memo: create(MemoSchema, { name: 'memos/detail', creator: 'users/alice', content: 'Body' }), from: '/' };
  renderSidebar('/memos/detail');
  fireEvent.click(screen.getByRole('button', { name: /share/i }));
  await remainsFixed(await screen.findByRole('menu'));
});
