import { chooseOption } from './dropdown.mjs';

export async function chooseAgent(page, option) {
  await chooseOption(page, '当前 Agent', option);
}
