#!/usr/bin/env node
// fab — Fab.com を CLI で操作する（ログイン済みの Chrome を CDP で借りて、そのページの中から Fab の API を叩く）。
//   前提: ログイン済みの Chrome が CDP で開いている（既定 127.0.0.1:9223, 環境変数 FAB_CDP で変更）
//   fab login                                   常駐 Chrome（9223 = spliceclaw と同じプロファイル）で Fab のログイン画面を開く
//   fab import-session <port>                   別の CDP ブラウザのログインを常駐 Chrome へ写す
//   fab status                                  ログイン確認
//   fab library [--limit 200] [--json]             所有ライブラリの一覧（listing UID と library UID を区別）
//   fab search <語> [--free] [--format fbx] [--limit 20] [--json]
//   fab show <uid|URL>                          詳細（形式・ライセンス・所有）
//   fab claim <uid|URL> [--license personal]    無料ライセンスでライブラリに追加（有料は拒否）
//   fab formats <uid|URL>                       ライブラリ内の形式とファイル
//   fab download <uid|URL> --format fbx|glb|gltf|obj|blender|additional-files [--out DIR] [--unzip]
//                                               未所有の無料品は自動で claim してから取得。--only glb で拡張子を絞る
import fs from 'node:fs'; import path from 'node:path'; import { execFileSync } from 'node:child_process';
const CDP = process.env.FAB_CDP || 'http://127.0.0.1:9223';
const argv = process.argv.slice(2); const cmd = argv.shift();
const flag = (n, d) => { const i = argv.indexOf('--' + n); if (i < 0) return d; const v = argv[i + 1]; argv.splice(i, (v && !v.startsWith('--')) ? 2 : 1); return (v && !v.startsWith('--')) ? v : true; };
const uidOf = s => (String(s).match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/) || [])[0] || s;
const die = m => { console.error('fab: ' + m); process.exit(1); };

async function page() {
  let tabs; try { tabs = await (await fetch(CDP + '/json')).json(); } catch { die(`CDP に繋がらない (${CDP})。ログイン済みの Chrome を --remote-debugging-port 付きで開く`); }
  let t = tabs.find(t => t.type === 'page' && /^https:\/\/www\.fab\.com/.test(t.url));
  if (!t) { t = await (await fetch(CDP + '/json/new?https://www.fab.com/', { method: 'PUT' })).json(); await new Promise(r => setTimeout(r, 5000)); }
  const ws = new WebSocket(t.webSocketDebuggerUrl); await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  let id = 0; const pend = new Map();
  ws.onmessage = e => { const m = JSON.parse(e.data); if (pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } };
  const send = (method, params) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  return { ws, async api(method, url, body) {
    const expr = `(async()=>{const c=(document.cookie.match(/fab_csrftoken=([^;]+)/)||[])[1];const o={method:${JSON.stringify(method)},credentials:'include',headers:{'accept':'application/json','x-csrftoken':c||''}};
      ${body ? `const f=new FormData();for(const [k,v] of Object.entries(${JSON.stringify(body)}))f.append(k,v);o.body=f;` : ''}
      const r=await fetch('/i'+${JSON.stringify(url)},o);const t=await r.text();return JSON.stringify({status:r.status,body:t})})()`;
    const m = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
    if (m.result?.exceptionDetails) die('ページ内エラー: ' + JSON.stringify(m.result.exceptionDetails).slice(0, 300));
    const { status, body: b } = JSON.parse(m.result.result.value); let j = null; try { j = JSON.parse(b); } catch {}
    return { status, json: j, text: b };
  } };
}
const out = (o, asJson) => console.log(asJson ? JSON.stringify(o, null, 1) : o);
const fmts = l => (l.assetFormats || []).map(f => f.assetFormatType?.code).filter(Boolean);

if (cmd === 'import-session') { execFileSync('node', [new URL('./import-session.mjs', import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1'), ...argv], { stdio: 'inherit' }); process.exit(0); }
if (cmd === 'login') { await fetch(CDP + '/json/new?https://www.fab.com/login', { method: 'PUT' }).catch(() => die('CDP に繋がらない')); console.log('ログイン画面を開いた。終わったら fab status'); process.exit(0); }
const P = await page();
try {
  if (cmd === 'status') {
    const r = await P.api('GET', '/users/me'); out(r.status === 200 ? `ok: ${r.json.displayName}（ログイン済み）` : `未ログイン (HTTP ${r.status})`);
  } else if (cmd === 'library') {
    const limit = Math.min(1000, +flag('limit', 200)), js = flag('json', false); let cursor = null; const rows = [];
    do {
      const qs = new URLSearchParams({ count: '100', sort_by: '-createdAt', source: 'acquired' }); if (cursor) qs.set('cursor', cursor);
      const r = await P.api('GET', '/library/search?' + qs); if (r.status !== 200) die(`library HTTP ${r.status}`);
      for (const a of r.json?.results || []) { const l = a.listing || {}; rows.push({ uid: l.uid, libraryUid: a.uid, title: l.title, type: l.listingType, formats: fmts(l), by: l.publisher?.sellerName, licenses: a.entitlement?.licenses, createdAt: a.createdAt, thumb: l.thumbnails?.[0]?.mediaUrl }); }
      const next = r.json?.cursors?.next; if (!next || next === cursor) break; cursor = next;
    } while (rows.length < limit);
    const selected = rows.slice(0, limit); js ? out(selected, true) : selected.forEach(l => console.log(`${l.uid}  [${l.formats.join(',')}]  ${l.title}`));
  } else if (cmd === 'search') {
    const free = flag('free', false), fmt = flag('format', null), lim = +flag('limit', 20), js = flag('json', false); const q = argv.filter(a => !a.startsWith('--'));
    const qs = new URLSearchParams({ q: q.join(' '), sort_by: '-relevance' }); if (free) qs.set('is_free', '1');
    const r = await P.api('GET', '/listings/search?' + qs); let res = (r.json?.results || []);
    if (fmt) res = res.filter(l => fmts(l).includes(fmt));
    res = res.slice(0, lim).map(l => ({ uid: l.uid, title: l.title, type: l.listingType, formats: fmts(l), free: l.isFree ?? (l.startingPrice?.price === 0), by: l.user?.sellerName || l.user?.displayName }));
    js ? out(res, true) : res.forEach(l => console.log(`${l.uid}  ${l.free ? 'FREE' : '    '}  [${l.formats.join(',')}]  ${l.title}`));
  } else if (cmd === 'show') {
    const u = uidOf(argv[0]); const r = await P.api('GET', `/listings/${u}`); if (r.status !== 200) die(`HTTP ${r.status}`);
    const st = await P.api('GET', `/users/me/listings-states?listing_ids=${u}`);
    const l = r.json; out({ uid: l.uid, title: l.title, by: l.publisher?.sellerName || l.user?.sellerName, type: l.listingType, free: l.isFree, formats: fmts(l), aiGenerated: l.isAiGenerated,
      licenses: (l.licenses || []).map(x => ({ slug: x.slug, name: x.name, price: x.priceTier?.price, offerId: x.offerId, cc0: x.isCc0 })),
      thumb: l.thumbnails?.[0]?.mediaUrl, state: st.json, url: `https://www.fab.com/listings/${l.uid}` }, true);
  } else if (cmd === 'claim') {
    const u = uidOf(argv[0]); console.log(await claim(u, flag('license', 'personal')));
  } else if (cmd === 'formats') {
    const u = uidOf(argv[0]); const l = (await P.api('GET', `/listings/${u}`)).json;
    for (const f of fmts(l)) { const r = await P.api('GET', `/listings/${u}/asset-formats/${f}`); console.log(f, (r.json?.files || []).map(x => `${x.name} ${(x.size / 1e6).toFixed(1)}MB`).join(', ')); }
  } else if (cmd === 'download') {
    const u = uidOf(argv[0]); const fmt = flag('format', 'fbx'); const dir = path.resolve(flag('out', '.')); const unzip = flag('unzip', false); const only = flag('only', null);
    const own = await P.api('GET', `/users/me/listings-states?listing_ids=${u}`);
    if (!JSON.stringify(own.json || '').includes('"acquired":true')) console.log(await claim(u, 'personal'));
    const r = await P.api('GET', `/listings/${u}/asset-formats/${fmt}`);
    if (r.status !== 200) die(`形式 ${fmt} が取れない (HTTP ${r.status})`);
    const selected = (r.json.files || []).filter(f => !only || f.name.toLowerCase().endsWith((String(only).includes('.') ? '' : '.') + String(only).toLowerCase()));
    if (!selected.length) die('取得できるファイルが無い。UE専用形式はLauncher/Fabプラグインを使用');
    fs.mkdirSync(dir, { recursive: true });
    for (const file of selected) {
      const d = await P.api('GET', `/listings/${u}/asset-formats/${fmt}/files/${file.uid}/download-info`);
      const url = JSON.stringify(d.json || {}).match(/"(https:[^"]+)"/)?.[1];
      if (!url) die('download-info に URL が無い: ' + d.text.slice(0, 300));
      const dest = path.join(dir, path.basename(file.name || `${u}.${fmt}.zip`));
      const curl = process.platform === 'win32' && fs.existsSync('C:/Program Files/Git/mingw64/bin/curl.exe') ? 'C:/Program Files/Git/mingw64/bin/curl.exe' : 'curl';
      execFileSync(curl, ['-fSL', '--retry', '2', '-o', dest, url.replace(/\u0026/g, '&')]); console.log(`saved ${dest} (${fs.statSync(dest).size} bytes)`);
      if (file.size && fs.statSync(dest).size !== file.size) die('サイズ不一致: ' + dest);
      if (unzip && /\.zip$/i.test(dest)) { execFileSync('unzip', ['-qo', dest, '-d', path.join(dir, path.basename(dest, '.zip'))]); console.log('unzipped → ' + path.join(dir, path.basename(dest, '.zip'))); }
    }
    const l = (await P.api('GET', `/listings/${u}`)).json; const lic = (l.licenses || []).find(x => x.priceTier?.price === 0);
    fs.writeFileSync(path.join(dir, 'FAB_SOURCE.json'), JSON.stringify({ uid: u, title: l.title, by: l.publisher?.sellerName || l.user?.sellerName, license: lic?.name, licenseSlug: lic?.slug, licenseBasis: 'listing-free-license-not-entitlement', cc0: lic?.slug === 'cc0', attributionRequired: lic?.slug === 'cc-by', aiGenerated: l.isAiGenerated, url: `https://www.fab.com/listings/${u}`, fetched: new Date().toISOString() }, null, 1));
  } else {
    console.log(fs.readFileSync(new URL(import.meta.url)).toString().split('\n').filter(l => l.startsWith('//')).map(l => l.slice(3)).join('\n'));
  }
} finally { P.ws.close(); }

async function claim(u, slug) {
  const r = await P.api('GET', `/listings/${u}`); const lic = (r.json?.licenses || []).find(x => x.slug === slug) || (r.json?.licenses || [])[0];
  if (!lic) die('ライセンス情報が無い');
  if ((lic.priceTier?.price ?? 1) !== 0) die(`無料ではない (${lic.name}: ${lic.priceTier?.price} ${lic.priceTier?.currencyCode})。購入は Web で`);
  const a = await P.api('POST', `/listings/${u}/add-to-library`, { offer_id: lic.offerId });
  return a.status < 300 ? `claimed: ${r.json.title}（${lic.name}・無料）` : (/already|own/i.test(a.text) ? `既に所有: ${r.json.title}` : `claim 失敗 HTTP ${a.status}: ${a.text.slice(0, 200)}`);
}
