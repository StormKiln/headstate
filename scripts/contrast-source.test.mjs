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

test('resolves inline and mixed literal surfaces before the unknown-surface fallback', () => {
    for (const source of [
        '<p style={{color:"#ffffff",backgroundColor:"#ffffff"}}>Unreadable</p>',
        '<div style={{backgroundColor:"#ffffff"}}><p style={{color:"#ffffff"}}>Unreadable</p></div>',
        '<p className="bg-white" style={{color:"#ffffff"}}>Unreadable</p>',
        '<p className="text-white" style={{backgroundColor:"#ffffff"}}>Unreadable</p>',
        '<div style={{backgroundColor:"#ffffff"}}><p className="text-white">Unreadable</p></div>',
        '<div className="bg-black"><p className="bg-black text-white" style={{backgroundColor:"#ffffff"}}>Unreadable</p></div>',
    ]) {
        const s = scanSource('inline.tsx', source);
        assert.deepEqual(s.errors, []);
        assert.ok(s.pairs.some(p => ['text-[#ffffff]', 'text-white'].includes(p.fg) && ['bg-[#ffffff]', 'bg-white'].includes(p.bg)), source);
        assert.ok(!s.pairs.some(p => p.bg === 'bg-[#21262d]'), source);
    }
    const disabledSurface = scanSource('inline.tsx', '<button className="text-white disabled:bg-white">Enabled</button>');
    assert.ok(!disabledSurface.pairs.some(p => p.bg === 'bg-white'));
    const valid = scanSource('inline.tsx', '<p style={{color:"#ffffff",backgroundColor:"#000000"}}>Readable</p>');
    assert.ok(valid.pairs.some(p => p.fg === 'text-[#ffffff]' && p.bg === 'bg-[#000000]'));
    const unknown = scanSource('inline.tsx', '<Component><p style={{color:"#ffffff"}}>Unknown inherited surface</p></Component>');
    assert.ok(unknown.pairs.some(p => p.bg === 'bg-[#21262d]'));
    const dynamic = scanSource('inline.tsx', '<p className="text-white" style={{backgroundColor:unknownColor}}>Needs contract</p>');
    assert.ok(dynamic.errors.some(e => e.includes('named contrast contract')));
});

test('inline foregrounds use effective active surfaces locally and on ancestors', () => {
    for (const source of [
        '<button style={{color:"#ffffff"}} className="bg-black disabled:bg-white">Enabled</button>',
        '<button disabled={busy} style={{color:"#ffffff"}} className="bg-black disabled:bg-white">Enabled when idle</button>',
        '<div className="bg-black disabled:bg-white"><span style={{color:"#ffffff"}}>Enabled</span></div>',
        '<button style={{color:"#ffffff"}} className="bg-white dark:bg-black disabled:bg-white">Dark theme</button>',
    ]) {
        const scan = scanSource('states.tsx', source);
        assert.deepEqual(scan.errors, []);
        assert.ok(scan.pairs.some(p => p.bg === 'bg-black'), source);
        assert.ok(!scan.pairs.some(p => p.bg === 'bg-white'), source);
    }
    // Conditional disabling must not erase an unreadable enabled/hover state.
    for (const classes of ['bg-white disabled:bg-black', 'bg-black hover:bg-white disabled:bg-black']) {
        const scan = scanSource('states.tsx', `<button disabled={busy} style={{color:"#ffffff"}} className="${classes}">Active information</button>`);
        assert.ok(scan.pairs.some(p => p.fg === 'text-[#ffffff]' && p.bg === 'bg-white'));
    }
});
