// Real mounted representatives: this is a finite dark-theme matrix, not an
// exhaustive route or native screen-reader certification.
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { extname, join } from 'node:path';
import { chromium } from 'playwright';
import axe from 'axe-core';
const dist = join(process.cwd(), 'dist-harness');
const server = createServer((req, res) => {
    try {
        const path = join(dist, new URL(req.url, 'http://localhost').pathname);
        const body = readFileSync(path);
        res.setHeader('content-type', ({ '.html': 'text/html', '.js': 'application/javascript', '.css': 'text/css', '.woff2': 'font/woff2' })[extname(path)] ?? 'application/octet-stream');
        res.end(body);
    }
    catch {
        res.statusCode = 404;
        res.end();
    }
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const browser = await chromium.launch();
const errors = [], matrix = [];
try {
    for (const fixture of ['controls', 'report', 'bulk']) {
        const page = await browser.newPage({ viewport: { width: 390, height: 844 }, hasTouch: true });
        await page.goto(`http://127.0.0.1:${server.address().port}/harness/contrast.html?case=${fixture}`);
        await page.waitForSelector('main');
        await page.addScriptTag({ content: axe.source });
        if (fixture === 'controls') {
            await page.getByText('/fixture/git', { exact: true }).waitFor();
            await page.locator('[aria-current="true"] span').filter({ hasText: 'your account' }).waitFor();
            const asked = page.getByTestId('transcript-header-asked');
            await asked.tap();
            if (await asked.count() !== 1)
                throw new Error('withheld Asked must not render stale prompt');
            if (await page.getByText('WITHHELD_SENTINEL').count())
                throw new Error('withheld text escaped');
            for (const zoom of [16, 32]) {
                await page.evaluate(size => document.documentElement.style.fontSize = `${size}px`, zoom);
                const shape = await asked.evaluate(el => ({ width: el.clientWidth, scroll: el.scrollWidth, height: el.clientHeight, line: parseFloat(getComputedStyle(el).lineHeight), text: el.textContent, title: el.title }));
                if (shape.scroll > shape.width + 1 || shape.height <= shape.line || !shape.text.includes('last line') || shape.title)
                    errors.push(`Asked ${zoom}px: ${JSON.stringify(shape)}`);
            }
            await page.evaluate(() => document.documentElement.style.fontSize = '16px');
        }
        if (fixture === 'bulk') {
            await page.getByRole('button', { name: 'Mark ready', exact: true }).click();
            await page.getByRole('dialog').waitFor();
        }
        async function check(state) {
            await page.waitForTimeout(200);
            const result = await page.evaluate(async () => window.axe.run(document, { runOnly: ['color-contrast'] }));
            for (const violation of result.violations)
                for (const n of violation.nodes)
                    errors.push(`${fixture}/${state}: ${n.target.join(' ')}: ${n.failureSummary}`);
            if (!result.passes.some(p => p.nodes.length))
                throw new Error(`${fixture}/${state}: no contrast evidence`);
            matrix.push({ fixture, state, passes: result.passes.flatMap(p => p.nodes).length, incomplete: result.incomplete.flatMap(p => p.nodes).length, incompleteTargets: result.incomplete.flatMap(p => p.nodes.map(n => ({ target: n.target, reason: n.failureSummary }))) });
        }
        await check('rest');
        const targets = fixture === 'controls' ? page.locator('[data-contract="buttons"] button:not([disabled]), [role="tab"], [aria-current="true"]') : fixture === 'report' ? page.getByText('Open on GitHub', { exact: true }) : page.getByRole('dialog').locator('li');
        const count = await targets.count();
        if (!count)
            throw new Error(`${fixture}: no representative state targets`);
        for (let i = 0; i < count; i++) {
            const target = targets.nth(i);
            await target.hover();
            await check(`hover-${i}`);
            await target.focus();
            await page.keyboard.press('Tab');
            await page.keyboard.press('Shift+Tab');
            await check(`keyboard-${i}`);
        }
        await page.close();
    }
    // The real details action and selected session row, through the existing
    // app harness rather than a copy of their classes.
    const app = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const state = { claudePage: 'sessions', claudeSelected: '00000000-0000-4000-8000-000000000010', claudeSessionTab: 'details' };
    await app.goto(`http://127.0.0.1:${server.address().port}/harness/shell.html?view=claude-code&state=${encodeURIComponent(JSON.stringify(state))}`);
    const resume = app.getByRole('button', { name: 'Copy resume command', exact: true });
    await resume.waitFor();
    await app.addScriptTag({ content: axe.source });
    for (const state of ['rest', 'hover', 'keyboard']) {
        if (state === 'hover')
            await resume.hover();
        if (state === 'keyboard') {
            await resume.focus();
            await app.keyboard.press('Tab');
            await app.keyboard.press('Shift+Tab');
        }
        await app.waitForTimeout(200);
        const r = await resume.evaluate(async (el) => window.axe.run(el, { runOnly: ['color-contrast'] }));
        for (const v of r.violations)
            for (const n of v.nodes)
                errors.push(`resume/${state}: ${n.failureSummary}`);
        if (!r.passes.some(p => p.nodes.length))
            throw new Error(`resume/${state}: no contrast evidence`);
        matrix.push({ fixture: 'real session resume', state, passes: r.passes.flatMap(p => p.nodes).length, incomplete: r.incomplete.flatMap(p => p.nodes).length, incompleteTargets: r.incomplete.flatMap(p => p.nodes.map(n => ({ target: n.target, reason: n.failureSummary }))) });
    }
    await app.close();
    console.log(JSON.stringify(matrix, null, 2));
    if (errors.length) {
        console.error([...new Set(errors)].join('\n'));
        process.exitCode = 1;
    }
}
finally {
    await browser.close();
    await new Promise(r => server.close(r));
}
