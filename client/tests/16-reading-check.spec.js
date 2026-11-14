import { test, expect } from '@playwright/test';
import { bootLines, project, line, reading, silentWav } from './helpers.js';

// Coverage item 16: 読みの確認. The pane-head ear button runs the check for the
// open project and lists, per line, the original text, the AI's reading, what
// was heard, and the respelled text. A fix can be dismissed, playback tells the
// server which line it is so the fix keeps being used, and the row shows that
// its reading was fixed. LineWorkspace.svelte, ReadingModal.svelte, player.

const ICHINICHI = 'それでは今日も一日頑張りましょう。';
const FIXED = 'それでは今日もいちにち頑張りましょう。';

const fixedReading = () =>
	reading('fixed', ICHINICHI, {
		expected: 'それではきょうもいちにちがんばりましょう',
		heard: 'それではきょうもついたちがんばりましょう',
		spoken: FIXED,
		words: [{ surface: '一日', expected: 'いちにち', heard: 'ついたち', action: 'fixed' }]
	});

const fixture = () => ({
	projects: [
		project(
			'イベント告知',
			[
				line('announcer', ICHINICHI, { id: 11 }),
				line('announcer', '十一月一日に出発します。', { id: 12 }),
				line('announcer', '明日は生物の授業があります。', { id: 13 }),
				line('announcer', '確認できない行です。', { id: 14 })
			],
			{ id: 1 }
		)
	],
	readingCheck: {
		11: fixedReading(),
		12: reading('ok', '十一月一日に出発します。', {
			expected: 'じゅういちがつついたちにしゅっぱつします'
		}),
		13: reading('kept', '明日は生物の授業があります。', {
			expected: 'あしたはせいぶつのじゅぎょうがあります',
			heard: 'あすはせいぶつのじゅぎょうがあります',
			words: [{ surface: '明日', expected: 'あした', heard: 'あす', action: 'kept' }]
		}),
		14: { failed: 'openai request failed: connection refused' }
	}
});

const openCheck = async (page) => {
	await page.getByRole('button', { name: '読みを確認' }).click();
	const dialog = page.getByRole('dialog');
	await expect(dialog).toBeVisible();
	return dialog;
};

test('the ear button checks the project and shows what was heard and how it was fixed', async ({
	page
}) => {
	const { requests } = await bootLines(page, fixture());
	const dialog = await openCheck(page);

	const check = requests.find((r) => r.path === '/api/projects/1/reading/check');
	expect(check.json).toEqual({ force: false });

	// The fixed line: original text, the word's two readings, the respelled text.
	const fixed = dialog.locator('.result', { hasText: ICHINICHI });
	await expect(fixed).toContainText('直した');
	await expect(fixed).toContainText('一日');
	await expect(fixed).toContainText('ついたち');
	await expect(fixed).toContainText('いちにち');
	await expect(fixed).toContainText(FIXED);

	// A difference the AI did not insist on is shown but nothing was changed.
	const kept = dialog.locator('.result', { hasText: '明日は生物の授業があります。' });
	await expect(kept).toContainText('そのまま');
	await expect(kept).toContainText('あす');
	await expect(kept.getByRole('button', { name: '直しを外す' })).toHaveCount(0);

	// A line that could not be checked says so, with the reason one step away.
	const failed = dialog.locator('.result', { hasText: '確認できない行です。' });
	await expect(failed).toContainText('確認できず');
	await failed.locator('summary').click();
	await expect(failed).toContainText('connection refused');

	// Correctly read lines are folded into one count.
	const fine = dialog.locator('details.fine');
	await expect(fine.locator('summary')).toHaveText('読み違いなし 1 行');
	await expect(fine.getByText('十一月一日に出発します。')).toBeHidden();
	await fine.locator('summary').click();
	await expect(fine).toContainText('じゅういちがつついたちにしゅっぱつします');
});

test('closing the check leaves a mark on the row whose reading was fixed', async ({ page }) => {
	await bootLines(page, fixture());
	await expect(page.locator('.detail-pane .row .reading-mark')).toHaveCount(0);

	await openCheck(page);
	await page.keyboard.press('Escape');
	await expect(page.getByRole('dialog')).toHaveCount(0);

	const marks = page.locator('.detail-pane .row .reading-mark');
	await expect(marks).toHaveCount(1);
	const row = page.locator('.detail-pane .row', { hasText: ICHINICHI });
	await expect(row.locator('.reading-mark')).toHaveAttribute('title', `読み上げ: ${FIXED}`);
	// The shown text is never replaced by the respelled one.
	await expect(row.locator('.text')).toHaveText(ICHINICHI);
});

test('a fix can be dismissed', async ({ page }) => {
	const { requests } = await bootLines(page, fixture());
	const dialog = await openCheck(page);

	const fixed = dialog.locator('.result', { hasText: ICHINICHI });
	await fixed.getByRole('button', { name: '直しを外す' }).click();
	await expect(fixed).toContainText('外した');
	await expect(fixed.getByRole('button', { name: '直しを外す' })).toHaveCount(0);
	expect(requests.some((r) => r.path === '/api/lines/11/reading/dismiss')).toBe(true);

	await page.keyboard.press('Escape');
	await expect(page.locator('.detail-pane .row .reading-mark')).toHaveCount(0);
});

test('checking again asks the server to redo every line', async ({ page }) => {
	const { requests } = await bootLines(page, fixture());
	const dialog = await openCheck(page);

	await dialog.getByRole('button', { name: '確かめ直す' }).click();
	await expect
		.poll(() => requests.filter((r) => r.path === '/api/projects/1/reading/check').length)
		.toBe(2);
	const second = requests.filter((r) => r.path === '/api/projects/1/reading/check')[1];
	expect(second.json).toEqual({ force: true });
	await expect(dialog.locator('.result', { hasText: ICHINICHI })).toContainText('直した');
});

test('a refused check tells why and changes nothing', async ({ page }) => {
	await bootLines(page, {
		...fixture(),
		readingCheck: { refuse: 'reading check is disabled (set READING_LISTEN_CMD in .env)' }
	});
	const dialog = await openCheck(page);
	await expect(dialog.getByRole('alert')).toContainText('READING_LISTEN_CMD');
	await expect(dialog.locator('.result')).toHaveCount(0);
});

test('other shortcuts do not fire while the check is open', async ({ page }) => {
	const { store } = await bootLines(page, fixture());
	await openCheck(page);
	await page.keyboard.press('d');
	await page.keyboard.press('d'); // dd would delete the focused line
	expect(store.projects[0].lines).toHaveLength(4);
});

test('playback tells the server which line it is, so the fix keeps being used', async ({ page }) => {
	const script = [{ text: '演技の行。', emotion: { honwaka: 40 } }];
	const { requests } = await bootLines(page, {
		projects: [
			project(
				'企画',
				[
					line('announcer', ICHINICHI, { id: 21, reading: fixedReading() }),
					line('acting', '演技の行。', { id: 22, script })
				],
				{ id: 2 }
			)
		],
		sayAudio: silentWav(1)
	});

	const rows = page.locator('.detail-pane .row');
	await rows.nth(0).getByRole('button', { name: '再生' }).click();
	await expect.poll(() => requests.some((r) => r.path === '/say' && r.method === 'GET')).toBe(true);
	const announcer = requests.find((r) => r.path === '/say' && r.method === 'GET');
	expect(announcer.query).toEqual({ text: ICHINICHI, line: '21' });

	await rows.nth(1).getByRole('button', { name: '再生' }).click();
	await expect.poll(() => requests.some((r) => r.path === '/say' && r.method === 'POST')).toBe(true);
	const acting = requests.find((r) => r.path === '/say' && r.method === 'POST');
	expect(acting.query).toEqual({ line: '22' });
	expect(acting.json).toEqual(script);
});
