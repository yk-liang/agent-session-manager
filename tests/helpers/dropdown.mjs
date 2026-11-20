export async function openDropdown(page, label) {
  await page.getByRole('combobox', { name: label, exact: true }).click();
  const list = page.getByRole('listbox', { name: label, exact: true });
  await list.waitFor();
  return list;
}

export async function chooseOption(page, label, option) {
  const list = await openDropdown(page, label);
  const item = typeof option === 'string'
    ? list.locator(`[role="option"][value=${JSON.stringify(option)}]`)
    : list.getByRole('option', { name: option.label, exact: true });
  await item.click();
  await list.waitFor({ state: 'hidden' });
}
