import { expect, it } from 'vitest';
import { page, userEvent } from 'vitest/browser';
import { fakeAdminEndpoint, fakeAutomations, renderAdminApp } from '@test-utils/acceptance';
import { detail, setup } from './run-history.test-utils';

it('shows a tier added while the automation editor was closed', async () => {
  setup();
  const tiers = [{ id: 'alpha', name: 'Alpha', type: 'paid', active: true }];
  const automation = {
    ...detail('first'), slug: 'member-welcome-email-paid',
    trigger_tier_scope: 'selected_paid', trigger_tier_ids: ['alpha'],
  };
  fakeAdminEndpoint('GET', '/automations/first/', { automations: [automation] });
  const save = fakeAdminEndpoint('PUT', '/automations/first/', ({ body }) => ({
    automations: [{ ...automation, ...(body as { automations: Record<string, unknown>[] }).automations[0] }],
  }));
  fakeAdminEndpoint('GET', /\/tiers\/\?/, () => ({ tiers }));
  fakeAutomations([]);
  await renderAdminApp('/automations/first', { labs: { automations: true } });
  const card = page.getByRole('article', { name: 'Paid subscription started', exact: true });
  await expect.element(card).toHaveTextContent('Alpha');
  await page.getByRole('link', { name: 'Back to automations' }).click();
  await expect.element(page.getByRole('heading', { name: 'No automations yet' })).toBeVisible();
  tiers.push({ id: 'beta', name: 'Beta', type: 'paid', active: true });
  window.history.back();
  await expect.element(card).toHaveTextContent('Alpha');
  await page.getByRole('combobox', { name: 'Tiers', exact: true }).click();
  const alpha = page.getByRole('option', { name: 'Alpha', exact: true });
  await expect.element(alpha).toHaveAttribute('aria-selected', 'true');
  await page.getByRole('option', { name: 'Beta', exact: true }).click();
  await userEvent.keyboard('{Escape}');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => save.requests.length).toBe(1);
  expect((save.requests[0].body as { automations: unknown[] }).automations[0]).toMatchObject({
    trigger_tier_scope: 'selected_paid',
    trigger_tier_ids: ['alpha', 'beta'],
  });
});
