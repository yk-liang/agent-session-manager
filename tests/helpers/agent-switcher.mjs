// Click the real menu so tests cover positioning, focus and configuration guards.
export async function chooseAgent(page, option) {
  await page.getByRole('combobox', { name: '当前 Agent', exact: true }).click();
  const menu = page.getByRole('listbox', { name: 'Agent', exact: true });
  const item = typeof option === 'string'
    ? menu.locator(`[role="option"][value="${option}"]`)
    : menu.getByRole('option', { name: option.label, exact: true });
  await item.click();
  await menu.waitFor({ state: 'hidden' });
}
