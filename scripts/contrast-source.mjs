// Source discovery, not a duplicate palette. Tailwind and the browser resolve
// the production utilities/theme. Inheritance needs the rendered matrix too.
import ts from 'typescript';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
function utility(token) {
    let depth = 0, last = -1;
    for (let i = 0; i < token.length; i++) {
        if (token[i] === '[' || token[i] === '(')
            depth++;
        if (token[i] === ']' || token[i] === ')')
            depth--;
        if (token[i] === ':' && depth === 0)
            last = i;
    }
    return { value: token.slice(last + 1), state: token.slice(0, last + 1) };
}
const sizes = /^(?:xs|sm|base|lg|xl|[2-9]xl|left|right|center|justify|start|end|wrap|nowrap|balance|pretty|ellipsis|clip)$/;
function foreground(c) {
    if (!c.startsWith('text-') && !c.startsWith('placeholder:'))
        return false;
    const v = c.slice(5);
    if (v === 'current' || v === 'inherit')
        return false;
    return !sizes.test(v) && !/^\[(?:length:|[\d.]+(?:px|rem|em|%))/.test(v);
}
function background(c) {
    return c.startsWith('bg-') && !/^(?:bg-clip-|bg-gradient|bg-linear|bg-none|bg-cover|bg-center|bg-no-repeat)/.test(c);
}
function owner(node) {
    for (let n = node.parent; n; n = n.parent)
        if (ts.isJsxOpeningElement(n) || ts.isJsxSelfClosingElement(n))
            return n;
}
export function scanSource(file, source) {
    const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const pairs = [], errors = [], classes = new Set();
    const icons = new Set();
    for (const statement of ast.statements)
        if (ts.isImportDeclaration(statement) && statement.moduleSpecifier.text === 'lucide-react') {
            const bindings = statement.importClause?.namedBindings;
            if (bindings && ts.isNamedImports(bindings))
                bindings.elements.forEach(e => icons.add(e.name.text));
        }
    // A literal inline background wins over classes on the same element.
    // Walk known ancestors only; component/conditional inheritance retains
    // the documented raised-surface contract and rendered-matrix coverage.
    function literalBackgrounds(el, inlineOnly = false) {
        const style = el.attributes.properties.find(a => ts.isJsxAttribute(a) && a.name.text === 'style');
        const object = style?.initializer && ts.isJsxExpression(style.initializer) ? style.initializer.expression : undefined;
        if (object && ts.isObjectLiteralExpression(object)) {
            const bg = object.properties.find(p => ts.isPropertyAssignment(p) && p.name.getText(ast).replace(/^["']|["']$/g, '') === 'backgroundColor');
            if (bg && ts.isStringLiteral(bg.initializer))
                return [{ value: `bg-[${bg.initializer.text.replaceAll(' ', '_')}]`, state: '' }];
        }
        if (inlineOnly) return [];
        const attr = el.attributes.properties.find(a => ts.isJsxAttribute(a) && a.name.text === 'className');
        const value = attr?.initializer && ts.isJsxExpression(attr.initializer) ? attr.initializer.expression : attr?.initializer;
        return value && ts.isStringLiteral(value) ? value.text.split(/\s+/).map(utility).filter(t => background(t.value)) : [];
    }
    function knownSurfaces(el, local) {
        const own = el ? literalBackgrounds(el, local !== undefined) : [];
        if (own.length) return own;
        if (local?.length) return local;
        for (let parent = el?.parent?.parent; parent; parent = parent.parent) {
            if (!ts.isJsxElement(parent)) continue;
            const inherited = literalBackgrounds(parent.openingElement);
            if (inherited.length) return inherited;
        }
        return [{ value: 'bg-[#21262d]', state: '' }];
    }
    function visit(n) {
        if (ts.isPropertyAssignment(n) && n.name.getText(ast) === 'opacity' && n.initializer.getText(ast) !== '1') {
            errors.push(`${file}:${ast.getLineAndCharacterOfPosition(n.getStart()).line + 1}: inline opacity needs an explicit disabled/decorative class state`);
        }
        if (ts.isPropertyAssignment(n) && ['color', 'backgroundColor'].includes(n.name.getText(ast)) && owner(n)) {
            const expr = n.initializer.getText(ast), where = `${file}:${ast.getLineAndCharacterOfPosition(n.getStart()).line + 1}`;
            if (ts.isStringLiteral(n.initializer)) {
                const color = n.initializer.text.replaceAll(' ', '_');
                const c = `${n.name.getText(ast) === 'color' ? 'text' : 'bg'}-[${color}]`;
                classes.add(c);
                if (n.name.getText(ast) === 'color') {
                    for (const surface of knownSurfaces(owner(n))) {
                        classes.add(surface.value);
                        pairs.push({ where, fg: c, bg: surface.value, min: 4.5 });
                    }
                }
            }
            else if (/^(?:fg|TONE(?:_COLOUR)?\[.*\])$/.test(expr) && /from ["'][^"']*palette["']/.test(source)) {
                // Finite palette-based tone maps are checked by palette.test.ts.
            }
            else if (!/palette[.\[]|(?:thermalColor|powerColor|barColor|capacityColor|labelForeground)\(|^`#\$\{label.color\}`$/.test(expr)) {
                errors.push(`${where}: dynamic colour requires a named contrast contract: ${expr}`);
            }
        }
        if (ts.isStringLiteralLike(n) || ts.isTemplateHead(n) || ts.isTemplateMiddle(n) || ts.isTemplateTail(n)) {
            const text = n.text;
            const where = `${file}:${ast.getLineAndCharacterOfPosition(n.getStart()).line + 1}`;
            const el = owner(n);
            const attrs = el?.attributes.properties.map(p => p.getText(ast)).join(' ') ?? '';
            const disabled = /(?:^|\s)disabled(?:$|\s|=\{true\})/.test(attrs);
            const decorative = /aria-hidden=(?:"true"|\{true\})/.test(attrs);
            const icon = el && icons.has(el.tagName.getText(ast));
            const tokens = text.split(/\s+/).filter(Boolean).map(utility);
            const active = tokens.filter(t => !/(?:disabled|after:|before:|data-starting-style|data-ending-style|data-\[active=false\])/.test(t.state)).filter(t => !tokens.some(d => d.state === `dark:${t.state}` && d.value.split('-')[0] === t.value.split('-')[0]));
            for (const t of active) {
                if (/^opacity-(?!100$)\d+/.test(t.value) && !disabled && !decorative)
                    errors.push(`${where}: inherited ${t.value} needs a disabled/decorative element, not faded information`);
            }
            const fg = active.filter(t => foreground(t.value));
            for (const f of fg)
                if (/\/\d+$/.test(f.value) && !disabled && !decorative)
                    errors.push(`${where}: ${f.value}: use an opaque readable text tone; alpha inherits the parent's surface`);
            const bg = knownSurfaces(el, active.filter(t => background(t.value)));
            for (const t of [...fg, ...bg])
                classes.add(t.value);
            if (!disabled && !decorative)
                for (const f of fg) {
                    // Explicit states must pair with their stated foreground; absent
                    // state foreground inherits the base. Conditional literal branches
                    // remain separate to avoid pairing mutually exclusive colours.
                    const surfaces = bg.length ? bg : [{ value: 'bg-[#21262d]', state: '' }];
                    for (const b of surfaces) {
                        const condition = f.state.match(/(?:data-(?:checked|active)|aria-(?:selected|checked))/)?.[0];
                        if (condition && bg.some(x => x.state.includes(condition)) && !b.state.includes(condition))
                            continue;
                        if (!f.state && b.state && fg.some(x => x.state === b.state))
                            continue;
                        pairs.push({ where, fg: f.value, bg: b.value, min: icon ? 3 : 4.5 });
                    }
                }
        }
        ts.forEachChild(n, visit);
    }
    visit(ast);
    return { pairs, errors, classes: [...classes] };
}
export function inventory(root = 'src') {
    const files = [], pairs = [], errors = [], classes = new Set();
    function walk(dir) {
        for (const e of readdirSync(dir, { withFileTypes: true })) {
            const path = join(dir, e.name);
            if (e.isDirectory()) {
                if (!['harness', 'fixtures'].includes(e.name))
                    walk(path);
            }
            else if (/\.(tsx?|css)$/.test(path) && !/\.(test|spec)\.|\/(fixtures|scrollShim|testSetup)\./.test(path))
                files.push(path);
        }
    }
    walk(root);
    for (const file of files.filter(f => f.endsWith('.css') && f !== join(root, 'index.css')))
        errors.push(`${file}: register the stylesheet in the compiled contrast entry before adding new CSS surfaces`);
    // Dynamic health colour functions are finite production branches, not
    // test constants. Their actual returned literals are checked as text or
    // meaningful bar graphics; arbitrary new dynamic styles fail discovery.
    const health = ts.createSourceFile('health.ts', readFileSync(join(root, 'lib/health.ts'), 'utf8'), ts.ScriptTarget.Latest, true);
    for (const n of health.statements)
        if (ts.isFunctionDeclaration(n) && ['thermalColor', 'powerColor', 'barColor', 'capacityColor'].includes(n.name?.text)) {
            const visit = (node) => { if (ts.isStringLiteral(node) && /^#[0-9a-f]+$/i.test(node.text)) {
                const fg = `text-[${node.text}]`;
                classes.add(fg);
                pairs.push({ where: `${root}/lib/health.ts:${health.getLineAndCharacterOfPosition(node.getStart()).line + 1}`, fg, bg: 'bg-[#21262d]', min: ['barColor', 'capacityColor'].includes(n.name.text) ? 3 : 4.5 });
            } ts.forEachChild(node, visit); };
            visit(n);
        }
    for (const file of files.filter(f => !f.endsWith('.css'))) {
        const scan = scanSource(file, readFileSync(file, 'utf8'));
        pairs.push(...scan.pairs);
        errors.push(...scan.errors);
        scan.classes.forEach(c => classes.add(c));
    }
    return { files, pairs, errors, classes: [...classes] };
}
