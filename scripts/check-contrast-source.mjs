// Compiles discovered production utilities with the real theme; no copied
// colour constants. Conservative literal/state contracts complement the
// mounted component matrix (they cannot infer arbitrary React inheritance).
import { readFileSync } from 'node:fs';
import { compile } from '@tailwindcss/node';
import { chromium } from 'playwright';
import { inventory } from './contrast-source.mjs';
const data = inventory();
const compiler = await compile(readFileSync('src/index.css', 'utf8'), { base: process.cwd() + '/src', onDependency() { } });
const css = compiler.build([...data.classes, 'bg-[#21262d]']);
const browser = await chromium.launch();
try {
    const page = await browser.newPage();
    await page.setContent('<html class="dark"><head></head><body></body></html>');
    await page.addStyleTag({ content: css });
    const failures = await page.evaluate(({ pairs }) => {
        const canvas = document.createElement('canvas');
        canvas.width = canvas.height = 1;
        const ctx = canvas.getContext('2d', { willReadFrequently: true });
        function rgba(c) { ctx.clearRect(0, 0, 1, 1); ctx.fillStyle = c; ctx.fillRect(0, 0, 1, 1); return [...ctx.getImageData(0, 0, 1, 1).data].map((v, i) => i === 3 ? v / 255 : v); }
        const blend = (a, b) => a.slice(0, 3).map((v, i) => v * a[3] + b[i] * (1 - a[3]));
        const lum = c => c.map(v => v / 255).map(v => v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4).reduce((s, v, i) => s + v * [.2126, .7152, .0722][i], 0);
        const ratio = (a, b) => (Math.max(lum(a), lum(b)) + .05) / (Math.min(lum(a), lum(b)) + .05);
        const failures = [];
        const root = document.createElement('div');
        root.style.background = '#21262d';
        document.body.append(root);
        const ground = rgba(getComputedStyle(root).backgroundColor);
        for (const p of pairs) {
            const bg = document.createElement('div'), fg = document.createElement('span');
            bg.className = p.bg;
            fg.className = p.fg;
            bg.style.color = 'rgb(1, 2, 3)';
            bg.append(fg);
            root.append(bg);
            const a = getComputedStyle(fg), b = getComputedStyle(bg);
            const background = blend(rgba(b.backgroundColor), ground), foreground = blend(rgba(a.color), background);
            const r = ratio(foreground, background);
            if (a.color === 'rgb(1, 2, 3)')
                failures.push(`${p.where}: unclassified foreground utility ${p.fg}`);
            else if (r < p.min)
                failures.push(`${p.where}: ${p.fg} on ${p.bg}: ${r.toFixed(2)} < ${p.min}`);
            bg.remove();
        }
        // Every declared semantic foreground/background pair in the production
        // dark theme, including shared primitives, is measured from computed CSS.
        const vars = ['', 'card', 'popover', 'primary', 'secondary', 'muted', 'accent', 'sidebar', 'sidebar-primary', 'sidebar-accent'];
        for (const name of vars) {
            const s = getComputedStyle(document.documentElement), f = s.getPropertyValue(`--${name ? name + '-' : ''}foreground`), b = s.getPropertyValue(`--${name || 'background'}`);
            const r = ratio(rgba(f), blend(rgba(b), ground));
            if (r < 4.5)
                failures.push(`src/index.css: ${name || 'body'} semantic pair ${r.toFixed(2)} < 4.5`);
        }
        return failures;
    }, data);
    const errors = [...data.errors, ...failures];
    console.log(`contrast source: ${data.files.length} production files, ${data.pairs.length} literal/state pairs, ${data.classes.length} resolved utilities`);
    if (errors.length) {
        console.error([...new Set(errors)].join('\n'));
        process.exitCode = 1;
    }
}
finally {
    await browser.close();
}
