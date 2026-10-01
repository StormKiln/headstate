import { test } from 'node:test';
import assert from 'node:assert/strict';
import { scanSource, inventory } from './contrast-source.mjs';
test('discovers actual text/state pairs, alpha and inherited opacity, ignoring comments', () => {
    const scan = scanSource('new.tsx', `// text-[#123456]\nconst x=<div className="opacity-60"><button className="text-white bg-[#238636] hover:bg-[#2ea043]">Go</button></div>`);
    assert.equal(scan.errors.length, 1);
    assert.match(scan.errors[0], /new.tsx:2.*opacity/);
    assert.ok(scan.pairs.some(p => p.fg === 'text-white' && p.bg === 'bg-[#2ea043]'));
    assert.ok(!JSON.stringify(scan).includes('123456'));
    assert.ok(scanSource('new.tsx', '<span className="text-white/80">count</span>').pairs.some(p => p.fg === 'text-white/80'));
});
test('allows genuine disabled controls, decorative opacity and catches new files', () => {
    assert.deepEqual(scanSource('x.tsx', '<button disabled className="opacity-50 text-[#6e7681]">Unavailable</button>').errors, []);
    assert.deepEqual(scanSource('x.tsx', '<div aria-hidden="true" className="opacity-50 bg-black/10" />').errors, []);
    const all = inventory();
    for (const name of ['src/App.tsx', 'src/index.css', 'src/components/ui/button.tsx', 'src/components/ReportDialog.tsx'])
        assert.ok(all.files.includes(name));
    assert.ok(all.pairs.length > 100);
});
test('pairs literal inherited surfaces and refuses faded text and inline opacity', () => {
    const s = scanSource('new.tsx', '<div className="bg-white"><span className="text-white/80">info</span></div>');
    assert.ok(s.errors.some(e => e.includes('opaque')));
    assert.ok(s.pairs.some(p => p.bg === 'bg-white'));
    assert.ok(scanSource('new.tsx', '<p style={{opacity:0.5}}>info</p>').errors.length);
    assert.ok(scanSource('new.tsx', '<p style={{color:unknownColour}}>info</p>').errors.length);
    const disabled = scanSource('new.tsx', '<button disabled={busy} className="text-[#777] hover:bg-white disabled:opacity-50">Go</button>');
    assert.ok(disabled.pairs.some(p => p.fg === 'text-[#777]' && p.bg === 'bg-white'));
});
