// Small strict parser for the generated provider documents used by this harness.
// It parses field aliases and argument values; strings/comments cannot become aliases.
export function parseDocument(source, variables={}) {
  const tokens=[];const re=/\s+|#[^\n]*|\.{3}|"(?:\\.|[^"\\])*"|[A-Za-z_][A-Za-z_0-9]*|-?\d+(?:\.\d+)?|[!$():={}\[\],@]/gy;
  let at=0;while(at<source.length){re.lastIndex=at;const match=re.exec(source);if(!match)throw Error('unsupported GraphQL token');at=re.lastIndex;if(!/^\s|^#|^,$/.test(match[0]))tokens.push(match[0]);}
  let i=0;const take=()=>tokens[i++];const expect=t=>{if(take()!==t)throw Error('invalid GraphQL structure');};
  const value=()=>{const t=take();if(t==='$')return variables[take()];if(t==='{'){const o={};while(tokens[i]!=='}'){const k=take();expect(':');o[k]=value();}i++;return o;}if(t==='['){const a=[];while(tokens[i]!==']')a.push(value());i++;return a;}if(t?.startsWith('"'))return JSON.parse(t);if(t==='null')return null;if(t==='true')return true;if(t==='false')return false;if(/^-?\d/.test(t))return Number(t);return t;};
  const fields=()=>{expect('{');const out=[];while(tokens[i]!=='}'){if(i>=tokens.length)throw Error('unterminated selection');if(tokens[i]==='...'){i++;expect('on');take();out.push(...fields());continue;}let name=take(),alias=name;if(tokens[i]===':'){i++;name=take();}const args={};if(tokens[i]==='('){i++;while(tokens[i]!==')'){const k=take();expect(':');args[k]=value();}i++;}if(tokens[i]==='@')throw Error('unsupported directive');const selection=tokens[i]==='{'?fields():[];out.push({name,alias,args,selection});}i++;return out;};
  let kind='query';if(tokens[i]==='query'||tokens[i]==='mutation'){kind=take();if(tokens[i]!=='('&&tokens[i]!=='{')take();if(tokens[i]==='('){let depth=0;do{const t=take();if(t==='(')depth++;if(t===')')depth--;}while(depth&&i<tokens.length);}}
  const selection=fields();if(i!==tokens.length)throw Error('unsupported trailing document');
  return {kind,selection,aliases:selection.filter(f=>f.name!=='rateLimit'&&f.alias!==f.name).length};
}
export function project(fields, object, resolve) {
  if(object===null||object===undefined)return null;
  if(Array.isArray(object))return object.map(item=>project(fields,item,resolve));
  const out={};for(const f of fields){const value=resolve(f,object);out[f.alias]=f.selection.length?project(f.selection,value,resolve):value??null;}return out;
}
