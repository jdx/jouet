import { expect, test } from './fixtures';
import { ConfigurationPage } from './page-models/configuration-page';
import { Navigation } from './page-models/navigation';

test('running balances refresh after repeated edits', async ({ browser }) => {
  const page = await browser.newPage();
  try {
    await page.goto('/');
    await new ConfigurationPage(page).createTestFile();
    const account = await new Navigation(page).createAccount({
      name: 'Balance check', offBudget: false, balance: 0,
    });
    await account.waitFor();
    await account.createSingleTransaction({ payee: '', notes: 'check', credit: '17.00' });
    await account.accountMenuButton.click();
    await page.getByRole('button', { name: 'Show running balance' }).click();
    const transaction = account.getNthTransaction(0);
    const balance = account.transactionTableRow.nth(0).getByTestId('balance');
    await expect(balance).toHaveText('17.00');
    for (const amount of ['42.00', '6.50']) {
      await transaction.credit.click();
      const input = transaction.credit.getByRole('textbox');
      await input.selectText();
      await input.pressSequentially(amount);
      await page.keyboard.press('Tab');
      await expect(balance).toHaveText(amount);
    }
  } finally {
    await page.close();
  }
});
