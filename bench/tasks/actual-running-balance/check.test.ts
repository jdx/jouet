import { expect, test } from './fixtures';
import { ConfigurationPage } from './page-models/configuration-page';
import { Navigation } from './page-models/navigation';

test('running balances refresh after repeated edits', async ({ browser }) => {
  const page = await browser.newPage();
  try {
    await page.goto('/');
    // Confirm the runner's configured browser context is used for this page.
    expect(await page.evaluate(() => navigator.userAgent)).toBe('playwright');
    await new ConfigurationPage(page).createTestFile();
    const account = await new Navigation(page).createAccount({
      name: 'Balance check', offBudget: false, balance: 0,
    });
    await account.waitFor();
    await account.createSingleTransaction({ payee: '', notes: 'older', credit: '17.00' });
    await account.createSingleTransaction({ payee: '', notes: 'newer', credit: '5.00' });
    await account.accountMenuButton.click();
    await page.getByRole('button', { name: 'Show running balance' }).click();
    const older = account.transactionTableRow.filter({ hasText: 'older' });
    const newer = account.transactionTableRow.filter({ hasText: 'newer' });
    const credit = older.getByTestId('credit');
    const olderBalance = older.getByTestId('balance');
    const newerBalance = newer.getByTestId('balance');
    await expect(olderBalance).toHaveText('17.00');
    await expect(newerBalance).toHaveText('22.00');
    for (const [amount, expectedNewer] of [['42.00', '47.00'], ['6.50', '11.50']]) {
      await credit.click();
      const input = credit.getByRole('textbox');
      await input.selectText();
      await input.pressSequentially(amount);
      await page.keyboard.press('Tab');
      await expect(olderBalance).toHaveText(amount);
      await expect(newerBalance).toHaveText(expectedNewer);
    }
  } finally {
    await page.close();
  }
});
