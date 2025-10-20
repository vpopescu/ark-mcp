import { test, expect } from '@playwright/test';

test('frontend behavior with auth disabled', async ({ page }) => {
    // Navigate to the admin page
    await page.goto('/admin');

    // Wait for the app to load - check for a main element
    await page.waitForSelector('main');

    // Check that no login button is present
    const loginButton = page.locator('button:has-text("log in")');
    await expect(loginButton).toHaveCount(0);

    // Check that no user menu is present
    const userMenu = page.locator('[aria-label="User menu"]');
    await expect(userMenu).toHaveCount(0);

    // Check that the stats tab is enabled (not disabled)
    const statsTab = page.locator('[role="tab"][value="stats"]');
    await expect(statsTab).not.toBeDisabled();
});