// 別の CDP ブラウザ（例: Pi の一時ブラウザ）でログインした Fab/Epic のセッションを、常駐プロファイル（9223）へ写す。
//   node import-session.mjs <from-port> [to-port=9223]
const [from, to = '9223'] = process.argv.slice(2);
if (!from) { console.error('usage: fab import-session <from-port>'); process.exit(2); }
async function cdp(port) {
  const v = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
  const ws = new WebSocket(v.webSocketDebuggerUrl); await new Promise(r => ws.onopen = r);
  let id = 0; const p = new Map(); ws.onmessage = e => { const m = JSON.parse(e.data); p.get(m.id)?.(m); };
  return { ws, send: (method, params) => new Promise(r => { const i = ++id; p.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); }) };
}
const A = await cdp(from), B = await cdp(to);
const all = (await A.send('Storage.getCookies')).result.cookies.filter(c => /(^|\.)fab\.com$|(^|\.)epicgames\.com$/.test(c.domain.replace(/^\./, '')) || /fab\.com|epicgames\.com/.test(c.domain));
const r = await B.send('Storage.setCookies', { cookies: all.map(({ size, session, priority, sourceScheme, sourcePort, partitionKey, ...c }) => c) });
console.log(r.error ? 'failed: ' + JSON.stringify(r.error) : `copied ${all.length} cookies (${[...new Set(all.map(c => c.name))].filter(n => /session|SID|EPIC|token/i.test(n)).join(', ')})`);
A.ws.close(); B.ws.close();
