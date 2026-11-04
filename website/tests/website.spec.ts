import { expect, test } from '@playwright/test';
import { posts as allPosts } from '../src/lib/copy';

const routes = ['./', './why/', './zh-CN/', './zh-CN/why/'];

test('English and Chinese routes keep correct language and metadata', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('./');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.locator('h1')).toHaveText('A Rust Cargo that keeps target small on its own');
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute('href', 'https://huacnlee.github.io/cargo/');
  await expect(page.locator('link[hreflang="zh-CN"]')).toHaveAttribute('href', 'https://huacnlee.github.io/cargo/zh-CN/');

  await page.goto('./zh-CN/');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await expect(page.locator('h1')).toHaveText('自动控制 target 大小的 Rust Cargo');
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute('href', 'https://huacnlee.github.io/cargo/zh-CN/');
  expect(errors).toEqual([]);
});

test('the language menu keeps you on the same page', async ({ page }) => {
  await page.goto('./why/');
  await expect(page.locator('h1')).toHaveText('Why I built this');
  await page.locator('#language-toggle').click();
  await page.getByRole('menuitemradio', { name: '简体中文' }).click();
  await expect(page).toHaveURL(/\/cargo\/zh-CN\/why\/$/);
  await expect(page.locator('h1')).toHaveText('为什么做这个项目');
  await page.locator('#language-toggle').click();
  await page.getByRole('menuitemradio', { name: 'English' }).click();
  await expect(page).toHaveURL(/\/cargo\/why\/$/);
});

test('language menu handles keyboard selection, escape and outside click', async ({ page }) => {
  await page.goto('./');
  const toggle = page.getByRole('button', { name: 'Language' });
  await toggle.focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitemradio', { name: 'English' })).toBeFocused();
  await expect(page.getByRole('menuitemradio', { name: 'English' })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('End');
  await expect(page.getByRole('menuitemradio', { name: '简体中文' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(toggle).toBeFocused();
  await expect(page.getByRole('menu')).toHaveCount(0);
  await toggle.click();
  await page.locator('h1').click();
  await expect(page.getByRole('menu')).toHaveCount(0);
});

// Every installed cargo and every published install command depends on these URLs.
for (const [file, marker] of [['install.sh', '#!'], ['install.ps1', 'huacnlee']]) {
  test(`${file} stays served at its published URL`, async ({ page }) => {
    const response = await page.request.get(`/cargo/${file}`);
    expect(response.ok()).toBe(true);
    expect(await response.text()).toContain(marker);
  });
}

test('tabs copy only the active install command', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('./');
  await page.getByRole('button', { name: 'Copy command' }).click();
  await expect(page.getByRole('status')).toHaveText('Copied');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh');
  await page.getByRole('tab', { name: 'macOS and Linux' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'Windows' })).toBeFocused();
  await page.getByRole('button', { name: 'Copy command' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('irm https://huacnlee.github.io/cargo/install.ps1 | iex');
});

test('the theme persists across reload and language', async ({ page }) => {
  await page.goto('./');
  await page.locator('#theme').selectOption('catppuccin');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'catppuccin');
  await page.goto('./zh-CN/why/');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'catppuccin');
  await expect(page.locator('#theme')).toHaveValue('catppuccin');
});

test('both languages render every measurement', async ({ page }) => {
  for (const route of ['./', './zh-CN/']) {
    await page.goto(route);
    await expect(page.locator('.metric')).toHaveCount(7);
    await expect(page.locator('.proof .stat-big strong').first()).toHaveText('5.1 GB');
    await expect(page.locator('.col')).toHaveCount(10);
    await expect(page.locator('.changes li')).toHaveCount(14);
  }
});

test('the screenshot loads and internal links resolve', async ({ page }) => {
  for (const route of routes) {
    await page.goto(route);
    const image = page.locator('.shot img');
    await expect(image).toBeVisible();
    expect(await image.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0)).toBe(true);
    for (const href of await page.locator('a[href^="/cargo/"]').evaluateAll(links => links.map(l => l.getAttribute('href')!))) {
      expect((await page.request.get(href.split('#')[0])).ok(), href).toBe(true);
    }
  }
});

test('the background page lists my X posts, one line each', async ({ page }) => {
  for (const [route, first, text] of [['./why/', /^Sep 17/, 'disk space'], ['./zh-CN/why/', /^9月17日/, '磁盘占用']] as const) {
    await page.goto(route);
    const posts = page.locator('.posts a');
    await expect(posts).toHaveCount(allPosts.length);
    await expect(posts.first()).toHaveAttribute('href', `https://x.com/huacnlee/status/${allPosts[0].id}`);
    await expect(posts.first().locator('time')).toHaveText(first);
    await expect(posts.first()).toContainText(text);
    const heights = await posts.evaluateAll(links => links.map(link => link.getBoundingClientRect().height));
    expect(new Set(heights).size).toBe(1);
  }
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto('./why/');
  const heights = await page.locator('.posts a').evaluateAll(links => links.map(link => link.getBoundingClientRect().height));
  expect(new Set(heights).size).toBe(1);
});

test('every page asks for sponsorship with a visible button', async ({ page }) => {
  for (const route of routes) {
    await page.goto(route);
    const button = page.locator('.sponsor .sponsor-button').first();
    await expect(button).toBeVisible();
    await expect(button).toHaveAttribute('href', 'https://github.com/sponsors/huacnlee');
    for (const link of await page.locator('.sponsor-button').all()) await expect(link).toHaveAttribute('target', '_blank');
    await expect(page.locator('.bar .sponsor-button')).toBeVisible();
  }
});

test('the language and theme controls have no border', async ({ page }) => {
  await page.goto('./');
  await expect(page.locator('#language-toggle')).toHaveCSS('border-top-width', '0px');
  await expect(page.locator('#theme')).toHaveCSS('border-top-width', '0px');
});

test('the sponsor card follows the install section on the home page', async ({ page }) => {
  for (const route of ['./', './zh-CN/']) {
    await page.goto(route);
    await expect(page.locator('.hero > .install ~ .sponsor')).toHaveCount(1);
  }
});

test('the sponsor card is outlined green with a solid green button in every theme', async ({ page }) => {
  await page.goto('./');
  for (const [theme, color] of [['tokyo-night', 'rgb(158, 206, 106)'], ['catppuccin', 'rgb(166, 227, 161)'], ['flexoki-light', 'rgb(102, 128, 11)']]) {
    await page.locator('#theme').selectOption(theme);
    const card = page.locator('.sponsor').first();
    await expect(card).toHaveCSS('border-top-color', color);
    await expect(card).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
    await expect(card.locator('p')).toHaveCSS('color', color);
    await expect(card.locator('.sponsor-button')).toHaveCSS('background-color', color);
  }
});

test('Chinese headings have no trailing full stop', async ({ page }) => {
  for (const route of ['./zh-CN/', './zh-CN/why/']) {
    await page.goto(route);
    for (const text of await page.locator('h1, h2, h3').allTextContents()) {
      expect(text.trim()).not.toMatch(/。$/);
    }
  }
});

for (const width of [360, 768, 1440]) {
  test(`every page fits ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const route of routes) {
      await page.goto(route);
      expect(await page.evaluate(() => document.documentElement.scrollWidth), route).toBeLessThanOrEqual(width);
    }
  });
}
